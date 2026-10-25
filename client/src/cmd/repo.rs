//! Command handler for a CLI domain.

use anyhow::{bail, Result};

use crate::api::GitaClient;
use crate::args;
use crate::output;

use super::{clone_repo, print_repo, print_star, size_str};


pub(super) async fn repo(cmd: args::RepoCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::RepoCommand::List { owner } => {
            let repos = client.list_repos(owner.as_deref()).await?;
            if json {
                output::print_json(&repos)?;
            } else {
                let mut table = output::Table::new(&["owner", "name", "private", "branch"]);
                for r in &repos {
                    table.add_row(vec![
                        r.owner.clone(),
                        r.name.clone(),
                        r.is_private.to_string(),
                        r.default_branch.clone(),
                    ]);
                }
                table.print();
            }
        }
        args::RepoCommand::View { owner, repo } => {
            let r = client.get_repo(&owner, &repo).await?;
            print_repo(&r, json);
        }
        args::RepoCommand::Create {
            name,
            description,
            default_branch,
            private,
        } => {
            let r = client
                .create_repo(
                    &name,
                    description.as_deref(),
                    Some(private),
                    default_branch.as_deref(),
                )
                .await?;
            print_repo(&r, json);
        }
        args::RepoCommand::Edit {
            owner,
            repo,
            description,
            private,
            public,
            name,
            new_owner,
        } => {
            if private && public {
                bail!("cannot set both --private and --public");
            }
            let is_private = if private {
                Some(true)
            } else if public {
                Some(false)
            } else {
                None
            };
            let r = client
                .update_repo(
                    &owner,
                    &repo,
                    description.as_deref(),
                    is_private,
                    name.as_deref(),
                    new_owner.as_deref(),
                )
                .await?;
            print_repo(&r, json);
        }
        args::RepoCommand::Delete { owner, repo } => {
            let msg = client.delete_repo(&owner, &repo).await?;
            println!("{}", msg.message);
        }
        args::RepoCommand::Clone { repo, dir } => {
            clone_repo(client, &repo, dir.as_deref()).await?;
        }
        args::RepoCommand::Tree {
            owner,
            repo,
            path,
            r#ref,
        } => {
            let t = client.get_tree(&owner, &repo, &path, &r#ref).await?;
            if json {
                output::print_json(&t)?;
            } else {
                let mut table = output::Table::new(&["path", "kind", "size"]);
                for e in &t.entries {
                    table.add_row(vec![e.path.clone(), e.kind.clone(), size_str(&e.size)]);
                }
                table.print();
            }
        }
        args::RepoCommand::File {
            owner,
            repo,
            path,
            r#ref,
        } => {
            let f = client.get_file(&owner, &repo, &path, &r#ref).await?;
            if json {
                output::print_json(&f)?;
            } else {
                println!("{}", f.content);
            }
        }
        args::RepoCommand::Branches { owner, repo } => {
            let branches = client.get_branches(&owner, &repo).await?;
            if json {
                output::print_json(&branches)?;
            } else {
                let mut table = output::Table::new(&["branch"]);
                for b in &branches {
                    table.add_row(vec![b.clone()]);
                }
                table.print();
            }
        }
        args::RepoCommand::BranchCreate {
            owner,
            repo,
            name,
            r#ref,
        } => {
            let v = client
                .create_branch(&owner, &repo, &name, r#ref.as_deref())
                .await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("Created branch {}", v["branch"]);
            }
        }
        args::RepoCommand::BranchDelete {
            owner,
            repo,
            branch,
        } => {
            let msg = client.delete_branch(&owner, &repo, &branch).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
        args::RepoCommand::Tags { owner, repo } => {
            let tags = client.get_tags(&owner, &repo).await?;
            if json {
                output::print_json(&tags)?;
            } else {
                let mut table = output::Table::new(&["tag"]);
                for t in &tags {
                    table.add_row(vec![t.clone()]);
                }
                table.print();
            }
        }
        args::RepoCommand::TagCreate {
            owner,
            repo,
            name,
            r#ref,
        } => {
            let v = client
                .create_tag(&owner, &repo, &name, r#ref.as_deref())
                .await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("Created tag {}", v["tag"]);
            }
        }
        args::RepoCommand::TagDelete { owner, repo, tag } => {
            let msg = client.delete_tag(&owner, &repo, &tag).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
        args::RepoCommand::Commit { owner, repo, sha } => {
            let v: serde_json::Value = client.get_commit(&owner, &repo, &sha).await?;
            if json {
                output::print_json(&v)?;
            } else {
                let commit = &v["commit"];
                println!("sha: {}", commit["sha"]);
                println!(
                    "author: {} <{}>",
                    commit["author_name"], commit["author_email"]
                );
                println!("message: {}", commit["message"]);
                println!("diff:\n{}", v["diff"]);
            }
        }
        args::RepoCommand::Commits {
            owner,
            repo,
            r#ref,
            limit,
        } => {
            let c = client
                .get_commits(&owner, &repo, r#ref.as_deref(), limit)
                .await?;
            if json {
                output::print_json(&c)?;
            } else {
                let mut table = output::Table::new(&["sha", "author", "message"]);
                for c in &c.commits {
                    table.add_row(vec![
                        c.sha.clone(),
                        c.author_name.clone(),
                        c.message.clone(),
                    ]);
                }
                table.print();
            }
        }
        args::RepoCommand::Star { owner, repo } => {
            let s = client.star_repo(&owner, &repo).await?;
            print_star(&s);
        }
        args::RepoCommand::Unstar { owner, repo } => {
            let s = client.unstar_repo(&owner, &repo).await?;
            print_star(&s);
        }
        args::RepoCommand::StarStatus { owner, repo } => {
            let s = client.get_star(&owner, &repo).await?;
            if json {
                output::print_json(&s)?;
            } else {
                println!("starred: {} (count: {})", s.starred, s.count);
            }
        }
        args::RepoCommand::PagesShow { owner, repo } => {
            let v = client.get_pages(&owner, &repo).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        args::RepoCommand::PagesStatus { owner, repo } => {
            let v = client.get_pages_status(&owner, &repo).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        args::RepoCommand::PagesEnable {
            owner,
            repo,
            branch,
            path,
        } => {
            let v = client
                .put_pages(&owner, &repo, true, branch.as_deref(), &path)
                .await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("Pages enabled: {}", serde_json::to_string_pretty(&v)?);
            }
        }
        args::RepoCommand::PagesDisable { owner, repo } => {
            let msg = client.delete_pages(&owner, &repo).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
        args::RepoCommand::Hooks { owner, repo } => {
            let resp = client.list_hooks(&owner, &repo).await?;
            if json {
                output::print_json(&resp)?;
            } else {
                let mut table = output::Table::new(&["id", "url", "events", "active"]);
                for h in &resp.hooks {
                    table.add_row(vec![
                        h.id.to_string(),
                        h.url.clone(),
                        h.events.join(","),
                        h.active.to_string(),
                    ]);
                }
                table.print();
            }
        }
        args::RepoCommand::HookCreate {
            owner,
            repo,
            webhook_url,
            events,
            secret,
            inactive,
        } => {
            let events_vec = events.as_deref().map(|e| {
                e.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            });
            let h = client
                .create_hook(
                    &owner,
                    &repo,
                    &webhook_url,
                    events_vec,
                    secret.as_deref(),
                    Some(!inactive),
                )
                .await?;
            if json {
                output::print_json(&h)?;
            } else {
                println!("Created hook #{} for {}", h.id, webhook_url);
            }
        }
        args::RepoCommand::HookDelete { owner, repo, id } => {
            let msg = client.delete_hook(&owner, &repo, id).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
        args::RepoCommand::Fork { owner, repo, name } => {
            let r = client.fork_repo(&owner, &repo, name.as_deref()).await?;
            print_repo(&r, json);
        }
    }
    Ok(())
}
