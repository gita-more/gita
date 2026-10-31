//! 网络相关类型

use crate::ids::{NetworkId, NodeId};
use serde::{Deserialize, Serialize};

/// 网络提供者
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetProvider {
    /// Linux bridge + dnsmasq DHCP（默认，最简单）
    Bridge,
    /// MacVTap（性能较好，需内核支持）
    MacVTap,
    /// QEMU passt（无需 root，用户态网络）
    Passt,
    /// 外部 CNI（预留）
    Cni,
}

/// 网络定义
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Network {
    pub id: NetworkId,
    pub name: String,
    pub provider: NetProvider,
    /// 宿主机网桥/接口名
    pub bridge: String,
    /// CIDR，如 "10.42.0.0/24"
    pub cidr: String,
    /// 网关
    pub gateway: String,
    /// DHCP 范围起止
    pub dhcp_start: Option<String>,
    pub dhcp_end: Option<String>,
    /// MTU
    pub mtu: u32,
    /// VLAN ID（可选）
    #[serde(default)]
    pub vlan: Option<u16>,
    /// 资源归属
    pub owner: crate::ids::OwnerRef,
    /// 是否为默认网络
    pub is_default: bool,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
}

/// IP 租约
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IpLease {
    pub id: i64,
    pub network_id: NetworkId,
    pub ip: String,
    pub mac: String,
    pub vm_id: Option<crate::ids::VmId>,
    pub node_id: Option<NodeId>,
    pub state: LeaseState,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

/// 租约状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Allocated,
    Reserved,
    Released,
    Expired,
}

/// 网络挂载描述（VM Spec 中引用）
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NetworkAttachment {
    /// 接入哪个逻辑网络（网络 ID 或名字）
    pub network: String,
    /// 静态 IP；为空则由 IPAM 分配
    #[serde(default)]
    pub ip: Option<String>,
    /// MAC 地址；为空则由系统生成
    #[serde(default)]
    pub mac: Option<String>,
    /// 子网掩码（点分十进制）；由控制面根据网络 CIDR 填好
    #[serde(default)]
    pub netmask: Option<String>,
    /// 网关；由控制面根据网络定义填好
    #[serde(default)]
    pub gateway: Option<String>,
    /// 宿主机侧的网桥设备名；由控制面根据网络定义填好
    #[serde(default)]
    pub bridge: Option<String>,
    /// MTU；为空则用网络定义的默认值
    #[serde(default)]
    pub mtu: Option<u32>,
}

impl NetworkAttachment {
    pub fn new(network: impl Into<String>) -> Self {
        Self { network: network.into(), ..Default::default() }
    }

    /// 设置静态 IP
    pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
        self.ip = Some(ip.into());
        self
    }

    /// 设置 MAC 地址
    pub fn with_mac(mut self, mac: impl Into<String>) -> Self {
        self.mac = Some(mac.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_attachment_builder() {
        let na = NetworkAttachment::new("ci-bridge")
            .with_ip("10.42.0.100")
            .with_mac("52:54:00:12:34:56");
        assert_eq!(na.network, "ci-bridge");
        assert_eq!(na.ip, Some("10.42.0.100".into()));
        assert_eq!(na.mac, Some("52:54:00:12:34:56".into()));
    }
}