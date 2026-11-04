//! IP 地址管理（IPAM）
//!
//! 简易实现：基于数据库租约表的 DHCP 分配器
//! - 网络定义 CIDR + DHCP 范围
//! - 分配时在可用范围内找第一个未占用 IP
//! - 释放时标记租约过期

use crate::repository::Repository;
use crate::Result;
use gita_vm_api::ids::VmId;
use gita_vm_api::network::{IpLease, LeaseState, Network};
use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::str::FromStr;

/// IPAM 分配器
pub struct Ipam<'a> {
    repo: &'a dyn Repository,
    network: &'a Network,
}

impl<'a> Ipam<'a> {
    pub fn new(repo: &'a dyn Repository, network: &'a Network) -> Self {
        Self { repo, network }
    }

    /// 为 VM 分配一个 IP（及可选 MAC）
    ///
    /// 分配是「读空闲地址 → 写入租约」两步，两个并发调用会读到同一份
    /// 租约快照、挑中同一个地址。真正防止重复分配的**只有**存储层的
    /// 唯一索引（`vm_ip_leases(network_id, ip) WHERE state IN
    /// ('allocated','reserved')`）：后写入的那次会拿到唯一约束冲突。
    /// 这里把这个冲突当作「有人先占了这个地址」，重新读一遍快照再挑，
    /// 而不是把错误抛给调用方。
    ///
    /// 重试次数有上限：如果每一次都冲突，说明地址实际上已经耗尽了，
    /// 这时报 `IpPoolExhausted` 比无限重试更诚实。
    pub async fn allocate(&self, vm_id: VmId, node_id: Option<i64>) -> Result<(String, String)> {
        // 每次重试都重新读租约表：上一次冲突意味着快照已经过期。
        const MAX_ATTEMPTS: usize = 8;
        let mut last_conflict = None;

        for _ in 0..MAX_ATTEMPTS {
            let (ip, mac) = self.pick_free().await?;
            let lease = IpLease {
                id: 0, // 由存储层分配
                network_id: self.network.id,
                ip: ip.clone(),
                mac: mac.clone(),
                vm_id: Some(vm_id),
                node_id: node_id.map(gita_vm_api::NodeId),
                state: LeaseState::Allocated,
                expires_at_ms: None, // 持久租约，VM 删除时释放
                created_at_ms: crate::now_ms(),
            };
            match self.repo.create_ip_lease(&lease).await {
                Ok(_) => return Ok((ip, mac)),
                Err(crate::Error::AlreadyExists(_)) => {
                    // 别人先占了：重读快照，再挑下一个。
                    last_conflict = Some(ip);
                }
                Err(e) => return Err(e),
            }
        }

        Err(crate::Error::IpPoolExhausted(format!(
            "连续 {MAX_ATTEMPTS} 次分配都撞上已占用的地址（最后一次：{}）",
            last_conflict.as_deref().unwrap_or("?")
        )))
    }

    /// 释放 VM 的 IP（标记租约过期）
    pub async fn release(&self, vm_id: VmId) -> Result<()> {
        self.repo.release_ip_lease_for_vm(vm_id).await?;
        Ok(())
    }

    /// 预留 IP（静态分配场景）
    pub async fn reserve(&self, ip: &str, mac: &str) -> Result<()> {
        // 校验 IP 在范围内
        let ipv4 = Ipv4Addr::from_str(ip).map_err(|e| crate::Error::Validation(e.to_string()))?;
        self.check_in_dhcp_range(&ipv4)?;

        // 检查是否已被占用
        if self.repo.get_ip_lease_by_ip(ip).await?.is_some() {
            return Err(crate::Error::AlreadyExists(format!("IP {} 已被占用", ip)));
        }

        let lease = IpLease {
            id: 0,
            network_id: self.network.id,
            ip: ip.to_string(),
            mac: mac.to_string(),
            vm_id: None,
            node_id: None,
            state: LeaseState::Reserved,
            expires_at_ms: None,
            created_at_ms: crate::now_ms(),
        };
        // The read above is advisory; the unique index decides. Two
        // concurrent reservations of the same address both see it free,
        // and only the insert can tell them apart. The store maps a
        // unique violation to `AlreadyExists`, which is exactly the
        // error a caller of `reserve` expects — so the error propagates
        // unchanged, and a genuine storage failure is not disguised as
        // "the address is taken".
        self.repo.create_ip_lease(&lease).await?;
        Ok(())
    }

    /// 获取 VM 的租约
    pub async fn get_lease(&self, vm_id: VmId) -> Result<Option<IpLease>> {
        self.repo.get_ip_lease_by_vm(vm_id).await
    }

    /// 在 DHCP 范围内挑选第一个未占用的 IP
    async fn pick_free(&self) -> Result<(String, String)> {
        let leases = self.repo.list_ip_leases(self.network.id).await?;
        let used: BTreeSet<Ipv4Addr> = leases
            .iter()
            .filter(|l| matches!(l.state, LeaseState::Allocated | LeaseState::Reserved))
            .filter_map(|l| Ipv4Addr::from_str(&l.ip).ok())
            .collect();

        let start = self
            .network
            .dhcp_start
            .as_ref()
            .ok_or_else(|| crate::Error::Validation("网络未配置 DHCP 范围".into()))?;
        let end = self
            .network
            .dhcp_end
            .as_ref()
            .ok_or_else(|| crate::Error::Validation("网络未配置 DHCP 范围".into()))?;

        let start_ip = Ipv4Addr::from_str(start)
            .map_err(|e| crate::Error::Validation(format!("DHCP start IP 无效: {}", e)))?;
        let end_ip = Ipv4Addr::from_str(end)
            .map_err(|e| crate::Error::Validation(format!("DHCP end IP 无效: {}", e)))?;

        let mut ip = start_ip;
        while ip <= end_ip {
            if !used.contains(&ip) {
                let mac = generate_mac(ip);
                return Ok((ip.to_string(), mac));
            }
            ip = next_ip(ip);
        }

        Err(crate::Error::IpPoolExhausted("DHCP 范围内无可用 IP".into()))
    }

    fn check_in_dhcp_range(&self, ip: &Ipv4Addr) -> Result<()> {
        let start = self
            .network
            .dhcp_start
            .as_ref()
            .ok_or_else(|| crate::Error::Validation("网络未配置 DHCP 范围".into()))?;
        let end = self
            .network
            .dhcp_end
            .as_ref()
            .ok_or_else(|| crate::Error::Validation("网络未配置 DHCP 范围".into()))?;
        let start_ip =
            Ipv4Addr::from_str(start).map_err(|e| crate::Error::Validation(e.to_string()))?;
        let end_ip =
            Ipv4Addr::from_str(end).map_err(|e| crate::Error::Validation(e.to_string()))?;
        if *ip < start_ip || *ip > end_ip {
            return Err(crate::Error::Validation("IP 不在 DHCP 范围内".into()));
        }
        Ok(())
    }
}

/// 生成 MAC 地址：基于 IP 的后 3 字节 + 固定前缀
fn generate_mac(ip: Ipv4Addr) -> String {
    let octets = ip.octets();
    format!(
        "52:54:{:02x}:{:02x}:{:02x}:{:02x}",
        0x00, octets[1], octets[2], octets[3]
    )
}

/// 计算下一个 IPv4 地址
fn next_ip(ip: Ipv4Addr) -> Ipv4Addr {
    let octets = ip.octets();
    let mut val = u32::from_be_bytes(octets);
    val = val.wrapping_add(1);
    Ipv4Addr::from(val.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::ids::{NetworkId, VmId};
    use gita_vm_api::network::{IpLease, LeaseState, NetProvider, Network};

    fn test_network() -> Network {
        Network {
            id: NetworkId(1),
            name: "test".into(),
            provider: NetProvider::Bridge,
            bridge: "br-test".into(),
            cidr: "10.42.0.0/24".into(),
            gateway: "10.42.0.1".into(),
            dhcp_start: Some("10.42.0.100".into()),
            dhcp_end: Some("10.42.0.200".into()),
            mtu: 1500,
            vlan: None,
            owner: gita_vm_api::OwnerRef::system(),
            is_default: true,
            created_at_ms: 0,
        }
    }

    struct MockRepo {
        leases: std::sync::Mutex<Vec<IpLease>>,
        /// When set, `create_ip_lease` yields to the other task between
        /// the caller's read of the lease list and the insert. That
        /// widens the window in which two allocations both see the same
        /// address free, so the test exercises the unique-index race
        /// rather than merely running two calls back to back.
        yield_before_insert: bool,
    }

    impl MockRepo {
        fn new() -> Self {
            Self {
                leases: std::sync::Mutex::new(vec![]),
                yield_before_insert: false,
            }
        }

        fn with_yielding_insert() -> Self {
            Self {
                leases: std::sync::Mutex::new(vec![]),
                yield_before_insert: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::repository::Repository for MockRepo {
        async fn create_ip_lease(&self, lease: &IpLease) -> crate::Result<i64> {
            if self.yield_before_insert {
                // Hand the other task a turn so it also reads the
                // pre-insert snapshot of the lease list.
                tokio::task::yield_now().await;
            }
            let mut l = lease.clone();
            let mut leases = self.leases.lock().unwrap();
            // Enforce the same rule the database's partial unique index
            // enforces: an address may be held by only one active lease.
            let taken = leases.iter().any(|existing| {
                existing.network_id == l.network_id
                    && existing.ip == l.ip
                    && matches!(existing.state, LeaseState::Allocated | LeaseState::Reserved)
            });
            if taken {
                return Err(crate::Error::AlreadyExists(format!("IP {} 已被占用", l.ip)));
            }
            l.id = leases.len() as i64 + 1;
            leases.push(l.clone());
            Ok(l.id)
        }

        async fn get_ip_lease_by_ip(&self, ip: &str) -> crate::Result<Option<IpLease>> {
            let leases = self.leases.lock().unwrap();
            Ok(leases.iter().find(|l| l.ip == ip).cloned())
        }

        async fn get_ip_lease_by_vm(&self, vm_id: VmId) -> crate::Result<Option<IpLease>> {
            let leases = self.leases.lock().unwrap();
            Ok(leases.iter().find(|l| l.vm_id == Some(vm_id)).cloned())
        }

        async fn release_ip_lease_for_vm(&self, vm_id: VmId) -> crate::Result<()> {
            let mut leases = self.leases.lock().unwrap();
            if let Some(l) = leases.iter_mut().find(|l| l.vm_id == Some(vm_id)) {
                l.state = LeaseState::Released;
                l.expires_at_ms = Some(crate::now_ms());
            }
            Ok(())
        }

        async fn list_ip_leases(&self, network_id: NetworkId) -> crate::Result<Vec<IpLease>> {
            let leases = self.leases.lock().unwrap();
            Ok(leases
                .iter()
                .filter(|l| l.network_id == network_id)
                .cloned()
                .collect())
        }

        // 其余方法存根
        async fn create_vm(&self, _: &gita_vm_api::Vm) -> crate::Result<()> {
            Ok(())
        }
        async fn get_vm(&self, _: VmId) -> crate::Result<Option<gita_vm_api::Vm>> {
            Ok(None)
        }
        async fn update_vm(&self, _: &gita_vm_api::Vm) -> crate::Result<()> {
            Ok(())
        }
        async fn delete_vm(&self, _: VmId) -> crate::Result<()> {
            Ok(())
        }
        async fn list_vms(
            &self,
            _: Option<gita_vm_api::ids::OwnerRef>,
        ) -> crate::Result<Vec<gita_vm_api::Vm>> {
            Ok(vec![])
        }
        async fn create_template(&self, _: &gita_vm_api::VmTemplate) -> crate::Result<()> {
            Ok(())
        }
        async fn get_template(
            &self,
            _: gita_vm_api::TemplateId,
        ) -> crate::Result<Option<gita_vm_api::VmTemplate>> {
            Ok(None)
        }
        async fn update_template(&self, _: &gita_vm_api::VmTemplate) -> crate::Result<()> {
            Ok(())
        }
        async fn delete_template(&self, _: gita_vm_api::TemplateId) -> crate::Result<()> {
            Ok(())
        }
        async fn list_templates(
            &self,
            _: Option<gita_vm_api::ids::OwnerRef>,
        ) -> crate::Result<Vec<gita_vm_api::VmTemplate>> {
            Ok(vec![])
        }
        async fn create_node(&self, _: &gita_vm_api::Node) -> crate::Result<()> {
            Ok(())
        }
        async fn get_node(
            &self,
            _: gita_vm_api::NodeId,
        ) -> crate::Result<Option<gita_vm_api::Node>> {
            Ok(None)
        }
        async fn update_node(&self, _: &gita_vm_api::Node) -> crate::Result<()> {
            Ok(())
        }
        async fn delete_node(&self, _: gita_vm_api::NodeId) -> crate::Result<()> {
            Ok(())
        }
        async fn list_nodes(
            &self,
            _: Option<gita_vm_api::ids::OwnerRef>,
        ) -> crate::Result<Vec<gita_vm_api::Node>> {
            Ok(vec![])
        }
        async fn create_image(&self, _: &gita_vm_api::Image) -> crate::Result<()> {
            Ok(())
        }
        async fn get_image(
            &self,
            _: gita_vm_api::ImageId,
        ) -> crate::Result<Option<gita_vm_api::Image>> {
            Ok(None)
        }
        async fn update_image(&self, _: &gita_vm_api::Image) -> crate::Result<()> {
            Ok(())
        }
        async fn delete_image(&self, _: gita_vm_api::ImageId) -> crate::Result<()> {
            Ok(())
        }
        async fn list_images(
            &self,
            _: Option<gita_vm_api::ids::OwnerRef>,
        ) -> crate::Result<Vec<gita_vm_api::Image>> {
            Ok(vec![])
        }
        async fn create_operation(&self, _: &gita_vm_api::Operation) -> crate::Result<()> {
            Ok(())
        }
        async fn get_operation(
            &self,
            _: gita_vm_api::ids::OperationId,
        ) -> crate::Result<Option<gita_vm_api::Operation>> {
            Ok(None)
        }
        async fn update_operation(&self, _: &gita_vm_api::Operation) -> crate::Result<()> {
            Ok(())
        }
        async fn list_operations(
            &self,
            _: Option<gita_vm_api::NodeId>,
        ) -> crate::Result<Vec<gita_vm_api::Operation>> {
            Ok(vec![])
        }
        async fn create_quota(&self, _: &gita_vm_api::Quota) -> crate::Result<()> {
            Ok(())
        }
        async fn get_quota(
            &self,
            _: gita_vm_api::quota::QuotaScope,
            _: i64,
        ) -> crate::Result<Option<gita_vm_api::Quota>> {
            Ok(None)
        }
        async fn update_quota(&self, _: &gita_vm_api::Quota) -> crate::Result<()> {
            Ok(())
        }
        async fn delete_quota(
            &self,
            _: gita_vm_api::quota::QuotaScope,
            _: i64,
        ) -> crate::Result<()> {
            Ok(())
        }
        async fn list_quotas(&self) -> crate::Result<Vec<gita_vm_api::Quota>> {
            Ok(vec![])
        }
        async fn get_quota_usage(
            &self,
            _: gita_vm_api::quota::QuotaScope,
            _: i64,
        ) -> crate::Result<gita_vm_api::QuotaUsage> {
            Ok(Default::default())
        }
        async fn append_event(&self, _: &gita_vm_api::Event) -> crate::Result<()> {
            Ok(())
        }
        async fn list_events(
            &self,
            _: Option<i64>,
            _: Option<&str>,
            _: i64,
            _: i64,
        ) -> crate::Result<Vec<gita_vm_api::Event>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn allocate_two_ips() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let (ip1, mac1) = ipam.allocate(VmId(1), None).await.unwrap();
        let (ip2, _mac2) = ipam.allocate(VmId(2), None).await.unwrap();

        assert_ne!(ip1, ip2);
        assert!(ip1.starts_with("10.42.0."));
        assert!(ip2.starts_with("10.42.0."));
        assert_eq!(mac1, "52:54:00:2a:00:64"); // 10.42.0.100
    }

    #[tokio::test]
    async fn release_frees_ip() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let (ip1, _) = ipam.allocate(VmId(1), None).await.unwrap();
        ipam.release(VmId(1)).await.unwrap();

        // 再次分配应该复用同一个 IP
        let (ip2, _) = ipam.allocate(VmId(2), None).await.unwrap();
        assert_eq!(ip1, ip2);
    }

    /// A reserved address is held for whoever asked for it: the next
    /// allocation steps over it.
    #[tokio::test]
    async fn a_reserved_address_is_not_handed_out() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        ipam.reserve("10.42.0.100", "52:54:00:2a:00:64")
            .await
            .unwrap();

        let (ip, _) = ipam.allocate(VmId(1), None).await.unwrap();
        assert_eq!(ip, "10.42.0.101", "the reserved address must be skipped");
    }

    /// The same address cannot be reserved twice.
    #[tokio::test]
    async fn an_address_cannot_be_reserved_twice() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        ipam.reserve("10.42.0.100", "52:54:00:2a:00:64")
            .await
            .unwrap();
        let err = ipam
            .reserve("10.42.0.100", "52:54:00:2a:00:64")
            .await
            .expect_err("the second reservation must fail");
        assert!(matches!(err, crate::Error::AlreadyExists(_)), "got {err:?}");
    }

    /// An address outside the range the network hands out is refused:
    /// it belongs to something else.
    #[tokio::test]
    async fn an_address_outside_the_range_is_refused() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let err = ipam
            .reserve("10.42.0.250", "52:54:00:2a:00:fa")
            .await
            .expect_err("the reservation must fail");
        assert!(matches!(err, crate::Error::Validation(_)), "got {err:?}");
    }

    /// An address that is not an address at all is refused.
    #[tokio::test]
    async fn something_that_is_not_an_address_is_refused() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let err = ipam
            .reserve("not an ip", "52:54:00:2a:00:64")
            .await
            .expect_err("the reservation must fail");
        assert!(matches!(err, crate::Error::Validation(_)), "got {err:?}");
    }

    /// When every address is taken, the allocator says so rather than
    /// handing out one twice.
    #[tokio::test]
    async fn a_pool_with_nothing_left_says_so() {
        let mut net = test_network();
        // A range of one address: it is taken after the first VM.
        net.dhcp_start = Some("10.42.0.100".into());
        net.dhcp_end = Some("10.42.0.100".into());

        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let (ip, _) = ipam.allocate(VmId(1), None).await.unwrap();
        assert_eq!(ip, "10.42.0.100");

        let err = ipam
            .allocate(VmId(2), None)
            .await
            .expect_err("the pool is empty");
        assert!(
            matches!(err, crate::Error::IpPoolExhausted(_)),
            "got {err:?}"
        );
    }

    /// A network that says nothing about a range hands out nothing.
    #[tokio::test]
    async fn a_network_without_a_range_hands_out_nothing() {
        let mut net = test_network();
        net.dhcp_start = None;
        net.dhcp_end = None;

        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let err = ipam
            .allocate(VmId(1), None)
            .await
            .expect_err("there is no range to pick from");
        assert!(matches!(err, crate::Error::Validation(_)), "got {err:?}");
    }

    /// Two allocations racing on one free address must end up with two
    /// different addresses.
    ///
    /// The allocator reads the lease table, picks the first free address,
    /// and inserts. Both callers can read before either inserts, so both
    /// pick `10.42.0.100`. The unique index is what stops them both from
    /// holding it — and the allocator has to turn that refusal into a
    /// retry, not into an error. This is the test that fails if the retry
    /// loop is removed.
    #[tokio::test]
    async fn concurrent_allocations_never_share_an_address() {
        let net = test_network();
        let repo = MockRepo::with_yielding_insert();
        let ipam = Ipam::new(&repo, &net);

        let (a, b) = tokio::join!(ipam.allocate(VmId(1), None), ipam.allocate(VmId(2), None));
        let (ip_a, mac_a) = a.expect("the first allocation must succeed");
        let (ip_b, mac_b) = b.expect("the second allocation must succeed");

        assert_ne!(
            ip_a, ip_b,
            "two concurrent allocations were handed the same address"
        );
        assert_ne!(
            mac_a, mac_b,
            "the MAC is derived from the IP and must differ too"
        );

        // Both leases are really there, and each VM owns exactly one.
        let leases = ipam.repo.list_ip_leases(net.id).await.unwrap();
        let active: Vec<&IpLease> = leases
            .iter()
            .filter(|l| matches!(l.state, LeaseState::Allocated | LeaseState::Reserved))
            .collect();
        assert_eq!(active.len(), 2, "both allocations must have a lease");
        let held: BTreeSet<&str> = active.iter().map(|l| l.ip.as_str()).collect();
        assert_eq!(held.len(), 2, "the two leases must name two addresses");
    }

    /// When every retry collides — the pool really is being drained by
    /// other callers — the allocator reports exhaustion rather than
    /// looping forever.
    #[tokio::test]
    async fn a_pool_that_keeps_colliding_reports_exhaustion() {
        let mut net = test_network();
        // One address in the range.
        net.dhcp_start = Some("10.42.0.100".into());
        net.dhcp_end = Some("10.42.0.100".into());

        // A repository whose create always refuses, as a fully-consumed
        // pool would look from the allocator's side.
        struct AlwaysTaken;
        #[async_trait::async_trait]
        impl crate::repository::Repository for AlwaysTaken {
            async fn list_ip_leases(&self, _: NetworkId) -> crate::Result<Vec<IpLease>> {
                // Report nothing held, so `pick_free` keeps choosing the
                // one address and the insert keeps refusing it.
                Ok(vec![])
            }
            async fn create_ip_lease(&self, lease: &IpLease) -> crate::Result<i64> {
                Err(crate::Error::AlreadyExists(format!(
                    "IP {} 已被占用",
                    lease.ip
                )))
            }
        }

        let repo = AlwaysTaken;
        let ipam = Ipam::new(&repo, &net);
        let err = ipam
            .allocate(VmId(1), None)
            .await
            .expect_err("every attempt collides");
        assert!(
            matches!(err, crate::Error::IpPoolExhausted(_)),
            "a permanently colliding pool is exhausted, got {err:?}"
        );
    }

    /// The lease of a VM is the one that was handed to it.
    #[tokio::test]
    async fn the_lease_of_a_vm_is_found() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        let (ip, _) = ipam.allocate(VmId(1), Some(7)).await.unwrap();
        let lease = ipam.get_lease(VmId(1)).await.unwrap().expect("a lease");
        assert_eq!(lease.ip, ip);
        assert_eq!(lease.node_id, Some(gita_vm_api::ids::NodeId(7)));
    }

    /// A VM that was never given an address has no lease.
    #[tokio::test]
    async fn a_vm_without_an_address_has_no_lease() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        assert!(ipam.get_lease(VmId(99)).await.unwrap().is_none());
    }

    #[test]
    fn mac_generation() {
        let ip = Ipv4Addr::new(10, 42, 0, 100);
        let mac = generate_mac(ip);
        assert_eq!(mac, "52:54:00:2a:00:64");
    }

    #[test]
    fn next_ip_increment() {
        let ip = Ipv4Addr::new(10, 42, 0, 100);
        let next = next_ip(ip);
        assert_eq!(next, Ipv4Addr::new(10, 42, 0, 101));
    }
}
