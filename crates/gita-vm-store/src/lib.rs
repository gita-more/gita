//! # gita-vm-store
//!
//! PostgreSQL 存储实现，基于 `gita-db` 的 `PgPool`。
//! 实现 `gita_vm_core::repository::Repository` trait。
//!
//! VM 的表结构在 `gita-db` 的迁移里（`20261031000000_vm_init.sql`），
//! 整个数据库只有一份迁移历史——两个 crate 各自跑 `sqlx::migrate!` 会
//! 共用 `_sqlx_migrations` 表并互相把对方的迁移判成「已应用但不存在」。

pub mod models;
pub mod repo;

pub use repo::VmStore;