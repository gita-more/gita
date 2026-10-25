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

        // Apply the only/except trigger filters from the .gita-ci.yml we
        // just parsed. The branch is matched by its short name (`main`),
        // which is what a user writes in `branches: [main, release/*]` —
        // matching the full ref would never match a short glob.
        let short_branch = upd
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&upd.ref_name);
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

        let run_id = match db
            .insert_pipeline_run(
                pipeline_id,
                repo_id,
                &upd.ref_name,
                &upd.new_sha,
                pusher_id,
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("could not enqueue a run for {owner}/{repo_name}: {e}");
                continue;
            }
        };
        let req_payload = serde_json::json!({
            "owner": owner,
            "repo": repo_name,
            "run_id": run_id,
            "pipeline_id": pipeline_id,
            "ref": upd.ref_name,
            "head_sha": upd.new_sha,
        });

        // Persist the event here rather than letting the bus's persist
        // hook do it, so we can learn its database id and point
        // `pipeline_runs.trigger_event_id` at it. The hook would insert
        // the row asynchronously and could not report the id back — which
        // is why this column was always NULL.
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
