//! gita-vm-node 独立运行入口

use clap::Parser;
use gita_vm_node::agent::NodeAgent;
use gita_vm_node::config::NodeConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = NodeConfig::parse();
    let mut agent = NodeAgent::new(config);

    // 注册失败时重试
    for attempt in 1..=5 {
        match agent.register().await {
            Ok(id) => {
                tracing::info!(node_id = id, "registered");
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "register failed, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(5 * attempt)).await;
                if attempt == 5 {
                    return Err(e);
                }
            }
        }
    }

    agent.run().await
}