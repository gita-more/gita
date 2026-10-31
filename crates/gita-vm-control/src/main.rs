//! gita-vm-control 独立运行入口（可选；通常嵌入 gita-server）

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "gita-vm-control", version, about = "Gita VM control plane (standalone mode)")]
pub struct Args {
    /// PostgreSQL 连接串
    #[arg(long, env = "GITA_DATABASE_URL", default_value = "postgres://gita:gita@localhost:5432/gita")]
    pub database_url: String,

    /// 监听地址
    #[arg(long, default_value = "127.0.0.1:3001")]
    pub listen: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    let db = std::sync::Arc::new(gita_db::Db::connect(&args.database_url).await?);
    let store = std::sync::Arc::new(gita_vm_store::VmStore::new(db.pool().clone()));
    let state = gita_vm_control::AppState::new(
        store,
        gita_vm_control::ControlConfig::default(),
    );

    // 启动后台循环
    let loops = gita_vm_control::loops::start_loops(state.clone());
    tracing::info!("started {} background loops", loops.len());

    let router = gita_vm_control::build_router(state);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(%args.listen, "gita-vm-control listening");
    axum::serve(listener, router).await?;
    Ok(())
}
