//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn issue(cmd: args::IssueCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::IssueCommand::List {
            owner,
            repo,
            all,
            status,
        } => {
            if all {
                let resp = client.list_all_issues(status.as_deref()).await?;
                if json {
                    output::print_json(&resp)?;
                } else {
                    let mut table =
                        output::Table::new(&["repo", "number", "title", "status", "author"]);
                    for i in &resp.issues {
                        table.add_row(vec![
                            i.full_name.clone(),
                            i.number.to_string(),
                            i.title.clone(),
                            i.status.clone(),
                            i.author.clone(),
                        ]);
                    }
                    table.print();
                }
            } else {
                let owner = owner
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("owner is required unless --all is given"))?;
                let repo = repo
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("repo is required unless --all is given"))?;
                let v = serde_json::to_value(
                    client.list_issues(owner, repo, status.as_deref()).await?,
                )?;
                if json {
                    output::print_json(&v)?;
                } else {
                    let issues = &v["issues"];
                    let mut table = output::Table::new(&["number", "title", "status", "author"]);
                    for i in issues.as_array().unwrap_or(&vec![]) {
                        table.add_row(vec![
                            i["number"].to_string(),
                            i["title"].to_string(),
                            i["status"].to_string(),
                            i["author"].to_string(),
                        ]);
                    }
                    table.print();
                }
            }
        }
        args::IssueCommand::View {
            owner,
            repo,
            number,
        } => {
            let issue = client.get_issue(&owner, &repo, number).await?;
            if json {
                output::print_json(&issue)?;
            } else {
                println!("Issue #{}: {}", issue.number, issue.title);
                println!("status: {} by {}", issue.status, issue.author);
                println!("body: {}", issue.body);
            }
        }
        args::IssueCommand::Create {
            owner,
            repo,
            title,
            body,
        } => {
            let issue = client
                .create_issue(&owner, &repo, &title, body.as_deref())
                .await?;
            if json {
                output::print_json(&issue)?;
            } else {
                println!("Created issue #{}: {}", issue.number, issue.title);
            }
        }
        args::IssueCommand::Comment {
            owner,
            repo,
            number,
            body,
        } => {
            let c = client
                .create_issue_comment(&owner, &repo, number, &body)
                .await?;
            if json {
                output::print_json(&c)?;
            } else {
                println!("Commented on issue #{}: {}", number, c.body);
            }
        }
        args::IssueCommand::Close {
            owner,
            repo,
            number,
        } => {
            let msg = client.close_issue(&owner, &repo, number).await?;
            println!("{}", msg.message);
        }
    }
    Ok(())
}
