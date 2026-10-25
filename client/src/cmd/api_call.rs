//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::output;


pub(super) async fn call_api(
    client: &GitaClient,
    path: &str,
    method: &str,
    body: Option<&str>,
    json: bool,
) -> Result<()> {
    // Paths may be passed with or without the `/api` prefix; normalise.
    let path = path.trim_start_matches('/');
    let path = if path.starts_with("api/") {
        format!("/{path}")
    } else {
        format!("/api/{path}")
    };

    let body_value = match body {
        Some(b) => {
            Some(serde_json::from_str(b).map_err(|e| anyhow::anyhow!("invalid --body JSON: {e}"))?)
        }
        None => None,
    };

    let v = client.api(method, &path, body_value).await?;
    if json {
        output::print_json(&v)?;
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}
