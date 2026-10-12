//! Gita CLI 集成测试
//!
//! 通过 `assert_cmd` 调用 `gita-server` 二进制，验证命令行行为；
//! ��时直接使用 `gita_server` 库验证配置加载与应用初始化。

use assert_cmd::Command;
use gita_server::{Args, App, Config};
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// 二进制 CLI 行为
// ---------------------------------------------------------------------------

#[test]
fn help_flag_shows_usage() {
    let mut cmd = Command::cargo_bin("gita-server").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("--config"));
}

#[test]
fn version_flag_shows_version() {
    let mut cmd = Command::cargo_bin("gita-server").unwrap();
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

#[tokio::test]
async fn app_build_creates_directories_and_router() {
    let tmp = TempDir::new().unwrap();
    let mut config = Config::default();
    config.server.data_dir = tmp.path().join("data");
    config.server.database_url = format!("sqlite://{}", tmp.path().join("data/gita.db").display());

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