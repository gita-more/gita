//! OS keyring credential store.
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
//! degrade to "no stored token", not to an unkillable process. So every
//! operation runs on a worker thread with a hard deadline, and the first
//! timeout latches the keyring off for the rest of the process so the
//! second call fails immediately instead of paying the timeout again.
//!
//! Set `GITA_DISABLE_KEYRING=1` to skip the keyring entirely.

use anyhow::{bail, Context, Result};
use keyring::Entry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

const SERVICE: &str = "gita";

/// Upper bound for a single keyring operation.
///
/// A healthy keyring answers in milliseconds; this only ever bounds the
/// pathological blocking case, so it can be generous.
const OP_TIMEOUT: Duration = Duration::from_secs(3);

/// Latched once an operation has timed out. Later calls fail immediately
/// instead of blocking for another `OP_TIMEOUT`.
static KEYRING_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

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

/// Bail out early if the keyring is known to be unusable.
fn ensure_usable() -> Result<()> {
    if disabled_by_env() {
        bail!("keyring disabled via GITA_DISABLE_KEYRING");
    }
    if KEYRING_UNAVAILABLE.load(Ordering::Relaxed) {
        bail!("keyring is unavailable in this environment");
    }
    Ok(())
}

/// Read the stored token for `url`. Returns `Ok(None)` when nothing is
/// stored (e.g. the Secret Service is not running on this machine).
pub fn get_secret(url: &str) -> Result<Option<String>> {
    if disabled_by_env() {
        return Ok(None);
    }
    ensure_usable()?;
    let url = url.to_string();
    run_bounded(move || match Entry::new(SERVICE, &account(&url))?.get_password() {
        Ok(token) => Ok(Some(token)),
        // "No entry" means the keyring works — it just has nothing for us.
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => bail!("keyring read failed for {url}: {e}"),
    })
}

/// Store `token` for `url`. Any previous token is replaced.
pub fn set_secret(url: &str, token: &str) -> Result<()> {
    ensure_usable()?;
    let url = url.to_string();
    let token = token.to_string();
    run_bounded(move || {
        Entry::new(SERVICE, &account(&url))?
            .set_password(&token)
            .with_context(|| format!("keyring write failed for {url}"))
    })
}

/// Store the authenticated username for `url`.
pub fn set_username(url: &str, username: &str) -> Result<()> {
    ensure_usable()?;
    let url = url.to_string();
    let username = username.to_string();
    run_bounded(move || {
        Entry::new(SERVICE, &username_account(&url))?
            .set_password(&username)
            .with_context(|| format!("keyring write failed for {url}#username"))
    })
}

/// Read the stored username for `url`.
pub fn get_username(url: &str) -> Result<Option<String>> {
    if disabled_by_env() {
        return Ok(None);
    }
    ensure_usable()?;
    let url = url.to_string();
    run_bounded(move || match Entry::new(SERVICE, &username_account(&url))?.get_password() {
        Ok(username) => Ok(Some(username)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => bail!("keyring read failed for {url}#username: {e}"),
    })
}

/// Remove the stored token and username for `url`.
///
/// Best-effort by design: a keyring we cannot reach cannot be cleared
/// either, and the local session is gone either way.
pub fn delete_secret(url: &str) -> Result<()> {
    if disabled_by_env() {
        return Ok(());
    }
    if KEYRING_UNAVAILABLE.load(Ordering::Relaxed) {
        return Ok(());
    }
    let url = url.to_string();
    let _ = run_bounded(move || {
        let _ = Entry::new(SERVICE, &account(&url))?.delete_credential();
        let _ = Entry::new(SERVICE, &username_account(&url))?.delete_credential();
        Ok::<(), anyhow::Error>(())
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyring is best-effort, so a call must never take the process
    /// down with it. On a machine whose Secret Service is locked or whose
    /// kernel keyring is unavailable the call blocks; the deadline is what
    /// keeps the CLI alive.
    #[test]
    fn keyring_operations_are_bounded() {
        let started = std::time::Instant::now();
        // Whether this succeeds, fails or blocks is irrelevant — what
        // matters is that it *returns*.
        let _ = set_secret("http://127.0.0.1:1/gita-timeout-test", "token");
        let elapsed = started.elapsed();
        assert!(
            elapsed < OP_TIMEOUT + Duration::from_secs(2),
            "keyring call took {elapsed:?}, expected it to give up within {OP_TIMEOUT:?}"
        );
    }

    /// After one timeout the keyring is latched off, so a second call
    /// returns immediately rather than blocking for another deadline.
    #[test]
    fn a_timeout_latches_the_keyring_off() {
        KEYRING_UNAVAILABLE.store(true, Ordering::Relaxed);
        let started = std::time::Instant::now();
        let err = set_secret("http://127.0.0.1:1/gita-latch-test", "token")
            .expect_err("latched keyring must refuse to store");
        assert!(err.to_string().contains("unavailable"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "latched call must fail fast"
        );
        KEYRING_UNAVAILABLE.store(false, Ordering::Relaxed);
    }
}
