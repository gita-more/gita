//! Runner registration.

use anyhow::Result;

use crate::worker_register;
use crate::Config;

pub async fn run(cmd: crate::config::RunnerCmd, config: &Config) -> Result<()> {
    match cmd {
        crate::config::RunnerCmd::Register { name, user } => {
            let token = worker_register(config, &name, &user).await?;
            println!("runner token: {token}");
            println!("export GITA_RUNNER_TOKEN={token}");
            Ok(())
        }
    }
}
