//! Command handler for a CLI domain.

use anyhow::Result;

use gita_client::api::GitaClient;
use crate::args;
use gita_client::output;


pub(super) async fn search(cmd: args::SearchCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::SearchCommand::Repos { q } => {
            let hits = client.search_repos(&q).await?;
            if json {
                output::print_json(&hits)?;
            } else {
                let mut table = output::Table::new(&["full_name", "private"]);
                for h in &hits.hits {
                    table.add_row(vec![h.full_name.clone(), h.is_private.to_string()]);
                }
                table.print();
            }
        }
        args::SearchCommand::Code { q } => {
            let hits = client.search_code(&q).await?;
            if json {
                output::print_json(&hits)?;
            } else {
                let mut table = output::Table::new(&["repo", "path", "line", "content"]);
                for h in &hits.hits {
                    table.add_row(vec![
                        h.repo.clone(),
                        h.path.clone(),
                        h.line.to_string(),
                        h.content.clone(),
                    ]);
                }
                table.print();
            }
        }
    }
    Ok(())
}
