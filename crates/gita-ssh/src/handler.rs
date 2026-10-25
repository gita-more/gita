//! Per-connection SSH session handling: public-key auth, exec request,
//! spawning the git child process, and publishing push events.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gita_core::access::Access;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use russh::keys::ssh_key;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::path::{parse_git_command, resolve_repo_path};
use crate::MAX_CAPTURED_COMMANDS;

/// The russh server factory: turns an accepted connection into a handler.
#[derive(Clone)]
pub(crate) struct GitaServerFactory {
    pub db: Arc<Db>,
    pub repos_root: PathBuf,
    pub bus: EventBus,
    pub git: Arc<GitManager>,
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

/// Per-connection handler.
pub(crate) struct GitaHandler {
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
    /// A copy of what the client sent to the git child.
    ///
    /// The client's ref-update commands arrive here, in the same pkt-line
    /// format the HTTP transport's request body uses. They are the
    /// client's *claim* about what it is pushing; which of them actually
    /// took effect is decided afterwards by looking at the repository.
    client_commands: Arc<tokio::sync::Mutex<Vec<u8>>>,
}

impl GitaHandler {
    pub(crate) fn new(
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
            client_commands: Arc::new(tokio::sync::Mutex::new(Vec::new())),
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
                let _ = session.data(
                    channel,
                    bytes::Bytes::from_static(b"invalid command encoding\n"),
                );
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
                let _ = session.data(
                    channel,
                    bytes::Bytes::from_static(b"repository not found\n"),
                );
                let _ = session.close(channel);
                return Ok(());
            }
        };
        if !bare.exists() {
            tracing::debug!("ssh: repo not on disk: {}", bare.display());
            let _ = session.data(
                channel,
                bytes::Bytes::from_static(b"repository not found\n"),
            );
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
                let _ = session.data(
                    channel,
                    bytes::Bytes::from_static(b"repository not found\n"),
                );
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
                let _ = session.data(
                    channel,
                    bytes::Bytes::from_static(b"repository not found\n"),
                );
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

        tracing::info!(
            "ssh git: {verb} on {} by user {:?}",
            bare.display(),
            self.authed_user
        );

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

        // Pump stdout -> channel data in a background task.
        let h = handle.clone();
        let channel_id = channel;
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
        let commands = self.client_commands.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            stderr_task.abort();
            if is_push {
                // The git child has exited, so the refs it was given are
                // settled. A push that landed publishes an event and may
                // trigger CI — the same things the HTTP transport does,
                // which is the point: choosing SSH over HTTP must not
                // change what the repository does.
                //
                // The client's commands say what it *asked* for; the
                // repository says what actually happened. Only the refs
                // that really moved are reported, so a refused push
                // publishes nothing.
                let claimed = {
                    let mut buf = commands.lock().await;
                    std::mem::take(&mut *buf)
                };
                let updates: Vec<gita_push::RefUpdate> =
                    gita_push::parse_receive_pack_commands(&claimed)
                        .into_iter()
                        .filter(|upd| {
                            gita_push::ref_update_applied(&git, &push_owner, &push_repo, upd)
                        })
                        .collect();
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
        // Keep a copy of what the client sent. The ref-update commands
        // come first, followed by the pack data; the parser stops at the
        // flush packet that ends the command list, so the pack data
        // behind it does not matter.
        {
            let mut buf = self.client_commands.lock().await;
            if buf.len() < MAX_CAPTURED_COMMANDS {
                let room = MAX_CAPTURED_COMMANDS - buf.len();
                buf.extend_from_slice(&data[..data.len().min(room)]);
            }
        }
        if let Some(arc) = self.child_stdin.as_ref() {
            let mut guard = arc.lock().await;
            if let Some(stdin) = guard.as_mut() {
                let _ = stdin.write_all(data).await;
            }
        }
        Ok(())
    }
}
