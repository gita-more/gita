//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn admin(cmd: args::AdminCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::AdminCommand::List => {
            let v = client.admin_list_users().await?;
            if json {
                output::print_json(&v)?;
            } else {
                let mut table = output::Table::new(&["id", "username", "email", "admin"]);
                for u in &v.users {
                    table.add_row(vec![
                        u.id.to_string(),
                        u.username.clone(),
                        u.email.clone(),
                        u.is_admin.to_string(),
                    ]);
                }
                table.print();
            }
        }
        args::AdminCommand::Create {
            username,
            email,
            password,
            admin,
        } => {
            let u = client
                .admin_create_user(&username, &email, &password, admin)
                .await?;
            if json {
                output::print_json(&u)?;
            } else {
                println!("Created user {} (admin: {})", u.username, u.is_admin);
            }
        }
        args::AdminCommand::SetAdmin { username, admin } => {
            let u = client
                .admin_update_user(&username, Some(admin), None)
                .await?;
            if json {
                output::print_json(&u)?;
            } else {
                println!("{} admin: {}", u.username, u.is_admin);
            }
        }
        args::AdminCommand::ResetPassword { username, password } => {
            let u = client
                .admin_update_user(&username, None, Some(&password))
                .await?;
            if json {
                output::print_json(&u)?;
            } else {
                println!("Password reset for {}", u.username);
            }
        }
        args::AdminCommand::Delete { username } => {
            let msg = client.admin_delete_user(&username).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
    }
    Ok(())
}
