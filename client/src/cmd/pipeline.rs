//! Command handler for a CLI domain.

use anyhow::Result;

use crate::api::GitaClient;
use crate::args;
use crate::output;


pub(super) async fn pipeline(cmd: args::PipelineCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::PipelineCommand::List { owner, repo } => {
            let v: serde_json::Value = client.list_pipelines(&owner, &repo).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("{v}");
            }
        }
        args::PipelineCommand::Runs { owner, repo, path } => {
            let runs = client.list_runs(&owner, &repo, &path).await?;
            if json {
                output::print_json(&runs)?;
            } else {
                let mut table = output::Table::new(&["run", "ref", "status"]);
                for r in &runs.runs {
                    table.add_row(vec![r.id.to_string(), r.r#ref.clone(), r.status.clone()]);
                }
                table.print();
            }
        }
        args::PipelineCommand::Trigger { owner, repo, path } => {
            let v = client.trigger_run(&owner, &repo, &path).await?;
            println!("triggered run: {v}");
        }
        args::PipelineCommand::Logs {
            owner,
            repo,
            run,
            job,
            since,
        } => {
            let logs = client.get_job_logs(&owner, &repo, run, job, since).await?;
            if json {
                output::print_json(&logs)?;
            } else {
                for line in &logs.lines {
                    println!("{} {}", line.ts, line.line);
                }
            }
        }
        args::PipelineCommand::Cancel { owner, repo, run } => {
            let msg = client.cancel_run(&owner, &repo, run).await?;
            println!("{}", msg.message);
        }
    }
    Ok(())
}
