//! Server startup, runner registration, and webhook signing.

use anyhow::{Context, Result};
use gita_db::Db;
use std::path::PathBuf;

use std::sync::Arc;

use tracing::{info, warn};

use crate::App;
use crate::Config;
use crate::shutdown;

/// Locate the built React frontend (`web/dist`).
///
/// The frontend may live in a few different places depending on how the
/// binary was built and deployed:
/// 1. `<CARGO_MANIFEST_DIR>/../../web/dist` — building from the workspace
///    source tree (compile-time absolute path).
/// 2. `<exe_dir>/web/dist` — a container/image layout where the binary and
///    frontend are siblings under a common root.
/// 3. `<current_dir>/web/dist` — running from the workspace root.
pub(crate) fn find_web_dist() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from));
    let cwd = std::env::current_dir().ok();

    let candidates = [
        manifest.join("../../web/dist"),
        exe_dir
            .as_ref()
            .map(|d| d.join("web/dist"))
            .unwrap_or_default(),
        cwd.as_ref().map(|d| d.join("web/dist")).unwrap_or_default(),
    ];

    candidates
        .into_iter()
        .find(|p| p.join("index.html").is_file())
}

/// Run the server.
pub async fn run(config: Config) -> Result<()> {
    let mut app = App::build(config.clone()).await?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!("Gita listening on http://{addr}");

    // Start the SSH listener (Git over SSH) when enabled.
    if config.server.ssh.enabled {
        let ssh_cfg = gita_ssh::SshServerConfig {
            listen_addr: format!("{}:{}", config.server.ssh.host, config.server.ssh.port),
            host_key: gita_ssh::load_or_create_host_key(
                &config.server.ssh.host_key.clone().unwrap_or_else(|| {
                    config.server.data_dir.join(gita_ssh::DEFAULT_HOST_KEY_NAME)
                }),
            )?,
            repos_root: config.server.data_dir.join("repos"),
        };
        let handle =
            gita_ssh::start(ssh_cfg, app.db.clone(), app.bus.clone(), app.git.clone()).await?;
        info!(
            "Gita SSH listening on {} (git@{}:{}/owner/repo.git)",
            handle.addr(),
            config.server.ssh.host,
            config.server.ssh.port
        );
        app.ssh_handle = Some(Arc::new(handle));
    }

    // Per-IP login rate limiter: 5 req/s with burst 10 — slows brute force
    // without affecting normal users.
    let limiter = gita_api::build_limiter(5, 10);
    let router = app.router_with_limiter(limiter);

    // Stop accepting new connections when the process is asked to stop,
    // but let the requests that are already running finish. A `SIGTERM`
    // that killed the process outright cut them off mid-flight.
    let serve = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown::shutdown_signal());

    if let Err(e) = serve.await {
        warn!("axum serve error: {e}");
    }

    // The HTTP listener is closed; now let the work that is still in
    // flight finish. The event row for something that just happened, and
    // the webhooks that event fans out to, are written from background
    // tasks — without this wait they would be lost.
    let pending = app.background_tasks.active();
    if pending > 0 {
        info!("waiting for {pending} background task(s) to finish");
    }
    app.background_tasks.drain().await;

    // The SSH listener has its own accept loop; stop it too.
    if let Some(handle) = app.ssh_handle.take() {
        handle.shutdown().await;
    }

    info!("Gita stopped");
    Ok(())
}

/// Mint a runner token for `username`, stored under `name`, and return
/// the raw token for the caller to print.
pub async fn worker_register(config: &Config, name: &str, username: &str) -> Result<String> {
    let db = Db::connect(&config.server.database_url).await?;
    let user = db
        .get_user_by_username(username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("user not found: {username}"))?;

    // 32 random bytes hex-encoded — same shape as the REST PAT flow.
    let token = random_hex_token();
    db.create_access_token(user.id, &token, &format!("runner/{name}"))
        .await?;
    Ok(token)
}

/// Sign a webhook payload with the hook's secret.
///
/// GitHub's model: the receiver recomputes HMAC-SHA256 over the raw request
/// body with the shared secret and compares it to the signature the sender
/// put in the header. A mismatch means the payload was tampered with or did
/// not come from this server.
///
/// Returns a lowercase hex digest.
pub fn sign_webhook_payload(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Generate a random hex token for runner registration.
fn random_hex_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
