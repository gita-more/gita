use anyhow::Result;

pub async fn run(cmd: crate::config::RunnerCmd) -> Result<()> {
    match cmd {
        crate::config::RunnerCmd::Register { name, user } => {
            register_runner(&name, &user).await
        }
    }
}

async fn register_runner(name: &str, user: &str) -> Result<()> {
    // This is actually handled in main.rs worker_register
    // Just a placeholder for CLI structure
    println!("Runner registration is done via: gita-admin worker register --name {} --user {}", name, user);
    Ok(())
}