//! Credential store: OS keyring with an encrypted local fallback.
//!
//! ## The keyring
//!
//! The token is stored under service name `"gita"` with the account set to
//! the canonical server URL (e.g. `http://127.0.0.1:3000`), so multiple
//! server profiles can coexist. The username is stored under
//! `{url}#username` for git smart HTTP clone URLs.
//!
//! On Linux the `keyring` crate enables two stores: the kernel keyring
//! (`linux-native-sync-persistent`) and the Secret Service
//! (`sync-secret-service`, zbus-based, no C dependencies).
//!
//! # Every call is bounded
//!
//! Both Linux stores can block **forever**, not merely fail:
//!
//! * Secret Service: when the user's collection is locked and no unlock
//!   agent can prompt (headless server, CI, cron), the D-Bus reply never
//!   arrives.
//! * Kernel keyring: `keyctl_join_session_keyring` asks the kernel for the
//!   user keyring via `request_key()`, which blocks until some other
//!   process supplies it. Sessions without `pam_keyinit` (containers, bare
//!   `ssh` sessions) never do.
//!
//! The CLI treats the keyring as best-effort — a missing keyring must
//! degrade, not hang. So every keyring operation runs on a worker thread
//! with a hard deadline, and the first timeout latches the keyring off for
//! the rest of the process so the second call fails immediately instead of
//! paying the timeout again. The failure is also recorded in
//! `~/.gita/keyring-unavailable` for an hour, so later commands skip the
//! keyring instead of paying the deadline again; the record expires, so a
//! keyring that starts working is picked up.
//!
//! ## The encrypted local fallback
//!
//! A keyring that cannot store the token must not lose the session: the
//! CLI would happily log in and then be anonymous again on the next
//! command. So when the keyring is unavailable (missing, latched off, or
//! disabled via `GITA_DISABLE_KEYRING=1`) the token degrades to
//! `~/.gita/credentials.enc` — encrypted with XChaCha20-Poly1305 under a
//! machine-bound key (see [`crate::crypt`]).
//!
//! The key lives in the keyring when the keyring works, otherwise in a
//! `0600` `~/.gita/keyfile`. The user supplies nothing beyond the login
//! they already typed: the fallback is fully transparent, and its
//! protection is aimed at the realistic threat — the disk's contents
//! leaving the machine (backups, images, sync clients). An attacker who
//! can run code as the user would simply run `gita` itself, so the local
//! keyfile costs nothing there.
//!
//! Reads prefer the keyring and fall back to the file; `delete_secret`
//! clears both. `GITA_DISABLE_KEYRING` only switches off the keyring — the
//! file fallback is exactly the answer to that situation. A store that
//! cannot be decrypted (the key was deleted, the file came from another
//! machine) reads as empty rather than failing: the credentials are gone
//! either way and the answer is a fresh login.
//!
//! Config and credentials live under `$GITA_CONFIG_DIR` or `~/.gita` (see
//! [`crate::config::config_dir`]).

use anyhow::{bail, Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::config::config_dir;

const SERVICE: &str = "gita";

/// Upper bound for a single keyring operation.
///
/// A healthy keyring answers in milliseconds; this only ever bounds the
/// pathological blocking case, so it can be generous.
const OP_TIMEOUT: Duration = Duration::from_secs(3);

/// Latched once an operation has timed out. Later calls fail immediately
/// instead of blocking for another `OP_TIMEOUT`.
static KEYRING_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Where a stored credential ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// The OS keyring.
    Keyring,
    /// The local encrypted credential file (the keyring was unavailable).
    EncryptedFile,
}

/// Path of the local credential file, for caller-facing messages.
pub fn credentials_file() -> Result<PathBuf> {
    config_dir()
        .map(|d| d.join("credentials.enc"))
        .context("cannot determine the gita config directory (set HOME or GITA_CONFIG_DIR)")
}

fn account(url: &str) -> String {
    url.to_string()
}

fn username_account(url: &str) -> String {
    format!("{url}#username")
}

/// Whether the user asked for the keyring to be left alone.
fn disabled_by_env() -> bool {
    match std::env::var("GITA_DISABLE_KEYRING") {
        Ok(v) => !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"),
        Err(_) => false,
    }
}

/// Run `op` on a worker thread and give up after [`OP_TIMEOUT`].
///
/// The worker is detached: if the keyring backend stays blocked forever
/// the thread outlives the call, but it dies with the process — which is
/// exactly what we want for a short-lived CLI.
fn run_bounded<T, F>(op: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may be gone if we already timed out; ignore the
        // send error rather than panicking in a detached thread.
        let _ = tx.send(op());
    });
    match rx.recv_timeout(OP_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            // Remember it beyond this process: the next command should
            // not pay the same deadline to rediscover that the keyring
            // blocks on this machine.
            mark_keyring_broken();
            bail!("keyring operation timed out after {}s", OP_TIMEOUT.as_secs());
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("keyring operation failed: the backend exited unexpectedly");
        }
    }
}

/// Whether a keyring read attempt should even be made.
fn keyring_skipped() -> bool {
    disabled_by_env() || KEYRING_UNAVAILABLE.load(Ordering::Relaxed) || keyring_marked_broken()
}

/// How long a recorded keyring failure keeps the keyring out of the way.
const KEYRING_MARKER_TTL_SECS: u64 = 3600;

/// The file that records "the keyring did not work here".
fn keyring_marker_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("keyring-unavailable"))
}

/// Whether an earlier run recorded the keyring as unusable.
///
/// The marker is per machine, not per process: the in-process latch only
/// helps within one command, so every login would otherwise pay the full
/// deadline again to rediscover that the keyring blocks. The marker
/// expires, so a keyring that starts working (a Secret Service finally
/// running) is picked up again.
fn keyring_marked_broken() -> bool {
    let Some(path) = keyring_marker_path() else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(until) = content.trim().parse::<u64>() else {
        return false;
    };
    now_secs() < until
}

/// Record that the keyring did not work, so the next command can skip it.
fn mark_keyring_broken() {
    let Some(path) = keyring_marker_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let until = now_secs() + KEYRING_MARKER_TTL_SECS;
    let _ = std::fs::write(path, until.to_string());
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Encrypted credentials file
// ---------------------------------------------------------------------------
//
// The file is never written in plaintext: a machine-bound key encrypts it
// (see `crate::crypt`). The key comes from the OS keyring when one works,
// otherwise from a `0600` keyfile beside it, so the user never has to
// supply anything beyond the login they already typed.

/// Keyring account holding the *encryption key* for the credential file.
const FILE_KEY_ACCOUNT: &str = "credentials-key";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Credentials {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sites: BTreeMap<String, SiteCredentials>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SiteCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}

/// Path of the local encryption key, used when the keyring cannot hold it.
pub fn key_file() -> Result<PathBuf> {
    config_dir()
        .map(|d| d.join("keyfile"))
        .context("cannot determine the gita config directory (set HOME or GITA_CONFIG_DIR)")
}

fn read_file_key() -> Option<[u8; crate::crypt::KEY_LEN]> {
    let path = key_file().ok()?;
    let bytes = std::fs::read(path).ok()?;
    bytes.try_into().ok()
}

/// Write the keyfile with `0600`, creating the directory `0700` if needed.
fn write_file_key(key: &[u8; crate::crypt::KEY_LEN]) -> Result<()> {
    let path = key_file()?;
    let dir = path
        .parent()
        .context("keyfile path has no parent directory")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create config dir {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(&path, key).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The key that encrypts the credential file, generating one on first use.
///
/// Prefers the OS keyring; falls back to the local keyfile. The choice is
/// returned so the file's header can record which store holds the key.
fn file_key() -> Result<([u8; crate::crypt::KEY_LEN], crate::crypt::KeySource)> {
    use crate::crypt::{generate_key, KeySource};

    if !keyring_skipped() {
        let from_keyring = run_bounded(|| {
            match Entry::new(SERVICE, FILE_KEY_ACCOUNT)?.get_password() {
                Ok(encoded) => Ok(Some(encoded)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => bail!("keyring read failed for the credential key: {e}"),
            }
        });
        match from_keyring {
            Ok(Some(encoded)) => {
                if let Some(key) = decode_key(&encoded) {
                    return Ok((key, KeySource::Keyring));
                }
            }
            Ok(None) => {
                // No key yet: mint one and keep it in the keyring.
                let key = generate_key();
                let encoded = encode_key(&key);
                let stored = run_bounded(move || {
                    Entry::new(SERVICE, FILE_KEY_ACCOUNT)?
                        .set_password(&encoded)
                        .map_err(Into::into)
                });
                if stored.is_ok() {
                    return Ok((key, KeySource::Keyring));
                }
            }
            Err(_) => {}
        }
    }

    // The keyring is out of the picture: use (or create) the local keyfile.
    if let Some(key) = read_file_key() {
        return Ok((key, KeySource::KeyFile));
    }
    let key = generate_key();
    write_file_key(&key)?;
    Ok((key, KeySource::KeyFile))
}

fn encode_key(key: &[u8; crate::crypt::KEY_LEN]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(key)
}

fn decode_key(encoded: &str) -> Option<[u8; crate::crypt::KEY_LEN]> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let bytes = STANDARD.decode(encoded.trim()).ok()?;
    bytes.try_into().ok()
}

/// Read the credential store, decrypting it as needed.
///
/// A missing file is an empty store. A file that cannot be decrypted —
/// the key is gone, the file is damaged, or it was written by another
/// machine — is treated as empty as well: the credentials are gone either
/// way, and the caller's answer is the same ("log in again"). It is not
/// deleted, so a key that comes back restores access.
fn read_credentials() -> Credentials {
    // The encrypted store, if present.
    if let Ok(path) = credentials_file() {
        if let Ok(blob) = std::fs::read(&path) {
            if blob.starts_with(b"GITAENC") {
                let Ok((key, _)) = file_key() else {
                    return Credentials::default();
                };
                let Ok(plaintext) = crate::crypt::decrypt(&key, &blob) else {
                    return Credentials::default();
                };
                return serde_json::from_slice(&plaintext).unwrap_or_default();
            }
            // Not ours (damaged or foreign): fall through to the legacy
            // file rather than guessing.
        }
    }

    // Migration: the pre-encryption plaintext file.
    let Some(legacy) = config_dir().map(|d| d.join("credentials.toml")) else {
        return Credentials::default();
    };
    let Ok(content) = std::fs::read_to_string(legacy) else {
        return Credentials::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

/// Write the credential store, encrypted, with `0600` (directory `0700`).
fn write_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_file()?;
    let dir = path
        .parent()
        .context("credentials path has no parent directory")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create config dir {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }

    let (key, source) = file_key()?;
    let plaintext = serde_json::to_vec(creds).context("serialize credentials")?;
    let blob = crate::crypt::encrypt(&key, source, &plaintext)?;
    std::fs::write(&path, blob).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Migration complete: a leftover plaintext store must not linger on
    // disk once the encrypted one exists.
    if let Some(legacy) = config_dir().map(|d| d.join("credentials.toml")) {
        let _ = std::fs::remove_file(legacy);
    }
    Ok(())
}

/// Update one site's entry in the credentials file.
fn with_site<F>(url: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut SiteCredentials),
{
    let mut creds = read_credentials();
    let site = creds.sites.entry(url.to_string()).or_default();
    f(site);
    // Drop the entry entirely when nothing is left in it.
    if site.token.is_none() && site.username.is_none() {
        creds.sites.remove(url);
    }
    write_credentials(&creds)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the stored token for `url`. Returns `Ok(None)` when nothing is
/// stored (e.g. the Secret Service is not running and no fallback file
/// exists).
pub fn get_secret(url: &str) -> Result<Option<String>> {
    if !keyring_skipped() {
        let url = url.to_string();
        let from_keyring = run_bounded(move || {
            match Entry::new(SERVICE, &account(&url))?.get_password() {
                Ok(token) => Ok(Some(token)),
                // "No entry" means the keyring works — it just has nothing
                // for us.
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => bail!("keyring read failed for {url}: {e}"),
            }
        });
        if let Ok(Some(token)) = from_keyring {
            return Ok(Some(token));
        }
    }
    Ok(read_credentials().sites.get(url).and_then(|s| s.token.clone()))
}

/// Store `token` for `url`. Any previous token is replaced.
///
/// Prefers the keyring; when the keyring cannot be used (unavailable,
/// latched off after a timeout, or disabled via `GITA_DISABLE_KEYRING`)
/// the token falls back to the local encrypted file — and the returned
/// [`Store`] tells the caller so it can say where the secret landed.
pub fn set_secret(url: &str, token: &str) -> Result<Store> {
    if !keyring_skipped() {
        let url = url.to_string();
        let token = token.to_string();
        let attempt = run_bounded(move || {
            Entry::new(SERVICE, &account(&url))?
                .set_password(&token)
                .with_context(|| format!("keyring write failed for {url}"))
        });
        if attempt.is_ok() {
            return Ok(Store::Keyring);
        }
    }
    with_site(url, |site| site.token = Some(token.to_string()))?;
    Ok(Store::EncryptedFile)
}

/// Store the authenticated username for `url`. Same fallback as
/// [`set_secret`].
pub fn set_username(url: &str, username: &str) -> Result<Store> {
    if !keyring_skipped() {
        let url = url.to_string();
        let username = username.to_string();
        let attempt = run_bounded(move || {
            Entry::new(SERVICE, &username_account(&url))?
                .set_password(&username)
                .with_context(|| format!("keyring write failed for {url}#username"))
        });
        if attempt.is_ok() {
            return Ok(Store::Keyring);
        }
    }
    with_site(url, |site| site.username = Some(username.to_string()))?;
    Ok(Store::EncryptedFile)
}

/// Read the stored username for `url`.
pub fn get_username(url: &str) -> Result<Option<String>> {
    if !keyring_skipped() {
        let url = url.to_string();
        let from_keyring = run_bounded(move || {
            match Entry::new(SERVICE, &username_account(&url))?.get_password() {
                Ok(username) => Ok(Some(username)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => bail!("keyring read failed for {url}#username: {e}"),
            }
        });
        if let Ok(Some(username)) = from_keyring {
            return Ok(Some(username));
        }
    }
    Ok(read_credentials()
        .sites
        .get(url)
        .and_then(|s| s.username.clone()))
}

/// Remove the stored token and username for `url`.
///
/// Best-effort by design: a keyring we cannot reach cannot be cleared
/// either, but the plaintext fallback can — and must be, or `logout`
/// would leave a live token on disk.
pub fn delete_secret(url: &str) -> Result<()> {
    if !keyring_skipped() {
        let url = url.to_string();
        let _ = run_bounded(move || {
            let _ = Entry::new(SERVICE, &account(&url))?.delete_credential();
            let _ = Entry::new(SERVICE, &username_account(&url))?.delete_credential();
            Ok::<(), anyhow::Error>(())
        });
    }
    with_site(url, |site| {
        site.token = None;
        site.username = None;
    })
}

/// The credentials stored for one server.
#[derive(Debug, Clone)]
pub struct SiteCredential {
    /// The server URL the credential is keyed by.
    pub url: String,
    pub username: Option<String>,
    pub token: Option<String>,
}

/// The scheme and authority of a URL, lowercased — e.g. `http://host:3100`.
///
/// Two URLs point at the same server when this matches. The path is
/// irrelevant (git asks about a repository, the credential is per
/// server), and the default port is spelled out so that `host` and
/// `host:80` compare equal.
fn authority_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let authority = match parsed.port_or_known_default() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    Some(format!("{}://{}", parsed.scheme().to_ascii_lowercase(), authority))
}

/// Split a URL into its scheme and host[:port].
///
/// Returns `None` for anything that does not parse as a URL.
pub fn split_scheme_host(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let host = match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    Some((parsed.scheme().to_ascii_lowercase(), host))
}

/// Whether two URLs point at the same server.
pub fn same_host(a: &str, b: &str) -> bool {
    match (authority_of(a), authority_of(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Find the credentials for a server by scheme and authority.
///
/// The credential store is keyed by full URL, but git only tells a helper
/// the scheme and the host (`https`, `example.com:8443`) — the path is
/// per-repository. So the match is on scheme + authority.
pub fn lookup_by_host(scheme: &str, host: &str) -> Result<Option<SiteCredential>> {
    let want = format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase());

    // The keyring has no "list" operation, so only the file fallback can
    // be searched. On a machine with a working keyring the caller falls
    // back to the default URL's own entry.
    let creds = read_credentials();
    for (url, site) in &creds.sites {
        if authority_of(url).as_deref() == Some(want.as_str()) {
            return Ok(Some(SiteCredential {
                url: url.clone(),
                username: site.username.clone(),
                token: site.token.clone(),
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that point `GITA_CONFIG_DIR` at a temp dir — the
    /// env var is process-global, and cargo runs tests in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_config_dir(run: impl FnOnce(&std::path::Path)) {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var("GITA_CONFIG_DIR", dir.path());
        run(dir.path());
        std::env::remove_var("GITA_CONFIG_DIR");
    }

    /// The keyring is best-effort, so a call must never take the process
    /// down with it. On a machine whose Secret Service is locked or whose
    /// kernel keyring is unavailable the call blocks; the deadline is what
    /// keeps the CLI alive.
    #[test]
    fn keyring_operations_are_bounded() {
        with_config_dir(|_| {
            let started = std::time::Instant::now();
            // Whether this ends up in the keyring or in the fallback file
            // is irrelevant — what matters is that it *returns*.
            let _ = set_secret("http://127.0.0.1:1/gita-timeout-test", "token");
            let elapsed = started.elapsed();
            assert!(
                elapsed < OP_TIMEOUT + Duration::from_secs(2),
                "keyring call took {elapsed:?}, expected it to give up within {OP_TIMEOUT:?}"
            );
        });
    }

    /// When the keyring cannot be used the token must not be lost: it
    /// degrades to the local encrypted file, which later reads pick up
    /// again. Losing the session after a successful login would make the
    /// CLI useless on any headless machine.
    #[test]
    fn tokens_fall_back_to_the_encrypted_file() {
        with_config_dir(|dir| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            let stored = set_secret("http://file-test", "tok").unwrap();
            assert_eq!(stored, Store::EncryptedFile);
            assert_eq!(
                get_secret("http://file-test").unwrap().as_deref(),
                Some("tok")
            );

            // The file is encrypted: the token must not appear in it.
            let on_disk = std::fs::read(credentials_file().unwrap()).unwrap();
            assert!(
                !on_disk.windows(3).any(|w| w == b"tok"),
                "the token must not be stored in plaintext"
            );

            // The file must be private.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = credentials_file().unwrap();
                let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "credentials.toml must be 0600");
                let dir_mode = std::fs::metadata(dir).unwrap().permissions().mode();
                assert_eq!(dir_mode & 0o777, 0o700, "the config dir must be 0700");
            }

            // Logout clears the file even with the keyring latched off.
            delete_secret("http://file-test").unwrap();
            assert_eq!(get_secret("http://file-test").unwrap(), None);
            KEYRING_UNAVAILABLE.store(false, Ordering::Relaxed);
        });
    }

    /// Username storage shares the encrypted fallback path.
    #[test]
    fn usernames_fall_back_to_the_encrypted_file() {
        with_config_dir(|_| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            let stored = set_username("http://file-test", "alice").unwrap();
            assert_eq!(stored, Store::EncryptedFile);
            assert_eq!(
                get_username("http://file-test").unwrap().as_deref(),
                Some("alice")
            );
            delete_secret("http://file-test").unwrap();
            assert_eq!(get_username("http://file-test").unwrap(), None);
            KEYRING_UNAVAILABLE.store(false, Ordering::Relaxed);
        });
    }

    /// `GITA_DISABLE_KEYRING` skips the keyring but the fallback file must
    /// still work — that switch exists precisely for machines without a
    /// usable keyring.
    #[test]
    fn disable_keyring_env_still_stores_to_the_file() {
        with_config_dir(|_| {
            std::env::set_var("GITA_DISABLE_KEYRING", "1");
            let stored = set_secret("http://env-test", "tok").unwrap();
            assert_eq!(stored, Store::EncryptedFile);
            assert_eq!(
                get_secret("http://env-test").unwrap().as_deref(),
                Some("tok")
            );
            delete_secret("http://env-test").unwrap();
            std::env::remove_var("GITA_DISABLE_KEYRING");
        });
    }

    /// A keyring failure recorded by an earlier run keeps the keyring out
    /// of the way — otherwise every command would pay the full deadline
    /// again to rediscover that the keyring blocks on this machine. The
    /// record expires, so a keyring that starts working is picked up.
    #[test]
    fn a_recorded_keyring_failure_skips_the_keyring() {
        with_config_dir(|dir| {
            assert!(!keyring_marked_broken(), "no marker to begin with");
            mark_keyring_broken();
            assert!(keyring_marked_broken());
            assert!(keyring_skipped(), "the keyring must be skipped");

            // An expired marker does not skip the keyring.
            std::fs::write(
                dir.join("keyring-unavailable"),
                now_secs().saturating_sub(1).to_string(),
            )
            .unwrap();
            assert!(!keyring_marked_broken());
        });
    }

    /// A pre-encryption plaintext store is picked up transparently, and
    /// the first write migrates it: the plaintext file disappears, the
    /// token lives on in the encrypted file, and nothing was lost.
    #[test]
    fn the_legacy_plaintext_store_is_migrated() {
        with_config_dir(|dir| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            std::fs::write(
                dir.join("credentials.toml"),
                "[sites.\"http://legacy-test\"]\ntoken = \"old-tok\"\nusername = \"old-user\"\n",
            )
            .unwrap();

            // Reads serve the legacy content as-is.
            assert_eq!(
                get_secret("http://legacy-test").unwrap().as_deref(),
                Some("old-tok")
            );
            assert!(dir.join("credentials.toml").exists(), "no write yet");

            // The first write moves everything into the encrypted file.
            set_secret("http://legacy-test", "new-tok").unwrap();
            assert!(
                !dir.join("credentials.toml").exists(),
                "the plaintext file must not survive the migration"
            );
            assert_eq!(
                get_secret("http://legacy-test").unwrap().as_deref(),
                Some("new-tok")
            );
            let on_disk = std::fs::read(dir.join("credentials.enc")).unwrap();
            assert!(
                !on_disk.windows(7).any(|w| w == b"new-tok"),
                "the migrated token must not be readable in the file"
            );
            KEYRING_UNAVAILABLE.store(false, Ordering::Relaxed);
        });
    }

    /// Deleting the encryption key must not leave the store claiming
    /// tokens it cannot read: reads return empty, so the caller falls
    /// back to a clean login instead of a broken session.
    #[test]
    fn a_lost_key_leaves_no_usable_tokens() {
        with_config_dir(|_| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            set_secret("http://lost-key-test", "tok").unwrap();
            assert_eq!(
                get_secret("http://lost-key-test").unwrap().as_deref(),
                Some("tok")
            );

            // The key is gone (deleted, or from another machine).
            std::fs::remove_file(key_file().unwrap()).unwrap();
            // A new key cannot decrypt the old blob.
            assert_eq!(get_secret("http://lost-key-test").unwrap(), None);
            KEYRING_UNAVAILABLE.store(false, Ordering::Relaxed);
        });
    }
}
