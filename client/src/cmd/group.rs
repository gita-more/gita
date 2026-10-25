//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;

use super::print_repo;


pub(super) async fn group(cmd: args::GroupCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::GroupCommand::List => {
            let groups = client.list_groups().await?;
            if json {
                output::print_json(&groups)?;
            } else {
                let mut table = output::Table::new(&["name", "description"]);
                for g in &groups {
                    table.add_row(vec![g.name.clone(), g.description.clone()]);
                }
                table.print();
            }
        }
        args::GroupCommand::Create { name, description } => {
            let g = client.create_group(&name, description.as_deref()).await?;
            if json {
                output::print_json(&g)?;
            } else {
                println!("Created group {}", g.name);
            }
        }
        args::GroupCommand::View { name } => {
            let v = client.get_group(&name).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("group: {}", v.group.name);
                println!("description: {}", v.group.description);
                println!("repositories:");
                for r in &v.repos {
                    println!("  - {}/{}", r.owner, r.name);
                }
            }
        }
        args::GroupCommand::Members { name } => {
            let v: serde_json::Value = client.list_group_members(&name).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("{v}");
            }
        }
        args::GroupCommand::MemberAdd {
            group,
            username,
            role,
        } => {
            let v = client
                .add_group_member(&group, &username, role.as_deref())
                .await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!(
                    "Added {username} to {group} as {}",
                    v["role"].as_str().unwrap_or("member")
                );
            }
        }
        args::GroupCommand::MemberRemove { group, username } => {
            let msg = client.remove_group_member(&group, &username).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
        args::GroupCommand::MemberSetRole {
            group,
            username,
            role,
        } => {
            let v = client.update_group_member(&group, &username, &role).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("Set {username} role in {group} to {}", v["role"]);
            }
        }
        args::GroupCommand::Repo {
            group,
            name,
            description,
            private,
            default_branch,
        } => {
            let r = client
                .create_group_repo(
                    &group,
                    &name,
                    description.as_deref(),
                    Some(private),
                    default_branch.as_deref(),
                )
                .await?;
            print_repo(&r, json);
        }
    }
    Ok(())
}
