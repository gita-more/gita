//! Command handlers: each group maps `args::Command` variants to API calls
//! plus output formatting. Lives separately from `lib.rs` so the dispatch
//! stays readable.

use anyhow::{bail, Result};

use crate::api::GitaClient;
use crate::args;
use crate::credential;
use crate::output;

pub async fn dispatch(cmd: args::Command, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::Command::Auth { cmd } => crate::auth::dispatch(cmd, client, json).await,
        args::Command::Repo { cmd } => repo(cmd, client, json).await,
        args::Command::Pr { cmd } => pr(cmd, client, json).await,
        args::Command::Issue { cmd } => issue(cmd, client, json).await,
        args::Command::Group { cmd } => group(cmd, client, json).await,
        args::Command::Search { cmd } => search(cmd, client, json).await,
        args::Command::Pipeline { cmd } => pipeline(cmd, client, json).await,
        args::Command::SshKey { cmd } => ssh_key(cmd, client, json).await,
        args::Command::Event { cmd } => event(cmd, client, json).await,
        args::Command::Api { path, method, body } => {
            call_api(client, &path, &method, body.as_deref(), json).await
        }
        args::Command::Config { cmd } => config(cmd, client, json),
    }
}

// ---------------------------------------------------------------------------
// repos
// ---------------------------------------------------------------------------

async fn repo(cmd: args::RepoCommand, client: &GitaClient, json: bool) -> Result<()> {
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
            let v = client.create_branch(&owner, &repo, &name, r#ref.as_deref()).await?;
            if json {
                output::print_json(&v)?;
            } else {
                println!("Created branch {}", v["branch"]);
            }
        }
        args::RepoCommand::BranchDelete { owner, repo, branch } => {
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
            let v = client.create_tag(&owner, &repo, &name, r#ref.as_deref()).await?;
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
                println!(
                    "starred: {} (count: {})",
                    s.starred, s.count
                );
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

fn print_repo(r: &crate::api::types::Repository, json: bool) {
    if json {
        // best effort; ignore serialize error
        let _ = output::print_json(r);
    } else {
        println!("full_name: {}", r.full_name);
        println!("description: {}", r.description);
        println!("private: {}", r.is_private);
        println!("default_branch: {}", r.default_branch);
        println!("created_at: {}", r.created_at);
    }
}

fn print_star(s: &crate::api::types::StarResponse) {
    println!(
        "starred: {} (count: {})",
        s.starred,
        s.new
            .map(|n| if n { "added" } else { "already starred" })
            .unwrap_or("unchanged")
    );
}

fn size_str(size: &Option<i64>) -> String {
    match size {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}

async fn clone_repo(client: &GitaClient, repo: &str, dir: Option<&std::path::Path>) -> Result<()> {
    // repo = "owner/name". Extract username + token for Basic-auth clone URL.
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("repo must be 'owner/name', got {repo}"))?;

    let (username, token) = match &client.token {
        Some(token) => (credential_username(client, owner)?, token.clone()),
        None => (String::new(), String::new()),
    };

    let origin = origin_of(&client.base_url)?;
    let (scheme, host) = origin.split_once("://").unwrap_or(("", &origin));
    let url = if token.is_empty() {
        format!("{scheme}://{host}/git/{owner}/{name}.git")
    } else {
        format!("{scheme}://{username}:{token}@{host}/git/{owner}/{name}.git")
    };

    let mut git = std::process::Command::new("git");
    git.arg("clone").arg(&url);
    // Self-signed certs are common for local/test servers; skip TLS verification
    // when not using https.
    if scheme == "http" {
        git.env("GIT_SSL_NO_VERIFY", "1");
    }
    if let Some(d) = dir {
        git.arg(d);
    }
    let status = git.status()?;
    if !status.success() {
        bail!("git clone failed with exit code {:?}", status.code());
    }
    let dest = dir
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    println!("Cloned {repo} into {dest}");
    Ok(())
}

fn origin_of(base_url: &str) -> Result<String> {
    let u: url::Url = base_url
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid base URL {base_url}: {e}"))?;
    let host = u
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("no host in {base_url}"))?;
    let port = u.port_or_known_default();
    match port {
        Some(p) if p == 80 && u.scheme() == "http" => Ok(format!("{}://{}", u.scheme(), host)),
        Some(p) if p == 443 && u.scheme() == "https" => Ok(format!("{}://{}", u.scheme(), host)),
        Some(p) => Ok(format!("{}://{}:{}", u.scheme(), host, p)),
        None => Ok(format!("{}://{}", u.scheme(), host)),
    }
}

fn credential_username(client: &GitaClient, owner: &str) -> Result<String> {
    // Prefer the stored username; fall back to the repo owner path component.
    // Keyring reads are best-effort (no Secret Service in CI).
    if let Ok(Some(u)) = credential::get_username(&client.base_url) {
        return Ok(u);
    }
    Ok(owner.to_string())
}

// ---------------------------------------------------------------------------
// pull requests
// ---------------------------------------------------------------------------

async fn pr(cmd: args::PrCommand, client: &GitaClient, json: bool) -> Result<()> {
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

// ---------------------------------------------------------------------------
// issues
// ---------------------------------------------------------------------------

async fn issue(cmd: args::IssueCommand, client: &GitaClient, json: bool) -> Result<()> {
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
                let v = serde_json::to_value(client.list_issues(owner, repo, status.as_deref()).await?)?;
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

// ---------------------------------------------------------------------------
// groups
// ---------------------------------------------------------------------------

async fn group(cmd: args::GroupCommand, client: &GitaClient, json: bool) -> Result<()> {
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

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

async fn search(cmd: args::SearchCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::SearchCommand::Repos { q } => {
            let hits = client.search_repos(&q).await?;
            if json {
                output::print_json(&hits)?;
            } else {
                let mut table = output::Table::new(&["full_name", "private"]);
                for h in &hits.hits {
                    table.add_row(vec![h.full_name.clone(), h.is_private.to_string()]);
                }
                table.print();
            }
        }
        args::SearchCommand::Code { q } => {
            let hits = client.search_code(&q).await?;
            if json {
                output::print_json(&hits)?;
            } else {
                let mut table = output::Table::new(&["repo", "path", "line", "content"]);
                for h in &hits.hits {
                    table.add_row(vec![
                        h.repo.clone(),
                        h.path.clone(),
                        h.line.to_string(),
                        h.content.clone(),
                    ]);
                }
                table.print();
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// pipelines
// ---------------------------------------------------------------------------

async fn pipeline(cmd: args::PipelineCommand, client: &GitaClient, json: bool) -> Result<()> {
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

// ---------------------------------------------------------------------------
// ssh keys
// ---------------------------------------------------------------------------

async fn ssh_key(cmd: args::SshKeyCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::SshKeyCommand::Add { title, key } => {
            // Allow `--title` and a key that is either inline content or a
            // path to an OpenSSH `.pub` file.
            let content = std::fs::read_to_string(&key).unwrap_or_else(|_| key.clone());
            let k = client.add_ssh_key(content.trim(), title.as_deref()).await?;
            if json {
                output::print_json(&k)?;
            } else {
                println!("Added SSH key #{}: {}", k.id, k.title);
            }
        }
        args::SshKeyCommand::List => {
            let resp = client.list_ssh_keys().await?;
            if json {
                output::print_json(&resp)?;
            } else {
                let mut table = output::Table::new(&["id", "title", "public_key", "created_at"]);
                for k in &resp.keys {
                    table.add_row(vec![
                        k.id.to_string(),
                        k.title.clone(),
                        k.public_key.clone(),
                        k.created_at.clone(),
                    ]);
                }
                table.print();
            }
        }
        args::SshKeyCommand::Delete { id } => {
            let msg = client.delete_ssh_key(id).await?;
            if json {
                output::print_json(&msg)?;
            } else {
                println!("{}", msg.message);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

async fn event(cmd: args::EventCommand, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::EventCommand::List { limit, repo, r#type } => {
            let events = client.list_events(limit, repo.as_deref(), r#type.as_deref()).await?;
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

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

fn config(cmd: args::ConfigCommand, _client: &GitaClient, _json: bool) -> Result<()> {
    match cmd {
        args::ConfigCommand::SetUrl { url } => {
            crate::config::Config::save_url(&url)?;
            println!("Server URL set to {url}");
            Ok(())
        }
        args::ConfigCommand::Show => {
            println!("(config display not implemented)");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// api (raw REST call)
// ---------------------------------------------------------------------------

async fn call_api(
    client: &GitaClient,
    path: &str,
    method: &str,
    body: Option<&str>,
    json: bool,
) -> Result<()> {
    // Paths may be passed with or without the `/api` prefix; normalise.
    let path = path.trim_start_matches('/');
    let path = if path.starts_with("api/") {
        format!("/{path}")
    } else {
        format!("/api/{path}")
    };

    let body_value = match body {
        Some(b) => Some(
            serde_json::from_str(b)
                .map_err(|e| anyhow::anyhow!("invalid --body JSON: {e}"))?,
        ),
        None => None,
    };

    let v = client.api(method, &path, body_value).await?;
    if json {
        output::print_json(&v)?;
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}
