//! SSH host-key management and server configuration.

use anyhow::{Context, Result};
use getrandom::rand_core::UnwrapErr;
use russh::keys::ssh_key;
use std::path::Path;
use std::path::PathBuf;

/// Default file name (inside the data dir) for the SSH host key.
pub const DEFAULT_HOST_KEY_NAME: &str = "ssh_host_ed25519_key";

/// Load an existing OpenSSH-format host key or generate a fresh Ed25519
/// key and persist it (mode 0600) so subsequent restarts keep the same
/// host identity.
pub fn load_or_create_host_key(path: &Path) -> Result<ssh_key::PrivateKey> {
    if path.exists() {
        let pem =
            std::fs::read(path).with_context(|| format!("read SSH host key {}", path.display()))?;
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
