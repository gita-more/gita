//! The embedded SSH server: listener startup, shutdown signal, and the
//! handle exposed to the rest of the application.

use std::sync::Arc;

use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use russh::server::Server;

use crate::config::SshServerConfig;
use crate::handler::GitaServerFactory;

/// A signal that tells the SSH listener to stop accepting.
///
/// It is held behind an `Arc` and shared with the listener task, so
/// dropping the handle does *not* stop the server. That matters: the
/// handle is often dropped right after the port is read (a test that
/// only wants the port, say), and a server that stopped when its
/// handle went away would be a trap.
#[derive(Clone, Default)]
struct ShutdownSignal {
    inner: Arc<ShutdownSignalInner>,
}

#[derive(Default)]
struct ShutdownSignalInner {
    requested: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl ShutdownSignal {
    fn request(&self) {
        self.inner
            .requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // `notify_one` stores a permit, so a listener that is not
        // waiting yet still wakes up.
        self.inner.notify.notify_one();
    }

    /// Wait until a shutdown is requested.
    async fn wait(&self) {
        loop {
            if self
                .inner
                .requested
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            self.inner.notify.notified().await;
        }
    }
}

pub struct SshServerHandle {
    pub local_addr: std::net::SocketAddr,
    /// Tells the listener to stop accepting connections.
    shutdown: ShutdownSignal,
    _join: tokio::task::JoinHandle<()>,
}

impl SshServerHandle {
    pub fn addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    /// Stop accepting new SSH connections.
    ///
    /// A push that is in flight when the server is told to stop is
    /// allowed to finish: the listener stops accepting, but the sessions
    /// that are already open run to completion.
    pub async fn shutdown(&self) {
        self.shutdown.request();
        // The accept loop notices the request and unwinds; give it a
        // moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
    let shutdown = ShutdownSignal::default();
    let listen_shutdown = shutdown.clone();
    let join = tokio::spawn(async move {
        // The listener stops accepting when the server is told to shut
        // down; the sessions that are already open are left to finish.
        let server = factory.run_on_socket(server_cfg, &socket);
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => {
                let _ = result;
            }
            _ = listen_shutdown.wait() => {
                // Stop accepting; the server future is dropped, which
                // closes the listener.
            }
        }
    });

    Ok(SshServerHandle {
        local_addr,
        shutdown,
        _join: join,
    })
}
