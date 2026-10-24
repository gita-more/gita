//! OS keyring credential store.
//!
//! The token is stored under service name `"gita"` with the account set to
//! the canonical server URL (e.g. `http://127.0.0.1:3000`), so multiple
//! server profiles can coexist. The username is stored under
//! `{url}#username` for git smart HTTP clone URLs.
//!
//! On Linux this uses Secret Service (zbus-based, no C dependencies) via
//! the `sync-secret-service` feature of the `keyring` crate.

use anyhow::{bail, Context, Result};
use keyring::Entry;

const SERVICE: &str = "gita";

fn account(url: &str) -> String {
    url.to_string()
}

fn username_account(url: &str) -> String {
    format!("{url}#username")
}

/// Read the stored token for `url`. Returns `Ok(None)` when nothing is
/// stored (e.g. the Secret Service is not running on this machine).
pub fn get_secret(url: &str) -> Result<Option<String>> {
    match Entry::new(SERVICE, &account(url))?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => bail!("keyring read failed for {url}: {e}"),
    }
}

/// Store `token` for `url`. Any previous token is replaced.
pub fn set_secret(url: &str, token: &str) -> Result<()> {
    Entry::new(SERVICE, &account(url))?
        .set_password(token)
        .with_context(|| format!("keyring write failed for {url}"))
}

/// Store the authenticated username for `url`.
pub fn set_username(url: &str, username: &str) -> Result<()> {
    Entry::new(SERVICE, &username_account(url))?
        .set_password(username)
        .with_context(|| format!("keyring write failed for {url}#username"))
}

/// Read the stored username for `url`.
pub fn get_username(url: &str) -> Result<Option<String>> {
    match Entry::new(SERVICE, &username_account(url))?.get_password() {
        Ok(username) => Ok(Some(username)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => bail!("keyring read failed for {url}#username: {e}"),
    }
}

/// Remove the stored token and username for `url`.
pub fn delete_secret(url: &str) -> Result<()> {
    let _ = Entry::new(SERVICE, &account(url))?.delete_credential();
    let _ = Entry::new(SERVICE, &username_account(url))?.delete_credential();
    Ok(())
}
