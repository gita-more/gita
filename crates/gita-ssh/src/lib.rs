//! Embedded SSH server for the Git protocol.
//!
//! Listens for SSH connections from `git@host:owner/repo.git` clients and
//! runs the corresponding `git-upload-pack` / `git-receive-pack` child
//! process with its stdio wired to the SSH channel. Public-key auth is
//! looked up against the `ssh_keys` table; an unknown key disconnects.
//!
//! Wire protocol: clients connect via `ssh://` or the scp-style `git@host:`
//! URLs. The server accepts only `exec` channels (no shell, no PTY).

use anyhow::{Context, Result};
use gita_db::Db;
use getrandom::rand_core::UnwrapErr;
use russh::keys::ssh_key;
use russh::server::Server;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Default file name (inside the data dir) for the SSH host key.
pub const DEFAULT_HOST_KEY_NAME: &str = "ssh_host_ed25519_key";

/// Load an existing OpenSSH-format host key or generate a fresh Ed25519
/// key and persist it (mode 0600) so subsequent restarts keep the same
/// host identity.
pub fn load_or_create_host_key(path: &std::path::Path) -> Result<ssh_key::PrivateKey> {
    if path.exists() {
        let pem = std::fs::read(path)
            .with_context(|| format!("read SSH host key {}", path.display()))?;
        return ssh_key::PrivateKey::from_openssh(pem)
            .with_context(|| format!("parse SSH host key {}", path.display()));
    }
    let mut rng = UnwrapErr(getrandom::SysRng);
    let key = ssh_key::PrivateKey::random(&mut rng, ssh_key::Algorithm::Ed25519)
        .context("generate Ed25519 host key")?;
    key.write_openssh_file(path, ssh_key::LineEnding::LF)
        .with_context(|| format!("write SSH host key {}", path.display()))?;
    Ok(key)
}

/// Configuration for the SSH server.
#[derive(Clone)]
pub struct SshServerConfig {
    pub listen_addr: String,
    pub host_key: ssh_key::PrivateKey,
    /// Path to the bare repositories root (e.g. `<data_dir>/repos`).
    pub repos_root: PathBuf,
}

pub struct SshServerHandle {
    pub local_addr: std::net::SocketAddr,
    _join: tokio::task::JoinHandle<()>,
}

impl SshServerHandle {
    pub fn addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }
}

/// Start the SSH server on `cfg.listen_addr`.
pub async fn start(cfg: SshServerConfig, db: Db) -> anyhow::Result<SshServerHandle> {
    let socket = tokio::net::TcpListener::bind(&cfg.listen_addr).await?;
    let local_addr = socket.local_addr()?;

    let server_cfg = Arc::new(russh::server::Config {
        auth_rejection_time: std::time::Duration::from_secs(3),
        keys: vec![cfg.host_key.clone()],
        ..Default::default()
    });
    let mut factory = GitaServerFactory {
        db: Arc::new(db),
        repos_root: cfg.repos_root.clone(),
    };
    let join = tokio::spawn(async move {
        let server = factory.run_on_socket(server_cfg, &socket);
        let _ = server.await;
    });

    Ok(SshServerHandle {
        local_addr,
        _join: join,
    })
}

// ---------------------------------------------------------------------------
// Server factory
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct GitaServerFactory {
    db: Arc<Db>,
    repos_root: PathBuf,
}

impl russh::server::Server for GitaServerFactory {
    type Handler = GitaHandler;
    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        GitaHandler::new(self.db.clone(), self.repos_root.clone())
    }
}

// ---------------------------------------------------------------------------
// Per-connection handler
// ---------------------------------------------------------------------------

struct GitaHandler {
    db: Arc<Db>,
    repos_root: PathBuf,
    authed_user: Option<i64>,
    /// Shared child stdin (channel data -> git receive-pack).
    child_stdin: Option<Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>>,
}

impl GitaHandler {
    fn new(db: Arc<Db>, repos_root: PathBuf) -> Self {
        Self {
            db,
            repos_root,
            authed_user: None,
            child_stdin: None,
        }
    }
}

impl russh::server::Handler for GitaHandler {
    type Error = anyhow::Error;

    fn auth_publickey(
        &mut self,
        _user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> impl Future<Output = Result<russh::server::Auth, Self::Error>> + Send {
        let db = self.db.clone();
        let fingerprint = public_key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
        async move {
            let row = db.find_ssh_key_by_fingerprint(&fingerprint).await?;
            match row {
                Some(key) => {
                    self.authed_user = Some(key.user_id);
                    Ok(russh::server::Auth::Accept)
                }
                None => Ok(russh::server::Auth::reject()),
            }
        }
    }

    async fn auth_succeeded(
        &mut self,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let _ = reply;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let command = match std::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => {
                let _ = session.data(channel, bytes::Bytes::from_static(b"invalid command encoding\n"));
                let _ = session.close(channel);
                return Ok(());
            }
        };
        let (verb, path) = match parse_git_command(command) {
            Some(parts) => parts,
            None => {
                let _ = session.data(
                    channel,
                    bytes::Bytes::from_static(
                        b"only git-upload-pack / git-receive-pack are supported\n",
                    ),
                );
                let _ = session.close(channel);
                return Ok(());
            }
        };

        let rel = path
            .trim_start_matches('/')
            .trim_start_matches("git/")
            .trim_end_matches('\0');
        let bare = self.repos_root.join(format!("{rel}.git"));
        if !bare.exists() {
            let _ = session.data(channel, bytes::Bytes::from_static(b"repository not found\n"));
            let _ = session.close(channel);
            return Ok(());
        }

        tracing::info!("ssh git: {verb} on {}", bare.display());

        let mut child = tokio::process::Command::new("git")
            .arg(verb)
            .env("GIT_PROJECT_ROOT", &self.repos_root)
            .env("GIT_HTTP_EXPORT_ALL", "1")
            .env("PATH_INFO", format!("/{rel}"))
            .env("REMOTE_USER", "ssh")
            .env("REMOTE_ADDR", "ssh")
            .env("CONTENT_TYPE", format!("application/x-{verb}-request"))
            .env("REQUEST_METHOD", "POST")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let child_stdin = child.stdin.take().expect("piped stdin");
        let child_stdout = child.stdout.take().expect("piped stdout");
        let child_stderr = child.stderr.take().expect("piped stderr");

        // Store the stdin so `data()` can push channel data into git.
        self.child_stdin = Some(Arc::new(tokio::sync::Mutex::new(Some(child_stdin))));

        // Drain stderr to tracing asynchronously.
        let mut stderr_reader = child_stderr;
        let stderr_task = tokio::spawn(async move {
            let mut err_buf = vec![0u8; 2048];
            loop {
                match stderr_reader.read(&mut err_buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&err_buf[..n]);
                        if !s.trim().is_empty() {
                            tracing::debug!(target: "gita_ssh.git.stderr", "{s}");
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Pump stdout -> channel data.
        let mut stdout_reader = child_stdout;
        let mut out_buf = vec![0u8; 8192];

        loop {
            match stdout_reader.read(&mut out_buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = out_buf[..n].to_vec();
                    if let Err(e) = session.data(channel, bytes::Bytes::from(chunk)) {
                        tracing::warn!("ssh channel write failed: {e}");
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => anyhow::bail!("wait child failed: {e}"),
        };

        stderr_task.abort();

        // Clear stdin so data() no longer writes into a dead process.
        if let Some(s) = self.child_stdin.take() {
            let mut guard = s.lock().await;
            *guard = None;
        }

        if !status.success() {
            tracing::warn!(
                "ssh git {verb} on {} exited with {:?}",
                bare.display(),
                status.code()
            );
        }
        let _ = session.close(channel);
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        _channel: russh::ChannelId,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: russh::ChannelId,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: russh::ChannelId,
        data: &[u8],
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(arc) = self.child_stdin.as_ref() {
            let mut guard = arc.lock().await;
            if let Some(stdin) = guard.as_mut() {
                let _ = stdin.write_all(data).await;
            }
        }
        Ok(())
    }
}

fn parse_git_command(command: &str) -> Option<(&'static str, String)> {
    let s = command.trim().trim_end_matches('\0');
    let stripped = s.strip_prefix("git-")?;
    let (verb, rest) = if let Some(rest) = stripped.strip_prefix("upload-pack ") {
        ("upload-pack", rest)
    } else if let Some(rest) = stripped.strip_prefix("receive-pack ") {
        ("receive-pack", rest)
    } else {
        return None;
    };
    let path = rest.trim();
    let path = path
        .strip_prefix('\'')
        .and_then(|p| p.strip_suffix('\''))
        .unwrap_or(path);
    Some((
        match verb {
            "upload-pack" => "upload-pack",
            "receive-pack" => "receive-pack",
            _ => return None,
        },
        path.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_is_generated_and_reloaded() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ssh_host_ed25519_key");

        let key = load_or_create_host_key(&path).unwrap();
        // The file must be created.
        assert!(path.exists());
        // Reloading returns an identical key (same host identity).
        let reloaded = load_or_create_host_key(&path).unwrap();
        assert_eq!(
            key.public_key().fingerprint(ssh_key::HashAlg::Sha256).to_string(),
            reloaded.public_key().fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        );
    }

    #[test]
    fn host_key_rejects_invalid_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad_key");
        std::fs::write(&path, "not a key").unwrap();
        assert!(load_or_create_host_key(&path).is_err());
    }

    #[test]
    fn parse_git_command_handles_common_forms() {
        assert_eq!(
            parse_git_command("git-upload-pack 'owner/repo.git'"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
        assert_eq!(
            parse_git_command("git-receive-pack '/srv/git/owner/repo.git'"),
            Some(("receive-pack", "/srv/git/owner/repo.git".to_string()))
        );
        assert_eq!(
            parse_git_command("git-upload-pack owner/repo.git"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
        assert_eq!(parse_git_command("ls"), None);
        assert_eq!(parse_git_command("git-upload-pack"), None);
    }
}