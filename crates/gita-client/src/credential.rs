//! Credential store: OS keyring with a plaintext-file fallback.
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
//! paying the timeout again.
//!
//! ## The plaintext fallback
//!
//! A keyring that cannot store the token must not lose the session: the
//! CLI would happily log in and then be anonymous again on the next
//! command. So when the keyring is unavailable (missing, latched off, or
//! disabled via `GITA_DISABLE_KEYRING=1`) the token degrades to
//! `~/.gita/credentials.toml`, written with `0600` permissions. The caller
//! is told where the secret ended up and must surface it — silently
//! writing a plaintext secret is a decision the user gets to make.
//!
//! Reads prefer the keyring and fall back to the file; `delete_secret`
//! clears both. `GITA_DISABLE_KEYRING` only switches off the keyring — the
//! file fallback is exactly the answer to that situation.
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
    /// The plaintext credentials file (the keyring was unavailable).
    PlaintextFile,
}

/// Path of the plaintext credential fallback, for caller-facing messages.
pub fn credentials_file() -> Result<PathBuf> {
    config_dir()
        .map(|d| d.join("credentials.toml"))
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
            bail!("keyring operation timed out after {}s", OP_TIMEOUT.as_secs());
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("keyring operation failed: the backend exited unexpectedly");
        }
    }
}

/// Whether a keyring read attempt should even be made.
fn keyring_skipped() -> bool {
    disabled_by_env() || KEYRING_UNAVAILABLE.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Plaintext credentials file
// ---------------------------------------------------------------------------

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

fn read_credentials() -> Credentials {
    let Ok(path) = credentials_file() else {
        return Credentials::default();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Credentials::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

/// Write the credentials file with `0600` (and its directory `0700`).
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
    let toml = toml::to_string_pretty(creds).context("serialize credentials")?;
    std::fs::write(&path, toml).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
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
/// the token falls back to the plaintext credentials file — and the
/// returned [`Store`] tells the caller so it can say where the secret
/// landed.
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
    Ok(Store::PlaintextFile)
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
    Ok(Store::PlaintextFile)
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
    /// degrades to the plaintext credentials file, which later reads pick
    /// up again. Losing the session after a successful login would make
    /// the CLI useless on any headless machine.
    #[test]
    fn tokens_fall_back_to_the_plaintext_file() {
        with_config_dir(|dir| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            let stored = set_secret("http://file-test", "tok").unwrap();
            assert_eq!(stored, Store::PlaintextFile);
            assert_eq!(
                get_secret("http://file-test").unwrap().as_deref(),
                Some("tok")
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

    /// Username storage shares the fallback path.
    #[test]
    fn usernames_fall_back_to_the_plaintext_file() {
        with_config_dir(|_| {
            KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
            let stored = set_username("http://file-test", "alice").unwrap();
            assert_eq!(stored, Store::PlaintextFile);
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
            assert_eq!(stored, Store::PlaintextFile);
            assert_eq!(
                get_secret("http://env-test").unwrap().as_deref(),
                Some("tok")
            );
            delete_secret("http://env-test").unwrap();
            std::env::remove_var("GITA_DISABLE_KEYRING");
        });
    }
}
