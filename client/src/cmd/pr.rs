//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn pr(cmd: args::PrCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::PrCommand::List {
            owner,
            repo,
            status,
        } => {
            let v: serde_json::Value =
                serde_json::to_value(client.list_prs(&owner, &repo, status.as_deref()).await?)?;
            if json {
                output::print_json(&v)?;
            } else {
                let prs = &v["prs"];
                let mut table = output::Table::new(&["number", "title", "status", "author"]);
                for p in prs.as_array().unwrap_or(&vec![]) {
                    table.add_row(vec![
                        p["number"].to_string(),
                        p["title"].to_string(),
                        p["status"].to_string(),
                        p["author"].to_string(),
                    ]);
                }
                table.print();
            }
        }
        args::PrCommand::View {
            owner,
            repo,
            number,
        } => {
            let pr = client.get_pr(&owner, &repo, number).await?;
            if json {
                output::print_json(&pr)?;
            } else {
                println!("PR #{}: {}", pr.number, pr.title);
                println!("status: {} by {}", pr.status, pr.author);
                println!(
                    "{source} -> {target}",
                    source = pr.source_branch,
                    target = pr.target_branch
                );
                println!("body: {}", pr.body);
            }
        }
        args::PrCommand::Create {
            owner,
            repo,
            title,
            body,
            source_branch,
            target_branch,
        } => {
            let pr = client
                .create_pr(
                    &owner,
                    &repo,
                    &title,
                    body.as_deref(),
                    &source_branch,
                    target_branch.as_deref(),
                )
                .await?;
            if json {
                output::print_json(&pr)?;
            } else {
                println!("Created PR #{}: {}", pr.number, pr.title);
            }
        }
        args::PrCommand::Comment {
            owner,
            repo,
            number,
            body,
        } => {
            let c = client
                .create_pr_comment(&owner, &repo, number, &body)
                .await?;
            if json {
                output::print_json(&c)?;
            } else {
                println!("Commented on PR #{}: {}", number, c.body);
            }
        }
        args::PrCommand::Merge {
            owner,
            repo,
            number,
        } => {
            let v = client.merge_pr(&owner, &repo, number).await?;
            println!("merged: {}", v["merge_sha"]);
        }
        args::PrCommand::Close {
            owner,
            repo,
            number,
        } => {
            let msg = client.close_pr(&owner, &repo, number).await?;
            println!("{}", msg.message);
        }
        args::PrCommand::Diff {
            owner,
            repo,
            number,
        } => {
            let v = client.get_pr_diff(&owner, &repo, number).await?;
            println!("{}", v["diff"]);
        }
    }
    Ok(())
}
