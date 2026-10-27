//! Server configuration types, TOML loading/merging, and defaults.

use anyhow::{Context, Result};

use serde::{Deserialize, Serialize};

use std::path::PathBuf;

use tracing::warn;

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
            // `database_schema`, but a config *file* setting it was
            // silently dropped on the way through `Config::load` — so a
            // server started with `-c gita.toml` always landed in
            // `public`, whatever the file said.
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
        } else {
            warn!("config file {} not found, using defaults", path.display());
        }
        Ok(config)
    }
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
