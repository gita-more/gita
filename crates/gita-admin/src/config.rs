//! Server configuration types, TOML loading/merging, and defaults.

use anyhow::{Context, Result};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use tracing::warn;

/// CLI arguments for gita-admin.
#[derive(Parser, Debug)]
#[command(
    name = "gita-admin",
    version,
    about = "Gita Admin - server & CI infrastructure"
)]
pub struct Args {
    #[arg(short = 'c', long, global = true, default_value = "gita.toml")]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the Gita server (includes CI runner background task)
    Serve,
    /// Worker management
    Worker {
        #[command(subcommand)]
        action: WorkerCommand,
    },
    /// CI image management
    Image {
        #[command(subcommand)]
        cmd: ImageCmd,
    },
    /// VM template management
    Template {
        #[command(subcommand)]
        cmd: TemplateCmd,
    },
    /// Runner management
    Runner {
        #[command(subcommand)]
        cmd: RunnerCmd,
    },
    /// Administrator user management
    Users {
        #[command(subcommand)]
        cmd: crate::users::UsersCmd,
    },
    /// VM cluster management (templates, nodes, instances)
    Vm {
        #[command(subcommand)]
        cmd: VmCmd,
    },
}

/// VM cluster subcommands.
///
/// The commands talk to the gita server's own VM control plane
/// (`/api/v1/vm/*`), so they work against a server that has
/// `[server.vm] enabled` — the same server that runs the CI jobs.
#[derive(Subcommand, Debug)]
pub enum VmCmd {
    /// List VM templates (what `vm:` blocks in .gita-ci.yml reference)
    Templates {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
    },
    /// Create a VM template
    Create {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
        /// Template name (referenced by `vm: template:` in .gita-ci.yml)
        #[arg(long)]
        name: String,
        /// Base image name (as registered in the image catalog)
        #[arg(long)]
        image: String,
        /// CPU cores
        #[arg(long, default_value_t = 2)]
        cpu: u64,
        /// Memory, e.g. 4G
        #[arg(long, default_value = "4G")]
        memory: String,
        /// Disk size, e.g. 20G
        #[arg(long, default_value = "20G")]
        disk: String,
        /// VMM backend (qemu only for now)
        #[arg(long, default_value = "qemu")]
        backend: String,
        /// Human-readable description
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Delete a VM template
    DeleteTemplate {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
        /// Template id
        #[arg(long)]
        id: i64,
    },
    /// Print the VM control plane's node API key (generated server-side
    /// when the deployment has not configured one). Pipe it into
    /// `gita-vm-node --api-key`.
    ApiKey {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
    },
    /// List cluster nodes (hypervisors)
    Nodes {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
    },
    /// List virtual machine instances
    Instances {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
    },
    /// Reclaim (delete) a virtual machine instance
    Reclaim {
        /// gita server URL
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        server_url: String,
        /// Access token (PAT or JWT) of an administrator
        #[arg(long, env = "GITA_TOKEN")]
        token: Option<String>,
        /// Instance id
        #[arg(long)]
        id: i64,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorkerCommand {
    /// Register a new runner and print its token
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        user: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImageCmd {
    /// Build a base CI image (requires root)
    Build {
        #[arg(long)]
        output: String,
        #[arg(long, default_value = "8G")]
        size: String,
        #[arg(long, default_value = "noble")]
        ubuntu_release: String,
        #[arg(long)]
        agent_script: Option<String>,
    },
    /// Register a built image to rcluster
    Register {
        #[arg(long)]
        control_url: String,
        #[arg(long)]
        pool: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "x86_64")]
        arch: String,
        /// rcluster API key (defaults to $RCLUSTER_API_KEY)
        #[arg(long)]
        api_key: Option<String>,
    },
    /// List registered images
    List {
        #[arg(long)]
        control_url: String,
        /// rcluster API key (defaults to $RCLUSTER_API_KEY)
        #[arg(long)]
        api_key: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplateCmd {
    Create {
        #[arg(long)]
        control_url: String,
        /// rcluster API key (defaults to $RCLUSTER_API_KEY)
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        image: String,
        /// 内核镜像名；缺省使用平台默认内核。
        #[arg(long)]
        kernel: Option<String>,
        #[arg(long)]
        template: String,
        #[arg(long, default_value = "4000")]
        cpu: u64,
        #[arg(long, default_value = "4G")]
        memory: String,
        #[arg(long, default_value = "20G")]
        disk: String,
        #[arg(long)]
        network: String,
    },
    List {
        #[arg(long)]
        control_url: String,
        /// rcluster API key (defaults to $RCLUSTER_API_KEY)
        #[arg(long)]
        api_key: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RunnerCmd {
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        user: String,
    },
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_db_url")]
    pub database_url: String,
    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,
    /// Optional Postgres schema to pin `search_path` to. Used by tests for
    /// per-test schema isolation.
    #[serde(default)]
    pub database_schema: Option<String>,
    /// Embedded SSH server configuration (Git over SSH).
    #[serde(default)]
    pub ssh: SshConfig,
    /// Master switch for GitHub-Pages-style site serving. When false, the
    /// `/{owner}/{repo}/{*path}` routes are not registered at all and every
    /// request falls through to the SPA.
    #[serde(default = "default_pages_enabled")]
    pub pages_enabled: bool,
    /// Optional URL prefix for Pages sites, e.g. "/p" serves /p/owner/repo/.
    /// Empty (the default) serves at /owner/repo/, mirroring GitHub project
    /// sites.
    #[serde(default)]
    pub pages_url_prefix: String,
    /// Return the password-reset token in the API response.
    ///
    /// Off by default: the response is uniform for existing and
    /// non-existent accounts, so returning the token would let an
    /// unauthenticated caller enumerate accounts *and* take them over. Turn
    /// it on only for local development or a deployment with no mail
    /// transport, where an operator reads the token out of the response.
    #[serde(default)]
    pub allow_insecure_password_reset: bool,
    /// VM 集群管理（控制面 + 节点 + CI 调度）。
    #[serde(default)]
    pub vm: VmConfig,
}

/// Configuration for the embedded SSH server (Git over SSH).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    /// Whether to start the SSH listener alongside HTTP.
    #[serde(default = "default_ssh_enabled")]
    pub enabled: bool,
    /// Bind address for the SSH listener.
    #[serde(default = "default_ssh_host")]
    pub host: String,
    /// Public host/hostname that clients use to connect via SSH.
    /// If not set, falls back to `host` (but `0.0.0.0` is replaced with `localhost`).
    #[serde(default)]
    pub public_host: Option<String>,
    /// Port for the SSH listener.
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Path to the OpenSSH-format host private key. Defaults to
    /// `<data_dir>/ssh_host_ed25519_key` and is auto-generated on first run.
    #[serde(default)]
    pub host_key: Option<PathBuf>,
}

fn default_ssh_enabled() -> bool {
    false
}
fn default_ssh_host() -> String {
    default_host()
}
fn default_ssh_port() -> u16 {
    2222
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: default_ssh_enabled(),
            host: default_ssh_host(),
            public_host: None,
            port: default_ssh_port(),
            host_key: None,
        }
    }
}

fn default_pages_enabled() -> bool {
    true
}

/// VM 集群管理配置。
///
/// 关闭时（默认）gita 不启动 VM 控制面，`/api/v1/vm/*` 路由不注册。
/// 打开后 gita 自己就是 VM 集群的控制面：CI 任务可以直接调度到集群的
/// 虚拟机上跑，不再依赖外部的集群管理系统。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// 总开关。关掉时既不注册路由也不启动后台循环。
    #[serde(default = "default_vm_enabled")]
    pub enabled: bool,
    /// VM 数据目录（镜像、快照、节点工作目录）。
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// 节点代理使用的 API key（访问 /api/v1/vm/internal/* ）。
    ///
    /// 显式配置的值优先；留空（或未配置）时控制面第一次启动自动
    /// 生成强随机值并持久化到 `vm_settings`，重启不变。管理员用
    /// `gita-admin vm api-key` 把它读出来交给 `gita-vm-node`。
    #[serde(default)]
    pub api_key: Option<String>,
    /// reconcile 循环间隔（秒）：把待创建的 VM 调度到节点上。
    #[serde(default = "default_vm_reconcile_interval")]
    pub reconcile_interval_secs: u64,
    /// 节点负载超过该比例就不再往这个节点调度。
    #[serde(default = "default_vm_overload_ratio")]
    pub overload_ratio: f64,
    /// 节点心跳超时（秒）：超过这个时间没有心跳就判定节点下线。
    #[serde(default = "default_vm_heartbeat_timeout")]
    pub heartbeat_timeout_secs: u64,
    /// 让 workflow job 跑在集群 VM 里，而不是 runner 宿主机上。
    ///
    /// 对应 runner 的 `GITA_WORKFLOW_VM`。需要 `enabled = true` 且至少
    /// 有一个已注册的节点；没有节点时 job 会因为找不到模板而失败。
    #[serde(default)]
    pub workflow_vm: bool,
}

fn default_vm_enabled() -> bool {
    false
}

fn default_vm_reconcile_interval() -> u64 {
    5
}

fn default_vm_overload_ratio() -> f64 {
    0.8
}

fn default_vm_heartbeat_timeout() -> u64 {
    30
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            data_dir: None,
            api_key: None,
            reconcile_interval_secs: default_vm_reconcile_interval(),
            overload_ratio: default_vm_overload_ratio(),
            heartbeat_timeout_secs: default_vm_heartbeat_timeout(),
            workflow_vm: false,
        }
    }
}

pub(crate) const REGISTER_RATE_PER_SECOND: u32 = 1;
pub(crate) const REGISTER_RATE_BURST: u32 = 3;

/// Per-IP password-reset rate limit: one request per second, with a burst of
/// three — the same shape as registration.
///
/// The reset endpoints are unauthenticated. A request for an existing
/// account writes a token row and a log line, and confirming a valid token
/// runs an Argon2 hash, so both are worth throttling. The burst of three
/// lets a user who mistyped their identifier retry without waiting.
pub(crate) const PASSWORD_RESET_RATE_PER_SECOND: u32 = 1;
pub(crate) const PASSWORD_RESET_RATE_BURST: u32 = 3;

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            data_dir: default_data_dir(),
            database_url: default_db_url(),
            jwt_secret: default_jwt_secret(),
            database_schema: None,
            ssh: SshConfig::default(),
            pages_enabled: default_pages_enabled(),
            pages_url_prefix: String::new(),
            allow_insecure_password_reset: false,
            vm: VmConfig::default(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    3000
}
fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}
fn default_db_url() -> String {
    "postgres://gita:gita@localhost:5432/gita".to_string()
}
fn default_jwt_secret() -> String {
    "dev-secret-change-me".to_string()
}

impl Config {
    /// Load config from a TOML file, merging defaults.
    pub fn load(path: &PathBuf) -> Result<Self> {
        let mut config = Config::default();
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read config file {}", path.display()))?;
            let parsed: Config = toml::from_str(&text)
                .with_context(|| format!("parse config file {}", path.display()))?;
            // Merge fields.
            if !parsed.server.host.is_empty() {
                config.server.host = parsed.server.host;
            }
            if parsed.server.port != 0 {
                config.server.port = parsed.server.port;
            }
            if !parsed.server.data_dir.as_os_str().is_empty() {
                config.server.data_dir = parsed.server.data_dir;
            }
            if !parsed.server.database_url.is_empty() {
                config.server.database_url = parsed.server.database_url;
            }
            if !parsed.server.jwt_secret.is_empty()
                && parsed.server.jwt_secret != default_jwt_secret()
            {
                config.server.jwt_secret = parsed.server.jwt_secret;
            }
            // SSH section (only when present in the file).
            // NOTE: this pattern is asymmetric — a boolean can only ever be
            // turned *on* by the file, never off. Pages uses presence-based
            // merging below instead; keep this block as-is to avoid churn.
            if parsed.server.ssh.enabled {
                config.server.ssh.enabled = true;
            }
            if !parsed.server.ssh.host.is_empty() {
                config.server.ssh.host = parsed.server.ssh.host;
            }
            if parsed.server.ssh.port != 0 {
                config.server.ssh.port = parsed.server.ssh.port;
            }
            if parsed.server.ssh.host_key.is_some() {
                config.server.ssh.host_key = parsed.server.ssh.host_key;
            }
            if parsed.server.ssh.public_host.is_some() {
                config.server.ssh.public_host = parsed.server.ssh.public_host;
            }

            // Pages section. Unlike SSH, these fields must be switchable in
            // both directions, so check key *presence* in the raw TOML rather
            // than the defaulted parsed struct (which cannot distinguish
            // "absent" from "explicitly false"/"empty").
            let raw: toml::Value = match toml::from_str(&text) {
                Ok(v) => v,
                // The same text already parsed successfully above, so this
                // can only fail if the file changed between reads.
                Err(_) => toml::Value::Table(Default::default()),
            };
            let raw_server = raw.get("server");
            // Schema pinning. Presence-based like the Pages flags: the
            // merge loop above works on defaulted values, and `None` is
            // the default here, so reading it from the parsed struct
            // could never distinguish "absent" from "explicitly unset".
            //
            // This used to be missing entirely: `App::build` honours
            // `database_schema` (that is how the schema-isolated integration
            // tests work), but `Config::load` — the path the real binary takes
            // with `-c gita.toml` — never merged the field, so a server
            // started with `-c gita.toml` always landed in `public`, whatever
            // the file said.
            if let Some(v) = raw_server
                .and_then(|s| s.get("database_schema"))
                .and_then(|v| v.as_str())
            {
                config.server.database_schema = Some(v.to_string());
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("pages_enabled"))
                .and_then(|v| v.as_bool())
            {
                config.server.pages_enabled = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("pages_url_prefix"))
                .and_then(|v| v.as_str())
            {
                config.server.pages_url_prefix = v.to_string();
            }
            // Password reset. Like the Pages flags, this must be switchable
            // in both directions, so it is read by key presence rather than
            // from the defaulted parsed struct.
            if let Some(v) = raw_server
                .and_then(|s| s.get("allow_insecure_password_reset"))
                .and_then(|v| v.as_bool())
            {
                config.server.allow_insecure_password_reset = v;
            }

            // VM 集群配置。同样的坑：`enabled` 必须按 key 是否存在来读，
            // 默认值是 false，合并 defaulted 结构体无法区分「没写」和
            // 「显式写成 false」。
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
            {
                config.server.vm.enabled = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("data_dir"))
                .and_then(|v| v.as_str())
            {
                config.server.vm.data_dir = Some(PathBuf::from(v));
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str())
            {
                config.server.vm.api_key = Some(v.to_string());
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("reconcile_interval_secs"))
                .and_then(|v| v.as_integer().map(|i| i as u64))
            {
                config.server.vm.reconcile_interval_secs = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("overload_ratio"))
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            {
                config.server.vm.overload_ratio = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("heartbeat_timeout_secs"))
                .and_then(|v| v.as_integer().map(|i| i as u64))
            {
                config.server.vm.heartbeat_timeout_secs = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("vm"))
                .and_then(|v| v.get("workflow_vm"))
                .and_then(|v| v.as_bool())
            {
                config.server.vm.workflow_vm = v;
            }
        } else {
            warn!("config file {} not found, using defaults", path.display());
        }
        Ok(config)
    }
}

/// Parse a size string like "4G" or "20G" into bytes.
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty size string");
    }
    let (num_str, unit) = if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024)
    } else {
        (s, 1)
    };
    let num: u64 = num_str.parse().context("invalid size number")?;
    Ok(num * unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign_webhook_payload;

    #[test]
    fn config_load_missing_file_falls_back_to_defaults() {
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
        assert!(config.server.database_url.contains("postgres://"));
    }

    #[test]
    fn config_load_merges_toml_values() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server]
            host = "0.0.0.0"
            port = 9000
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        // Non-overridden fields keep defaults.
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
        assert!(config.server.database_url.contains("postgres://"));
    }

    /// The `[server.vm]` keys must survive `Config::load`.
    ///
    /// That merge is hand-written (the TOML is walked key by key), so a new
    /// field is silently dropped unless someone adds a branch for it — and
    /// a dropped key looks exactly like "the operator left it at the
    /// default".
    #[test]
    fn config_load_merges_every_vm_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server.vm]
            enabled = true
            workflow_vm = true
            reconcile_interval_secs = 11
            overload_ratio = 0.5
            heartbeat_timeout_secs = 22
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.vm.enabled);
        assert!(
            config.server.vm.workflow_vm,
            "workflow_vm must reach the runner"
        );
        assert_eq!(config.server.vm.reconcile_interval_secs, 11);
        assert_eq!(config.server.vm.overload_ratio, 0.5);
        assert_eq!(config.server.vm.heartbeat_timeout_secs, 22);
    }

    #[test]
    fn config_does_not_override_jwt_secret_with_default_value() {
        // If the TOML literally sets jwt_secret to the default string,
        // Config::load must NOT treat that as an override (it skips it).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(&path, "[server]\njwt_secret = \"dev-secret-change-me\"\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
    }

    /// A config *file* that pins a schema must reach the running server.
    ///
    /// `App::build` has always honoured `database_schema` (that is how the
    /// schema-isolated integration tests work), but `Config::load` — the
    /// path the real binary takes with `-c gita.toml` — never merged the
    /// field, so the file's value was silently dropped and the server
    /// landed in `public`.
    #[test]
    fn config_load_merges_database_schema() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            "[server]\ndatabase_schema = \"gita_shutdown_test\"\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.server.database_schema.as_deref(),
            Some("gita_shutdown_test")
        );

        // Absent in the file → stays None.
        let path = dir.path().join("empty.toml");
        std::fs::write(&path, "[server]\nport = 3000\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.database_schema.is_none());
    }

    #[test]
    fn config_load_merges_ssh_section() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server]
            host = "0.0.0.0"

            [server.ssh]
            enabled = true
            host = "0.0.0.0"
            port = 2224
            host_key = "/tmp/custom_host_key"
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.ssh.enabled);
        assert_eq!(config.server.ssh.host, "0.0.0.0");
        assert_eq!(config.server.ssh.port, 2224);
        assert_eq!(
            config.server.ssh.host_key.as_deref(),
            Some(std::path::Path::new("/tmp/custom_host_key"))
        );
        // Defaults are preserved for fields not present in the file.
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn config_ssh_defaults_are_disabled() {
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert!(!config.server.ssh.enabled);
        assert_eq!(config.server.ssh.port, 2222);
        assert!(config.server.ssh.host_key.is_none());
    }

    #[test]
    fn config_pages_enabled_defaults_on_and_can_be_turned_off() {
        // Missing file -> default on.
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert!(config.server.pages_enabled);
        assert_eq!(config.server.pages_url_prefix, "");

        // Explicitly false -> master switch off (the asymmetric SSH merge
        // pattern would have ignored this; presence-based merging must not).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(&path, "[server]\npages_enabled = false\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(
            !config.server.pages_enabled,
            "pages_enabled=false must be honored"
        );

        // Explicitly true -> on.
        std::fs::write(&path, "[server]\npages_enabled = true\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.pages_enabled);
    }

    #[test]
    fn config_pages_url_prefix_is_loaded() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(&path, "[server]\npages_url_prefix = \"/p\"\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.pages_url_prefix, "/p");
    }

    /// The insecure password-reset switch is off by default and can be
    /// turned on from the config file.
    #[test]
    fn config_allow_insecure_password_reset_defaults_off_and_is_loadable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");

        // Default: off.
        std::fs::write(&path, "[server]\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(
            !config.server.allow_insecure_password_reset,
            "returning the reset token must be opt-in"
        );

        // Explicitly enabled.
        std::fs::write(&path, "[server]\nallow_insecure_password_reset = true\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.allow_insecure_password_reset);

        // Explicitly disabled again — the flag must be switchable in both
        // directions, which is why it is read by key presence.
        std::fs::write(&path, "[server]\nallow_insecure_password_reset = false\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(!config.server.allow_insecure_password_reset);
    }

    /// Load the shipped example config and assert the Pages keys are where the
    /// code reads them.
    ///
    /// Regression test: TOML keys are scoped to the most recent table header,
    /// so placing `pages_enabled` after `[server.ssh]` silently parses it as
    /// `server.ssh.pages_enabled` — leaving `Config::load` to see nothing and
    /// the documented setting quietly inert. This test parses the actual
    /// example file the operator copies, so the two cannot drift apart.
    #[test]
    fn example_config_parses_pages_keys_under_server() {
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.example.toml");
        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|e| panic!("read {}: {e}", example.display()));

        let raw: toml::Value = toml::from_str(&text).expect("example config should be valid TOML");
        let server = raw
            .get("server")
            .expect("example config should have [server]");
        assert!(
            server.get("pages_enabled").is_some(),
            "pages_enabled must live under [server], not [server.ssh] — TOML keys \
             belong to the preceding table header, so a misplaced key is silently ignored"
        );

        // And it must survive the real loader with the documented value.
        let config = Config::load(&example).unwrap();
        assert!(
            config.server.pages_enabled,
            "example config documents pages_enabled = true; loading it must keep Pages on"
        );
        assert_eq!(config.server.pages_url_prefix, "");

        // The password-reset switch has the same scoping requirement. The
        // example documents it commented out, so it must still *parse* as a
        // key of [server] when present — a misplaced one would land under
        // [server.ssh] and be silently ignored.
        let server = raw
            .get("server")
            .expect("example config should have [server]");
        assert!(
            server.get("allow_insecure_password_reset").is_none(),
            "the example must not enable the insecure password reset by default"
        );
    }

    /// The Docker entrypoint generates config.toml from env vars; its Pages
    /// keys must land under `[server]` too.
    ///
    /// Same TOML table-scoping trap as the example config, in a shell script
    /// that no Rust test otherwise touches — the bug is invisible until
    /// someone tries to turn Pages off in a container and it stays on.
    #[test]
    fn docker_entrypoint_generates_pages_keys_under_server() {
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docker/docker-entrypoint.sh");
        let text = std::fs::read_to_string(&script)
            .unwrap_or_else(|e| panic!("read {}: {e}", script.display()));

        // Locate the generated [server] block and the [server.ssh] header.
        let server_at = text
            .find("\n[server]\n")
            .expect("entrypoint should emit [server]");
        let ssh_at = text
            .find("\n[server.ssh]\n")
            .expect("entrypoint should emit [server.ssh]");
        let pages_at = text
            .find("pages_enabled =")
            .expect("entrypoint should emit pages_enabled");
        assert!(
            pages_at > server_at && pages_at < ssh_at,
            "pages_enabled is emitted at offset {pages_at}, which falls outside the \
             [server] table ({server_at}..{ssh_at}) — TOML scopes keys to the most \
             recent table header, so it would be read as server.ssh.pages_enabled \
             and silently ignored"
        );

        // The VM block has the same trap: its keys must land under
        // [server.vm], not [server.ssh].
        let vm_at = text
            .find("\n[server.vm]\n")
            .expect("entrypoint should emit [server.vm]");
        let vm_enabled_at = text
            .find("enabled = ${GITA_SERVER_VM_ENABLED")
            .expect("entrypoint should emit the vm enabled flag");
        assert!(
            vm_enabled_at > vm_at && vm_enabled_at < ssh_at,
            "the vm enabled flag is emitted at offset {vm_enabled_at}, which falls outside \
             the [server.vm] table ({vm_at}..{ssh_at}) — TOML scopes keys to the most \
             recent table header, so it would be read as server.ssh.enabled and silently \
             ignored"
        );

        // The password-reset switch has the same trap.
        let reset_at = text
            .find("allow_insecure_password_reset =")
            .expect("entrypoint should emit allow_insecure_password_reset");
        assert!(
            reset_at > server_at && reset_at < ssh_at,
            "allow_insecure_password_reset is emitted at offset {reset_at}, which falls \
             outside the [server] table ({server_at}..{ssh_at}) — TOML scopes keys to \
             the most recent table header, so it would be read as \
             server.ssh.allow_insecure_password_reset and silently ignored"
        );
    }

    /// The VM section must load from the config file.
    ///
    /// Regression test: the VM block was merged into the defaulted
    /// `ServerConfig` struct, which cannot distinguish "the key was absent"
    /// from "the key was explicitly set to the default value". `enabled`
    /// defaults to `false`, so reading it from the defaulted struct would
    /// make `enabled = true` in the file a no-op — the control plane would
    /// silently never start.
    #[test]
    fn config_load_merges_vm_section() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server]
            host = "0.0.0.0"

            [server.vm]
            enabled = true
            data_dir = "/var/lib/gita/vm"
            api_key = "secret-key"
            reconcile_interval_secs = 10
            overload_ratio = 0.7
            heartbeat_timeout_secs = 45
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.vm.enabled, "vm.enabled must be honored");
        assert_eq!(
            config.server.vm.data_dir,
            Some(PathBuf::from("/var/lib/gita/vm"))
        );
        assert_eq!(config.server.vm.api_key.as_deref(), Some("secret-key"));
        assert_eq!(config.server.vm.reconcile_interval_secs, 10);
        assert!((config.server.vm.overload_ratio - 0.7).abs() < 1e-9);
        assert_eq!(config.server.vm.heartbeat_timeout_secs, 45);
    }

    /// The VM switch must be switchable in **both** directions.
    #[test]
    fn config_vm_enabled_can_be_turned_off_explicitly() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");

        // Absent -> default (off).
        std::fs::write(&path, "[server]\n").unwrap();
        assert!(!Config::load(&path).unwrap().server.vm.enabled);

        // Explicitly on.
        std::fs::write(&path, "[server]\n[server.vm]\nenabled = true\n").unwrap();
        assert!(Config::load(&path).unwrap().server.vm.enabled);

        // Explicitly off again — this is the case that reading the value
        // out of the defaulted struct would get wrong.
        std::fs::write(&path, "[server]\n[server.vm]\nenabled = false\n").unwrap();
        assert!(!Config::load(&path).unwrap().server.vm.enabled);
    }

    /// A VM key placed after another table header lands in the wrong table
    /// and is silently ignored — the same TOML scoping trap the Pages
    /// flags have. The example config is parsed by the real loader here so
    /// the two cannot drift apart.
    #[test]
    fn example_config_keeps_vm_keys_under_the_vm_table() {
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/config.example.toml");
        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|e| panic!("read {}: {e}", example.display()));
        let raw: toml::Value = toml::from_str(&text).expect("example config should be valid TOML");
        let vm = raw
            .get("server")
            .and_then(|s| s.get("vm"))
            .expect("example config should document [server.vm]");
        assert!(
            vm.get("enabled").is_some(),
            "vm.enabled must live under [server.vm]"
        );
    }

    /// The webhook signature must be a real HMAC-SHA256 over the exact body,
    /// because that is what a receiver recomputes to verify the payload.
    #[test]
    fn webhook_signature_is_hmac_sha256_of_the_body() {
        let body = r#"{"event":"push","repo_id":7}"#;
        let sig = sign_webhook_payload("s3cret", body);
        // Known-answer: HMAC-SHA256("s3cret", body) computed independently.
        assert_eq!(sig.len(), 64, "sha256 hex digest is 64 chars");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable across calls (deterministic, not random).
        assert_eq!(sig, sign_webhook_payload("s3cret", body));

        // Different secret -> different signature.
        assert_ne!(sig, sign_webhook_payload("other", body));
        // Different body -> different signature.
        assert_ne!(sig, sign_webhook_payload("s3cret", "{}"));
    }

    /// Cross-check against a hand-computed HMAC-SHA256 to catch an accidental
    /// change of algorithm or digest encoding.
    #[test]
    fn webhook_signature_matches_known_hmac_vector() {
        // RFC 4231 test case 1: key = 20 bytes of 0x0b, data = "Hi There".
        let key =
            "\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b";
        let expected = "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7";
        assert_eq!(sign_webhook_payload(key, "Hi There"), expected);
    }
}
