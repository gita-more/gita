//! IP 地址管理（IPAM）
//!
//! 简易实现：基于数据库租约表的 DHCP 分配器
//! - 网络定义 CIDR + DHCP 范围
//! - 分配时在可用范围内找第一个未占用 IP
//! - 释放时标记租约过期

use gita_vm_api::network::{IpLease, LeaseState, Network};
use gita_vm_api::ids::VmId;
use crate::repository::Repository;
use crate::Result;
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
    pub async fn allocate(&self, vm_id: VmId, node_id: Option<i64>) -> Result<(String, String)> {
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
        self.repo.create_ip_lease(&lease).await?;
        Ok((ip, mac))
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

        let start = self.network.dhcp_start.as_ref().ok_or_else(|| {
            crate::Error::Validation("网络未配置 DHCP 范围".into())
        })?;
        let end = self.network.dhcp_end.as_ref().ok_or_else(|| {
            crate::Error::Validation("网络未配置 DHCP 范围".into())
        })?;

        let start_ip = Ipv4Addr::from_str(start).map_err(|e| {
            crate::Error::Validation(format!("DHCP start IP 无效: {}", e))
        })?;
        let end_ip = Ipv4Addr::from_str(end).map_err(|e| {
            crate::Error::Validation(format!("DHCP end IP 无效: {}", e))
        })?;

        let mut ip = start_ip;
        while ip <= end_ip {
            if !used.contains(&ip) {
                let mac = generate_mac(ip);
                return Ok((ip.to_string(), mac));
            }
            ip = next_ip(ip);
        }

        Err(crate::Error::IpPoolExhausted(
            "DHCP 范围内无可用 IP".into(),
        ))
    }

    fn check_in_dhcp_range(&self, ip: &Ipv4Addr) -> Result<()> {
        let start = self.network.dhcp_start.as_ref().ok_or_else(|| {
            crate::Error::Validation("网络未配置 DHCP 范围".into())
        })?;
        let end = self.network.dhcp_end.as_ref().ok_or_else(|| {
            crate::Error::Validation("网络未配置 DHCP 范围".into())
        })?;
        let start_ip = Ipv4Addr::from_str(start)
            .map_err(|e| crate::Error::Validation(e.to_string()))?;
        let end_ip = Ipv4Addr::from_str(end)
            .map_err(|e| crate::Error::Validation(e.to_string()))?;
        if *ip < start_ip || *ip > end_ip {
            return Err(crate::Error::Validation("IP 不在 DHCP 范围内".into()));
        }
        Ok(())
    }
}

/// 生成 MAC 地址：基于 IP 的后 3 字节 + 固定前缀
fn generate_mac(ip: Ipv4Addr) -> String {
    let octets = ip.octets();
    format!("52:54:{:02x}:{:02x}:{:02x}:{:02x}", 0x00, octets[1], octets[2], octets[3])
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
    use gita_vm_api::network::{IpLease, LeaseState, NetProvider, Network};
    use gita_vm_api::ids::{NetworkId, VmId};

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
    }

    impl MockRepo {
        fn new() -> Self {
            Self { leases: std::sync::Mutex::new(vec![]) }
        }
    }

    #[async_trait::async_trait]
    impl crate::repository::Repository for MockRepo {
        async fn create_ip_lease(&self, lease: &IpLease) -> crate::Result<i64> {
            let mut l = lease.clone();
            let id = {
                let mut leases = self.leases.lock().unwrap();
                l.id = leases.len() as i64 + 1;
                leases.push(l.clone());
                l.id
            };
            Ok(id)
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
            Ok(leases.iter().filter(|l| l.network_id == network_id).cloned().collect())
        }

        // 其余方法存根
        async fn create_vm(&self, _: &gita_vm_api::Vm) -> crate::Result<()> { Ok(()) }
        async fn get_vm(&self, _: VmId) -> crate::Result<Option<gita_vm_api::Vm>> { Ok(None) }
        async fn update_vm(&self, _: &gita_vm_api::Vm) -> crate::Result<()> { Ok(()) }
        async fn delete_vm(&self, _: VmId) -> crate::Result<()> { Ok(()) }
        async fn list_vms(&self, _: Option<gita_vm_api::ids::OwnerRef>) -> crate::Result<Vec<gita_vm_api::Vm>> { Ok(vec![]) }
        async fn create_template(&self, _: &gita_vm_api::VmTemplate) -> crate::Result<()> { Ok(()) }
        async fn get_template(&self, _: gita_vm_api::TemplateId) -> crate::Result<Option<gita_vm_api::VmTemplate>> { Ok(None) }
        async fn update_template(&self, _: &gita_vm_api::VmTemplate) -> crate::Result<()> { Ok(()) }
        async fn delete_template(&self, _: gita_vm_api::TemplateId) -> crate::Result<()> { Ok(()) }
        async fn list_templates(&self, _: Option<gita_vm_api::ids::OwnerRef>) -> crate::Result<Vec<gita_vm_api::VmTemplate>> { Ok(vec![]) }
        async fn create_node(&self, _: &gita_vm_api::Node) -> crate::Result<()> { Ok(()) }
        async fn get_node(&self, _: gita_vm_api::NodeId) -> crate::Result<Option<gita_vm_api::Node>> { Ok(None) }
        async fn update_node(&self, _: &gita_vm_api::Node) -> crate::Result<()> { Ok(()) }
        async fn delete_node(&self, _: gita_vm_api::NodeId) -> crate::Result<()> { Ok(()) }
        async fn list_nodes(&self, _: Option<gita_vm_api::ids::OwnerRef>) -> crate::Result<Vec<gita_vm_api::Node>> { Ok(vec![]) }
        async fn create_image(&self, _: &gita_vm_api::Image) -> crate::Result<()> { Ok(()) }
        async fn get_image(&self, _: gita_vm_api::ImageId) -> crate::Result<Option<gita_vm_api::Image>> { Ok(None) }
        async fn update_image(&self, _: &gita_vm_api::Image) -> crate::Result<()> { Ok(()) }
        async fn delete_image(&self, _: gita_vm_api::ImageId) -> crate::Result<()> { Ok(()) }
        async fn list_images(&self, _: Option<gita_vm_api::ids::OwnerRef>) -> crate::Result<Vec<gita_vm_api::Image>> { Ok(vec![]) }
        async fn create_operation(&self, _: &gita_vm_api::Operation) -> crate::Result<()> { Ok(()) }
        async fn get_operation(&self, _: gita_vm_api::ids::OperationId) -> crate::Result<Option<gita_vm_api::Operation>> { Ok(None) }
        async fn update_operation(&self, _: &gita_vm_api::Operation) -> crate::Result<()> { Ok(()) }
        async fn list_operations(&self, _: Option<gita_vm_api::NodeId>) -> crate::Result<Vec<gita_vm_api::Operation>> { Ok(vec![]) }
        async fn create_quota(&self, _: &gita_vm_api::Quota) -> crate::Result<()> { Ok(()) }
        async fn get_quota(&self, _: gita_vm_api::quota::QuotaScope, _: i64) -> crate::Result<Option<gita_vm_api::Quota>> { Ok(None) }
        async fn update_quota(&self, _: &gita_vm_api::Quota) -> crate::Result<()> { Ok(()) }
        async fn delete_quota(&self, _: gita_vm_api::quota::QuotaScope, _: i64) -> crate::Result<()> { Ok(()) }
        async fn list_quotas(&self) -> crate::Result<Vec<gita_vm_api::Quota>> { Ok(vec![]) }
        async fn get_quota_usage(&self, _: gita_vm_api::quota::QuotaScope, _: i64) -> crate::Result<gita_vm_api::QuotaUsage> { Ok(Default::default()) }
        async fn append_event(&self, _: &gita_vm_api::Event) -> crate::Result<()> { Ok(()) }
        async fn list_events(&self, _: Option<i64>, _: Option<&str>, _: i64, _: i64) -> crate::Result<Vec<gita_vm_api::Event>> { Ok(vec![]) }
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

        ipam.reserve("10.42.0.100", "52:54:00:2a:00:64").await.unwrap();

        let (ip, _) = ipam.allocate(VmId(1), None).await.unwrap();
        assert_eq!(ip, "10.42.0.101", "the reserved address must be skipped");
    }

    /// The same address cannot be reserved twice.
    #[tokio::test]
    async fn an_address_cannot_be_reserved_twice() {
        let net = test_network();
        let repo = MockRepo::new();
        let ipam = Ipam::new(&repo, &net);

        ipam.reserve("10.42.0.100", "52:54:00:2a:00:64").await.unwrap();
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
        assert!(matches!(err, crate::Error::IpPoolExhausted(_)), "got {err:?}");
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