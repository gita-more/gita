//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn ssh_key(cmd: args::SshKeyCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::SshKeyCommand::Add { title, key } => {
            // Allow `--title` and a key that is either inline content or a
            // path to an OpenSSH `.pub` file.
            let content = std::fs::read_to_string(&key).unwrap_or_else(|_| key.clone());
            let k = client.add_ssh_key(content.trim(), title.as_deref()).await?;
            if json {
                output::print_json(&k)?;
            } else {
                println!("Added SSH key #{}: {}", k.id, k.title);
            }
        }
        args::SshKeyCommand::List => {
            let resp = client.list_ssh_keys().await?;
            if json {
                output::print_json(&resp)?;
            } else {
                let mut table = output::Table::new(&["id", "title", "public_key", "created_at"]);
                for k in &resp.keys {
                    table.add_row(vec![
                        k.id.to_string(),
                        k.title.clone(),
                        k.public_key.clone(),
                        k.created_at.clone(),
                    ]);
                }
                table.print();
            }
        }
        args::SshKeyCommand::Delete { id } => {
            let msg = client.delete_ssh_key(id).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
    }
    Ok(())
}
