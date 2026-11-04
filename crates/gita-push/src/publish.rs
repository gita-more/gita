//! Publishing a push: emit the `push` events and enqueue the CI runs
//! the pushed commit triggers.

use gita_core::{Event, EventType};
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use tracing::{info, warn};

use crate::refs::RefUpdate;

/// Publish the `push` events for the applied ref updates and enqueue the
/// CI runs they trigger.
///
/// `pusher_id` is the user who pushed, if the transport could identify
/// them. It ends up on the event and on the CI run, so that a policy
/// like "do not run CI for pushes by this bot" has something to read.
pub async fn publish_push(
    db: &Db,
    bus: &EventBus,
    git: &GitManager,
    owner: &str,
    repo_name: &str,
    updates: &[RefUpdate],
    pusher_id: Option<i64>,
) {
    let repo = match db.get_repository_by_name(owner, repo_name).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!("push to unknown repository {owner}/{repo_name}");
            return;
        }
        Err(e) => {
            warn!("could not load repository {owner}/{repo_name}: {e}");
            return;
        }
    };
    let repo_id = repo.id;

    for upd in updates {
        let payload = serde_json::json!({
            "owner": owner,
            "repo": repo_name,
            "ref": upd.ref_name,
            "old_sha": upd.old_sha,
            "new_sha": upd.new_sha,
            "pusher_id": pusher_id,
        });
        info!(
            push_ref = %upd.ref_name,
            new_sha = %upd.new_sha,
            "push to {owner}/{repo_name}"
        );
        bus.publish(EventType::Push, Some(repo_id), pusher_id, payload);
    }

    // CI trigger: if the pushed commit contains `.gita-ci.yml`, upsert the
    // pipeline, enqueue a run, and emit a PipelineRequested event.
    //
    // The event is persisted here instead of by the bus's persist hook, so
    // its database id is known and can be stored in
    // `pipeline_runs.trigger_event_id`. The hook cannot report an id back
    // — it is synchronous, and the insert is not.
    for upd in updates {
        if upd.is_deletion() {
            continue; // branch deletion — skip
        }
        let file = match git.read_file(owner, repo_name, &upd.new_sha, ".gita-ci.yml") {
            Ok(Some(file)) => file,
            Ok(None) => continue, // no CI config at this commit
            Err(e) => {
                warn!("could not read .gita-ci.yml of {owner}/{repo_name}: {e}");
                continue;
            }
        };

        // Parse + apply only/except filter before enqueueing. The changed
        // paths are the difference between the pushed commit and the one
        // it replaced, so that `only:`/`except:` path filters have
        // something to match against. Without them an
        // `except: paths: [...]` rule could never fire.
        let changed_paths = changed_paths_for_push(git, owner, repo_name, upd).await;
        let changed_refs: Vec<&str> = changed_paths.iter().map(|p| p.as_str()).collect();

        // The pipeline row records the repository's CI configuration; it
        // exists whether or not this push triggers a run. Recording it
        // before the filters are applied keeps that distinction: the
        // filter decides whether a *run* is enqueued, not whether the
        // configuration is remembered.
        let pipeline_id = match db
            .upsert_pipeline(
                repo_id,
                ".gita-ci.yml",
                &upd.ref_name,
                &upd.new_sha,
                &file.content,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("could not record the pipeline for {owner}/{repo_name}: {e}");
                continue;
            }
        };

        // The branch is matched by its short name (`main`), which is what
        // a user writes in `branches: [main, release/*]` — matching the
        // full ref would never match a short glob.
        let short_branch = short_branch_name(&upd.ref_name);
        match gita_pipeline::parse_yaml(&file.content) {
            Ok(spec) => {
                use gita_pipeline::TriggerDecision;
                if matches!(
                    spec.triggers.should_run(short_branch, &changed_refs),
                    TriggerDecision::Skip
                ) {
                    info!(
                        "skip enqueue for {}/{} on {} (filtered by only/except)",
                        owner, repo_name, upd.ref_name
                    );
                    continue;
                }
            }
            Err(e) => {
                // A malformed yaml is fine to skip here: the runner will
                // mark the run failed on pickup. We just don't enqueue.
                warn!("parse .gita-ci.yml failed (skip enqueue): {e}");
                continue;
            }
        }

        enqueue_run(
            db,
            bus,
            &RunRequest {
                repo_id,
                owner,
                repo_name,
                pipeline_id,
                ref_name: &upd.ref_name,
                head_sha: &upd.new_sha,
                pusher_id,
            },
        )
        .await;
    }

    // Workflow trigger: GitHub-Actions-style workflow files under
    // `.github/workflows/`. Each matching workflow becomes its own run,
    // the same way a `.gita-ci.yml` pipeline does.
    for upd in updates {
        if upd.is_deletion() {
            continue; // branch deletion — skip
        }

        let workflow_files = match list_workflow_files(git, owner, repo_name, &upd.new_sha) {
            Ok(files) => files,
            Err(e) => {
                warn!("could not list workflows of {owner}/{repo_name}: {e}");
                continue;
            }
        };
        if workflow_files.is_empty() {
            continue;
        }

        let changed_paths = changed_paths_for_push(git, owner, repo_name, upd).await;
        let changed_refs: Vec<&str> = changed_paths.iter().map(|p| p.as_str()).collect();
        let short_branch = short_branch_name(&upd.ref_name);

        for (path, content) in workflow_files {
            // The pipeline row records the workflow's configuration; it
            // exists whether or not this push triggers a run, the same
            // way a `.gita-ci.yml` pipeline row does.
            let pipeline_id = match db
                .upsert_pipeline(repo_id, &path, &upd.ref_name, &upd.new_sha, &content)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    warn!("could not record the pipeline {path} of {owner}/{repo_name}: {e}");
                    continue;
                }
            };

            // A malformed workflow is skipped here; the runner would mark
            // the run failed on pickup, but there is no point enqueueing
            // work we already know cannot be parsed.
            let workflow = match gita_workflow::parse_workflow(&content) {
                Ok(w) => w,
                Err(e) => {
                    warn!("parse {path} of {owner}/{repo_name} failed (skip enqueue): {e}");
                    continue;
                }
            };

            let decision =
                gita_workflow::should_run_on_push(&workflow.trigger, short_branch, &changed_refs);
            if matches!(decision, gita_workflow::TriggerDecision::Skip) {
                info!(
                    "skip enqueue for {owner}/{repo_name} on {} ({path} did not match the push)",
                    upd.ref_name
                );
                continue;
            }

            enqueue_run(
                db,
                bus,
                &RunRequest {
                    repo_id,
                    owner,
                    repo_name,
                    pipeline_id,
                    ref_name: &upd.ref_name,
                    head_sha: &upd.new_sha,
                    pusher_id,
                },
            )
            .await;
        }
    }
}

/// The short branch name of a ref (`main` for `refs/heads/main`).
///
/// Branch filters are written with short names (`branches: [main]`), so
/// matching the full ref would never match.
fn short_branch_name(ref_name: &str) -> &str {
    ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name)
}

/// The workflow files present at a commit, as `(path, content)` pairs.
///
/// Only files directly under `.github/workflows/` with a `.yml` or `.yaml`
/// extension count, matching GitHub's rule.
fn list_workflow_files(
    git: &GitManager,
    owner: &str,
    repo_name: &str,
    commit: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let entries = git.list_tree(owner, repo_name, commit, ".github/workflows")?;
    let mut files = Vec::new();
    for entry in entries {
        if entry.kind != "blob" {
            continue;
        }
        let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        // `list_tree` is recursive, so a nested file must not be treated
        // as a workflow at the top level.
        if entry
            .path
            .trim_start_matches(".github/workflows/")
            .contains('/')
        {
            continue;
        }
        if let Ok(Some(content)) = git.read_file(owner, repo_name, commit, &entry.path) {
            files.push((entry.path, content.content));
        }
    }
    files.sort();
    Ok(files)
}

/// The run to enqueue: which pipeline it belongs to, what it tests, and
/// who pushed it.
struct RunRequest<'a> {
    repo_id: i64,
    owner: &'a str,
    repo_name: &'a str,
    pipeline_id: i64,
    ref_name: &'a str,
    head_sha: &'a str,
    pusher_id: Option<i64>,
}

/// Enqueue a run for an already-recorded pipeline and emit the
/// `PipelineRequested` event.
///
/// The event is persisted here rather than by the bus's persist hook, so
/// its database id is known and can be stored in
/// `pipeline_runs.trigger_event_id`. The hook cannot report an id back —
/// it is synchronous, and the insert is not.
async fn enqueue_run(db: &Db, bus: &EventBus, req: &RunRequest<'_>) {
    let RunRequest {
        repo_id,
        owner,
        repo_name,
        pipeline_id,
        ref_name,
        head_sha,
        pusher_id,
    } = *req;

    let run_id = match db
        .insert_pipeline_run(pipeline_id, repo_id, ref_name, head_sha, pusher_id, None)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            warn!("could not enqueue a run for {owner}/{repo_name}: {e}");
            return;
        }
    };
    let req_payload = serde_json::json!({
        "owner": owner,
        "repo": repo_name,
        "run_id": run_id,
        "pipeline_id": pipeline_id,
        "ref": ref_name,
        "head_sha": head_sha,
    });

    let mut event = Event::new(
        EventType::PipelineRequested,
        Some(repo_id),
        pusher_id,
        req_payload,
    );
    match db.insert_event(&event).await {
        Ok(event_id) => {
            event.id = event_id;
            // Backfill the link now that the event exists.
            if let Err(e) = db.set_pipeline_run_trigger_event(run_id, event_id).await {
                warn!("failed to link pipeline run {run_id} to event {event_id}: {e}");
            }
        }
        Err(e) => {
            // The event could not be stored. Publish it anyway (with
            // id 0) so subscribers still see the request — the bus
            // hook will retry the insert.
            warn!("failed to persist PipelineRequested event: {e}");
        }
    }
    bus.publish_persisted(event);
}

/// The paths a pushed commit changed, relative to the repository root.
///
/// This is what `only:`/`except:` path filters match against. For a new
/// branch the whole tree counts as changed, since there is no earlier
/// commit to compare against.
async fn changed_paths_for_push(
    git: &GitManager,
    owner: &str,
    repo_name: &str,
    upd: &RefUpdate,
) -> Vec<String> {
    // A branch creation has no previous commit; treat every file as
    // changed so path filters still have a chance to match.
    let old_sha = upd.old_sha.trim();
    if old_sha.is_empty() || old_sha.chars().all(|c| c == '0') {
        return git
            .list_tree(owner, repo_name, &upd.new_sha, "")
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| e.kind == "blob")
                    .map(|e| e.path)
                    .collect()
            })
            .unwrap_or_default();
    }
    match git.diff_files_between(owner, repo_name, old_sha, &upd.new_sha) {
        Ok(files) => files.into_iter().map(|(_, path)| path).collect(),
        Err(e) => {
            warn!("could not compute changed paths for {owner}/{repo_name}: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_git::GitManager;

    fn run_git(args: &[&str], cwd: &std::path::Path) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A bare repository with one commit holding `files` (path → content).
    ///
    /// Returns the git manager and the commit sha.
    async fn repo_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, GitManager, String) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let git = GitManager::new(dir.path().to_path_buf());
        git.create_repo("alice", "widgets", "main")
            .await
            .expect("create repo");
        let bare = dir.path().join("alice").join("widgets.git");
        let url = format!("file://{}", bare.display());

        let work = tempfile::TempDir::new().expect("work tree");
        run_git(&["init", "-q", "-b", "main"], work.path());
        run_git(&["remote", "add", "origin", &url], work.path());
        for (path, content) in files {
            let dest = work.path().join(path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, content).unwrap();
        }
        run_git(&["add", "-A"], work.path());
        run_git(&["commit", "-q", "-m", "test commit"], work.path());
        run_git(&["push", "-q", "origin", "main"], work.path());

        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(work.path())
            .output()
            .expect("rev-parse");
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (dir, git, sha)
    }

    /// The branch filters in `.gita-ci.yml` name the branch without the
    /// `refs/heads/` prefix; a tag ref keeps its full name.
    #[test]
    fn a_branch_name_is_shortened_and_a_tag_is_not() {
        assert_eq!(short_branch_name("refs/heads/main"), "main");
        assert_eq!(short_branch_name("refs/heads/feature/x"), "feature/x");
        assert_eq!(short_branch_name("refs/tags/v1"), "refs/tags/v1");
    }

    /// Only the workflow files directly under `.github/workflows/` count:
    /// a nested directory is not a workflow, and neither is a file with
    /// another extension.
    #[tokio::test]
    async fn only_top_level_yaml_workflow_files_are_listed() {
        let (_dir, git, sha) = repo_with_files(&[
            (".github/workflows/ci.yml", "name: ci\n"),
            (".github/workflows/other.yaml", "name: other\n"),
            (".github/workflows/notes.txt", "not a workflow\n"),
            (".github/workflows/nested/deep.yml", "name: deep\n"),
            ("README.md", "hello\n"),
        ])
        .await;

        let files = list_workflow_files(&git, "alice", "widgets", &sha).expect("list");
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec![".github/workflows/ci.yml", ".github/workflows/other.yaml"],
            "got {paths:?}"
        );
    }

    /// A repository with no workflows directory yields no workflows, and
    /// no error.
    #[tokio::test]
    async fn a_repository_without_workflows_yields_no_workflows() {
        let (_dir, git, sha) = repo_with_files(&[("README.md", "hello\n")]).await;
        let files = list_workflow_files(&git, "alice", "widgets", &sha).expect("list");
        assert!(files.is_empty(), "got {files:?}");
    }

    /// A branch that did not exist before has no previous commit, so
    /// every file in the tree counts as changed.
    #[tokio::test]
    async fn a_new_branch_reports_every_file_as_changed() {
        let (_dir, git, sha) = repo_with_files(&[("a.txt", "a\n"), ("dir/b.txt", "b\n")]).await;
        let upd = RefUpdate {
            old_sha: "0".repeat(40),
            new_sha: sha.clone(),
            ref_name: "refs/heads/new".to_string(),
        };
        let mut changed = changed_paths_for_push(&git, "alice", "widgets", &upd).await;
        changed.sort();
        assert_eq!(changed, vec!["a.txt".to_string(), "dir/b.txt".to_string()]);
    }

    /// A push that only changes one file reports only that file.
    #[tokio::test]
    async fn a_push_reports_only_the_files_that_changed() {
        let (dir, git, sha) = repo_with_files(&[("a.txt", "a\n"), ("b.txt", "b\n")]).await;
        let bare = dir.path().join("alice").join("widgets.git");
        let url = format!("file://{}", bare.display());
        let work = tempfile::TempDir::new().expect("work tree");
        run_git(&["clone", "-q", &url, "."], work.path());
        std::fs::write(work.path().join("a.txt"), "a changed\n").unwrap();
        run_git(&["commit", "-q", "-am", "second"], work.path());
        run_git(&["push", "-q", "origin", "main"], work.path());
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(work.path())
            .output()
            .expect("rev-parse");
        let second = String::from_utf8_lossy(&out.stdout).trim().to_string();

        let upd = RefUpdate {
            old_sha: sha,
            new_sha: second,
            ref_name: "refs/heads/main".to_string(),
        };
        let changed = changed_paths_for_push(&git, "alice", "widgets", &upd).await;
        assert_eq!(changed, vec!["a.txt".to_string()]);
    }

    /// A diff that cannot be computed yields no paths rather than an
    /// error: the caller treats an empty list as "nothing matched".
    #[tokio::test]
    async fn an_unreadable_diff_yields_no_paths() {
        let (_dir, git, _sha) = repo_with_files(&[("a.txt", "a\n")]).await;
        let upd = RefUpdate {
            old_sha: "1".repeat(40),
            new_sha: "2".repeat(40),
            ref_name: "refs/heads/main".to_string(),
        };
        let changed = changed_paths_for_push(&git, "alice", "widgets", &upd).await;
        assert!(changed.is_empty(), "got {changed:?}");
    }
}
