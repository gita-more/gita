//! Gita CLI 集成测试
//!
//! 通过 `assert_cmd` 调用 `gita-server` 二进制，验证命令行行为；
//! ��时直接使用 `gita_server` 库验证配置加载与应用初始化。

use assert_cmd::Command;
use gita_server::{App, Args, Config};
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

/// Path to the gita-server binary. The standard `assert_cmd::cargo_bin`
/// relies on `CARGO_BIN_EXE_<name>` being set when cargo builds an
/// integration test, but we bind the binary through cargo's normal bin
/// discovery (in `crates/gita-server`), not through the integration
/// crate, so the env var is unset. Resolve the path relative to the
/// workspace's `target/debug/` directory instead.
fn gita_server_bin() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("target")
        .join("debug")
        .join("gita-server")
}

// ---------------------------------------------------------------------------
// 二进制 CLI 行为
// ---------------------------------------------------------------------------

#[test]
fn help_flag_shows_usage() {
    let mut cmd = Command::new(gita_server_bin());
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("--config"));
}

#[test]
fn version_flag_shows_version() {
    let mut cmd = Command::new(gita_server_bin());
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("gita"))
        .stdout(predicate::str::contains("0.0.1"));
}

#[test]
fn missing_config_falls_back_to_defaults() {
    // 不存在的配置文件不应导致启动失败（Config::load 会 fallback 到默认值）。
    let missing = PathBuf::from("/nonexistent/gita.toml");
    let config = Config::load(&missing).unwrap();
    assert_eq!(config.server.port, 3000);
    assert_eq!(config.server.host, "127.0.0.1");
}

// ---------------------------------------------------------------------------
// 应用初始化（不监听端口，仅验证装配）
// ---------------------------------------------------------------------------

/// Database URL for integration tests. Override via `GITA_TEST_DATABASE_URL`.
fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

#[tokio::test]
async fn app_build_creates_directories_and_router() {
    let tmp = TempDir::new().unwrap();
    let mut config = Config::default();
    config.server.data_dir = tmp.path().join("data");
    // Connect to the bundled PG instance. init_schema is idempotent across
    // runs (CREATE TABLE IF NOT EXISTS), so concurrent test runs share the
    // default `public` schema safely.
    config.server.database_url = test_db_url();

    let app = App::build(config).await.unwrap();
    // 数据目录与仓库目录应被创建
    assert!(tmp.path().join("data").is_dir());
    assert!(tmp.path().join("data/repos").is_dir());
    // 路由应可组装
    let _router = app.router();
}

// ---------------------------------------------------------------------------
// Args 解析
// ---------------------------------------------------------------------------

#[test]
fn args_parse_config_path() {
    use clap::Parser;
    let args = Args::parse_from(["gita", "--config", "custom.toml"]);
    assert_eq!(args.config, PathBuf::from("custom.toml"));
}
