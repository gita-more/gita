//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn event(cmd: args::EventCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::EventCommand::List {
            limit,
            repo,
            r#type,
        } => {
            let events = client
                .list_events(limit, repo.as_deref(), r#type.as_deref())
                .await?;
            if json {
                output::print_json(&events)?;
            } else {
                let mut table = output::Table::new(&["id", "type", "repo", "actor", "created_at"]);
                for e in &events.events {
                    table.add_row(vec![
                        e.id.to_string(),
                        e.event_type.clone(),
                        e.repo_id.map(|r| r.to_string()).unwrap_or("-".to_string()),
                        e.actor_id.map(|a| a.to_string()).unwrap_or("-".to_string()),
                        e.created_at.clone(),
                    ]);
                }
                table.print();
            }
        }
    }
    Ok(())
}
