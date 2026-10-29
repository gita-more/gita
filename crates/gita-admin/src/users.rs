//! Administrator user management (`gita-admin users ...`).
//!
//! These commands talk to a running server over the REST API using the
//! shared [`gita_client`] transport. The server URL and the admin token
//! are resolved like every other client call: `GITA_URL` / `GITA_TOKEN`
//! environment variables first, then the client config file, then the
//! OS keyring.

use anyhow::Result;
use clap::Subcommand;

use gita_client::api::GitaClient;
use gita_client::output;

/// User-management subcommands.
#[derive(Subcommand, Debug)]
pub enum UsersCmd {
    /// List all registered users.
    List,
    /// Create a user account.
    Create {
        username: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
        /// Grant administrator privileges to the new account.
        #[arg(long)]
        admin: bool,
    },
    /// Grant or revoke a user's administrator role.
    SetAdmin {
        username: String,
        /// Bare `--admin` grants the role; `--admin=false` revokes it.
        #[arg(
            long,
            action = clap::ArgAction::Set,
            num_args = 0..=1,
            default_missing_value = "true",
            default_value_t = true
        )]
        admin: bool,
    },
    /// Reset a user's password without knowing the old one.
    ResetPassword {
        username: String,
        #[arg(long)]
        password: String,
    },
    /// Delete a user, their personal repositories and their owned groups.
    Delete { username: String },
}

/// Run a user-management command.
pub async fn run(cmd: UsersCmd) -> Result<()> {
    let config = gita_client::Config::load(None)?;
    let client = GitaClient::new(config.server_url, config.token.clone())?;

    match cmd {
        UsersCmd::List => {
            let v = client.admin_list_users().await?;
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
        UsersCmd::Create {
            username,
            email,
            password,
            admin,
        } => {
            let u = client
                .admin_create_user(&username, &email, &password, admin)
                .await?;
            println!("Created user {} (admin: {})", u.username, u.is_admin);
        }
        UsersCmd::SetAdmin { username, admin } => {
            let u = client
                .admin_update_user(&username, Some(admin), None)
                .await?;
            println!("{} admin: {}", u.username, u.is_admin);
        }
        UsersCmd::ResetPassword { username, password } => {
            client
                .admin_update_user(&username, None, Some(&password))
                .await?;
            println!("Password reset for {}", username);
        }
        UsersCmd::Delete { username } => {
            let msg = client.admin_delete_user(&username).await?;
            println!("{}", msg.message);
        }
    }
    Ok(())
}
