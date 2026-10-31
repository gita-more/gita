//! # gita-vm-node
//!
//! 节点代理：部署在 hypervisor 上，长轮询控制面拉取操作，通过 VMM 驱动执行。

pub mod agent;
pub mod config;

pub use agent::NodeAgent;
pub use config::NodeConfig;