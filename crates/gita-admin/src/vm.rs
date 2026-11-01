//! VM cluster management (`gita-admin vm ...`).
//!
//! These commands talk to a running gita server's own VM control plane
//! (`/api/v1/vm/*`) — the same server that runs the CI jobs. The server
//! URL and the administrator token are resolved the way every other
//! client call is: the `--server-url` / `--token` flags first, then the
//! `GITA_URL` / `GITA_TOKEN` environment variables, then the client
//! config file, then the OS keyring.

use anyhow::{Context, Result};

use gita_client::api::GitaClient;
use gita_client::output;

pub use crate::config::VmCmd;

/// Parse a size string like `4G` / `512M` into bytes.
///
/// Binary units, matching the convention used everywhere else in gita
/// for memory and disk sizes.
fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, unit): (&str, u64) = if let Some(rest) = s.strip_suffix(['G', 'g']) {
        (rest, 1024 * 1024 * 1024)
    } else if let Some(rest) = s.strip_suffix(['M', 'm']) {
        (rest, 1024 * 1024)
    } else if let Some(rest) = s.strip_suffix(['K', 'k']) {
        (rest, 1024)
    } else {
        (s, 1)
    };
    let num: u64 = num
        .trim()
        .parse()
        .with_context(|| format!("invalid size value: {s}"))?;
    Ok(num * unit)
}

/// Build the client for a VM command.
///
/// An explicit `--server-url` / `--token` wins; otherwise the shared
/// client configuration is used (env vars, config file, keyring).
fn client(server_url: String, token: Option<String>) -> Result<GitaClient> {
    let config = gita_client::Config::load(None)?;
    // The flag has a default, so an explicit value always wins; the token
    // falls back to the shared configuration (env vars, config file,
    // keyring) when it was not given.
    let tok = token.or(config.token);
    GitaClient::new(server_url, tok)
}

/// Run a VM cluster command.
pub async fn run(cmd: VmCmd) -> Result<()> {
    match cmd {
        VmCmd::Templates { server_url, token } => {
            let client = client(server_url, token)?;
            let v = client.vm_list_templates().await?;
            let mut table = output::Table::new(&["id", "name", "version", "description"]);
            for t in &v.templates {
                table.add_row(vec![
                    t.id.to_string(),
                    t.name.clone(),
                    t.version.to_string(),
                    t.description.clone(),
                ]);
            }
            table.print();
        }
        VmCmd::Create {
            server_url,
            token,
            name,
            image,
            cpu,
            memory,
            disk,
            backend,
            description,
        } => {
            let memory_bytes = parse_size(&memory)?;
            let disk_bytes = parse_size(&disk)?;
            let spec = serde_json::json!({
                "cpu_millis": cpu * 1000,
                "memory_bytes": memory_bytes,
                "disk_bytes": disk_bytes,
                "image": { "name": image },
                "backend": backend,
            });
            let client = client(server_url, token)?;
            let t = client
                .vm_create_template(&name, &description, &spec)
                .await?;
            println!("Created VM template {} (id {})", t.name, t.id);
        }
        VmCmd::DeleteTemplate {
            server_url,
            token,
            id,
        } => {
            let client = client(server_url, token)?;
            client.vm_delete_template(id).await?;
            println!("Deleted VM template {id}");
        }
        VmCmd::Nodes { server_url, token } => {
            let client = client(server_url, token)?;
            let v = client.vm_list_nodes().await?;
            let mut table = output::Table::new(&["id", "name", "phase", "running_vms"]);
            for n in &v.nodes {
                let phase = n
                    .status
                    .get("phase")
                    .and_then(|p| p.as_str())
                    .unwrap_or("?")
                    .to_string();
                let running = n
                    .status
                    .get("running_vms")
                    .and_then(|r| r.as_i64())
                    .unwrap_or(0)
                    .to_string();
                table.add_row(vec![n.id.to_string(), n.name.clone(), phase, running]);
            }
            table.print();
        }
        VmCmd::Instances { server_url, token } => {
            let client = client(server_url, token)?;
            let v = client.vm_list_instances().await?;
            let mut table = output::Table::new(&["id", "name", "status", "node", "ip"]);
            for vm in &v.vms {
                table.add_row(vec![
                    vm.id.to_string(),
                    vm.name.clone(),
                    vm.status.clone(),
                    vm.node.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                    vm.ip_address.clone().unwrap_or_else(|| "-".into()),
                ]);
            }
            table.print();
        }
        VmCmd::Reclaim {
            server_url,
            token,
            id,
        } => {
            let client = client(server_url, token)?;
            client.vm_delete_instance(id).await?;
            println!("Reclaimed VM instance {id}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_binary_units() {
        assert_eq!(parse_size("4G").unwrap(), 4 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("65536K").unwrap(), 65536 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size(" 2g ").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn garbage_sizes_are_rejected() {
        assert!(parse_size("").is_err());
        assert!(parse_size("4X").is_err());
        assert!(parse_size("four").is_err());
    }
}
