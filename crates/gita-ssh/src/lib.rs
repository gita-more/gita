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
use gita_core::access::Access;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use getrandom::rand_core::UnwrapErr;
use russh::keys::ssh_key;
use russh::server::Server;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Default file name (inside the data dir) for the SSH host key.
pub const DEFAULT_HOST_KEY_NAME: &str = "ssh_host_ed25519_key";

/// A pushed ref advertisement can be large for big repositories; keep the
/// capture bounded so a misbehaving server cannot grow memory without
/// bound. The refs themselves are at the start, so even a capped capture
/// covers them.
pub const MAX_CAPTURED_STDOUT: usize = 4 * 1024 * 1024;

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
///
/// `bus` and `git` are what a push needs *after* it lands: the event to
/// publish and the repository to read the CI configuration from. They
/// are passed in rather than looked up, because the SSH server is a
/// transport: what a push does about it is the same thing the HTTP
/// transport does, and that lives in `gita_push`.
pub async fn start(
    cfg: SshServerConfig,
    db: Db,
    bus: EventBus,
    git: Arc<GitManager>,
) -> anyhow::Result<SshServerHandle> {
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
        bus,
        git,
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
    bus: EventBus,
    git: Arc<GitManager>,
}

impl russh::server::Server for GitaServerFactory {
    type Handler = GitaHandler;
    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        GitaHandler::new(
            self.db.clone(),
            self.repos_root.clone(),
            self.bus.clone(),
            self.git.clone(),
        )
    }
}

// ---------------------------------------------------------------------------
// Per-connection handler
// ---------------------------------------------------------------------------

struct GitaHandler {
    db: Arc<Db>,
    repos_root: PathBuf,
    authed_user: Option<i64>,
    /// The event bus a push publishes its `push` event on.
    bus: EventBus,
    /// The repository manager, used to read the CI configuration of the
    /// commit that was pushed.
    git: Arc<GitManager>,
    /// Russh handle for pushing channel data from spawned tasks.
    handle: Option<russh::server::Handle>,
    /// Shared child stdin (channel data -> git receive-pack).
    child_stdin: Option<Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>>,
}

impl GitaHandler {
    fn new(
        db: Arc<Db>,
        repos_root: PathBuf,
        bus: EventBus,
        git: Arc<GitManager>,
    ) -> Self {
        Self {
            db,
            repos_root,
            authed_user: None,
            bus,
            git,
            handle: None,
            child_stdin: None,
        }
    }
}

/// Resolve an SSH git path (`owner/repo.git`, `/owner/repo.git`, with or
/// without a trailing `.git`) into an on-disk bare repository path,
/// refusing path traversal and malformed names.
fn resolve_repo_path(repos_root: &std::path::Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    let rel = rel.strip_suffix(".git").unwrap_or(rel);
    let mut parts = rel.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    // Reject extra path segments and traversal.
    if parts.next().is_some() {
        return None;
    }
    if owner.is_empty()
        || repo.is_empty()
        || owner.contains("..")
        || repo.contains("..")
        || owner.contains('\\')
        || repo.contains('\\')
        || owner.contains(':')
        || repo.contains(':')
    {
        return None;
    }
    Some(repos_root.join(owner).join(format!("{repo}.git")))
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
        // Accept the session channel; dropping the handle would reject it
        // with "administratively prohibited", which breaks git clients.
        reply.accept().await;
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

        let rel = path.trim_end_matches('\0');
        let bare = match resolve_repo_path(&self.repos_root, rel) {
            Some(p) => p,
            None => {
                tracing::debug!("ssh: rejected path rel={:?}", rel);
                let _ = session.data(channel, bytes::Bytes::from_static(b"repository not found\n"));
                let _ = session.close(channel);
                return Ok(());
            }
        };
        if !bare.exists() {
            tracing::debug!("ssh: repo not on disk: {}", bare.display());
            let _ = session.data(channel, bytes::Bytes::from_static(b"repository not found\n"));
            let _ = session.close(channel);
            return Ok(());
        }

        // Resolve owner/repo from the path for authorization.
        let rel_norm = rel.trim_start_matches('/');
        let (owner, repo_name) = match rel_norm
            .strip_suffix(".git")
            .unwrap_or(rel_norm)
            .split_once('/')
        {
            Some((o, r)) => (o.to_string(), r.to_string()),
            None => {
                let _ = session.data(channel, bytes::Bytes::from_static(b"repository not found\n"));
                let _ = session.close(channel);
                return Ok(());
            }
        };

        // Load the repository record and apply the shared access rule
        // (`gita_core::access`): a push needs push rights, a clone needs
        // read rights. Only the report differs — SSH has no status
        // codes, so a refusal is a message over the channel.
        let repo = match self.db.get_repository_by_name(&owner, &repo_name).await {
            Ok(Some(r)) => r,
            _ => {
                let _ = session.data(channel, bytes::Bytes::from_static(b"repository not found\n"));
                let _ = session.close(channel);
                return Ok(());
            }
        };
        let action = if verb == "receive-pack" {
            Access::Push
        } else {
            Access::Read
        };
        let allowed = match self.db.can_access(&repo, self.authed_user, action).await {
            Ok(allowed) => allowed,
            Err(e) => {
                tracing::error!("db error: {e}");
                false
            }
        };
        if !allowed {
            let message: &'static [u8] = if verb == "receive-pack" {
                b"permission denied: only the repository owner can push\n"
            } else {
                b"permission denied: this repository is private\n"
            };
            let _ = session.data(channel, bytes::Bytes::from_static(message));
            let _ = session.close(channel);
            return Ok(());
        }

        tracing::info!("ssh git: {verb} on {} by user {:?}", bare.display(), self.authed_user);

        // Acknowledge the exec request so the client starts sending protocol
        // data, then hand off the git child process to two background tasks:
        // a pump that copies stdout -> channel data, and a waiter that
        // reports the exit status and closes the channel. This keeps the
        // session event loop free so `data()` (channel -> stdin) keeps
        // flowing; blocking here would deadlock git push/pull.
        let handle = session.handle();
        self.handle = Some(handle.clone());
        let _ = handle.channel_success(channel).await;

        let mut child = tokio::process::Command::new("git")
            .arg(verb)
            .arg(&bare)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let child_stdin = child.stdin.take().expect("piped stdin");
        let child_stdout = child.stdout.take().expect("piped stdout");
        let child_stderr = child.stderr.take().expect("piped stderr");

        // Store the stdin so `data()` can push channel data into git.
        let stdin_arc = Arc::new(tokio::sync::Mutex::new(Some(child_stdin)));
        self.child_stdin = Some(stdin_arc.clone());

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

        // Pump stdout -> channel data in a background task, keeping a copy
        // of what the git child wrote. The copy is what the `report-status`
        // parser reads once the child exits: the section that says which
        // refs were actually updated lives at the end of stdout.
        let h = handle.clone();
        let channel_id = channel;
        let capture = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let waiter_capture = capture.clone();
        tokio::spawn(async move {
            let mut out_buf = vec![0u8; 8192];
            let mut stdout_reader = child_stdout;
            loop {
                match stdout_reader.read(&mut out_buf).await {
                    Ok(0) => {
                        let _ = h.eof(channel_id).await;
                        break;
                    }
                    Ok(n) => {
                        // Keep a copy for the report-status parser. The
                        // advertisement the client needs is small, but a
                        // cap keeps a misbehaving server from growing the
                        // buffer without bound.
                        {
                            let mut buf = capture.lock().await;
                            if buf.len() < MAX_CAPTURED_STDOUT {
                                let room = MAX_CAPTURED_STDOUT - buf.len();
                                buf.extend_from_slice(&out_buf[..n.min(room)]);
                            }
                        }
                        let chunk = bytes::Bytes::copy_from_slice(&out_buf[..n]);
                        if h.data(channel_id, chunk).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for the child and report its exit status / close the channel.
        let h2 = handle.clone();
        let bus = self.bus.clone();
        let git = self.git.clone();
        let db = self.db.clone();
        let pusher_id = self.authed_user;
        let push_owner = owner.clone();
        let push_repo = repo_name.clone();
        let is_push = verb == "receive-pack";
        tokio::spawn(async move {
            let status = child.wait().await;
            stderr_task.abort();
            if is_push {
                // The git child has exited, so its stdout is complete and
                // the report-status section can be read. A push that landed
                // publishes an event and may trigger CI — the same things
                // the HTTP transport does, which is the point: choosing
                // SSH over HTTP must not change what the repository does.
                let output = {
                    let mut buf = waiter_capture.lock().await;
                    std::mem::take(&mut *buf)
                };
                let updates = gita_push::parse_report_status(&output);
                if !updates.is_empty() {
                    gita_push::publish_push(
                        &db,
                        &bus,
                        &git,
                        &push_owner,
                        &push_repo,
                        &updates,
                        pusher_id,
                    )
                    .await;
                }
            }
            if let Ok(s) = status {
                if let Some(code) = s.code() {
                    let _ = h2.exit_status_request(channel_id, code as u32).await;
                }
            }
            let _ = h2.close(channel_id).await;
        });

        // Once the child exits, further `data()` writes to its pipe will
        // fail with BrokenPipe (handled below), so we do not need to
        // explicitly clear `self.child_stdin` here.
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
        // Trailing NUL is tolerated (some clients append it).
        assert_eq!(
            parse_git_command("git-upload-pack 'owner/repo.git'\0"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
    }

    #[test]
    fn resolve_repo_path_accepts_valid_owner_repo() {
        let root = PathBuf::from("/data/repos");

        let p = resolve_repo_path(&root, "alice/hello.git").unwrap();
        assert_eq!(p, root.join("alice").join("hello.git"));
        // Leading slash and .git suffix variations.
        let p2 = resolve_repo_path(&root, "/alice/hello.git").unwrap();
        assert_eq!(p2, root.join("alice").join("hello.git"));
        let p3 = resolve_repo_path(&root, "/alice/hello").unwrap();
        assert_eq!(p3, root.join("alice").join("hello.git"));
    }

    #[test]
    fn resolve_repo_path_rejects_traversal_and_malformed() {
        let root = PathBuf::from("/data/repos");

        // Path traversal attempts.
        assert!(resolve_repo_path(&root, "../secret.git").is_none());
        assert!(resolve_repo_path(&root, "../../etc/passwd.git").is_none());
        assert!(resolve_repo_path(&root, "a/../../b").is_none());
        // Missing owner or repo.
        assert!(resolve_repo_path(&root, "alice.git").is_none());
        assert!(resolve_repo_path(&root, "alice").is_none());
        assert!(resolve_repo_path(&root, "").is_none());
        // Too many segments.
        assert!(resolve_repo_path(&root, "a/b/c.git").is_none());
        // Windows separators / drive letters.
        assert!(resolve_repo_path(&root, "a\\b\\c.git").is_none());
        assert!(resolve_repo_path(&root, "C:/x.git").is_none());
    }
}