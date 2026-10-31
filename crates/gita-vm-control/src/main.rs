//! gita-vm-control 独立运行入口
//!
//! 通常 VM 控制面是嵌进 gita-server 的（`gita-admin serve`），这个二进制
//! 用于把控制面单独跑起来——比如控制面与节点分处不同网络，或者调试时
//! 不想重启主服务。

use clap::Parser;
use gita_api::auth::Auth;
use gita_vm_control::auth::VmAuth;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "gita-vm-control", version, about = "Gita VM control plane (standalone mode)")]
pub struct Args {
    /// PostgreSQL 连接串
    #[arg(long, env = "GITA_DATABASE_URL", default_value = "postgres://gita:gita@localhost:5432/gita")]
    pub database_url: String,

    /// 监听地址
    #[arg(long, default_value = "127.0.0.1:3001")]
    pub listen: String,

    /// JWT 签名密钥（必须与 gita-server 一致，否则用户令牌无法校验）
    #[arg(long, env = "GITA_JWT_SECRET", default_value = "dev-secret-change-me")]
    pub jwt_secret: String,

    /// 节点代理使用的 API key（访问 /internal/* 接口）
    #[arg(long, env = "GITA_VM_API_KEY", default_value = "")]
    pub vm_api_key: String,
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

    // 鉴权是必需的：控制面永远带鉴权运行。
    let auth = VmAuth::new(
        (*db).clone(),
        Arc::new(Auth::new(args.jwt_secret.clone())),
        Some(args.vm_api_key.clone()),
    );

    let state = gita_vm_control::AppState::new(
        store,
        gita_vm_control::ControlConfig::default(),
        auth,
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
