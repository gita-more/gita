//! Command handler for a CLI domain.

use anyhow::Result;

use gita_client::api::GitaClient;
use gita_client::output;


pub(super) async fn call_api(
    client: &GitaClient,
    path: &str,
    method: &str,
    body: Option<&str>,
    json: bool,
) -> Result<()> {
    let path = normalize_api_path(path);

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

/// The path a user typed, turned into the path the API answers on.
///
/// Paths may be passed with or without the `/api` prefix, and with or
/// without the leading slash; all of it has to land on `/api/...`.
fn normalize_api_path(path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.starts_with("api/") {
        format!("/{path}")
    } else {
        format!("/api/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_api_path;

    #[test]
    fn a_bare_path_gets_the_api_prefix() {
        assert_eq!(normalize_api_path("user"), "/api/user");
        assert_eq!(normalize_api_path("/user"), "/api/user");
    }

    /// A user who already typed the prefix must not end up with two.
    #[test]
    fn a_path_with_the_prefix_is_left_alone() {
        assert_eq!(normalize_api_path("api/user"), "/api/user");
        assert_eq!(normalize_api_path("/api/user"), "/api/user");
    }

    #[test]
    fn a_nested_path_is_preserved() {
        assert_eq!(
            normalize_api_path("/repos/alice/widgets.git"),
            "/api/repos/alice/widgets.git"
        );
    }
}
