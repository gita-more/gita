//! # gita-vm-store
//!
//! PostgreSQL 存储实现，基于 `gita-db` 的 `PgPool`。
//! 实现 `gita_vm_core::repository::Repository` trait。

pub mod models;
pub mod repo;

pub use repo::VmStore;

/// 运行数据库迁移
pub async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}