//! What a push does after the fact.
//!
//! A push that lands on a repository has two consequences the rest of
//! the system cares about: a `push` event, and — if the pushed commit
//! carries a `.gita-ci.yml` — one or more CI runs.
//!
//! Both consequences used to belong to the HTTP transport alone. The
//! SSH transport ran the same `git receive-pack`, updated the same
//! refs, and then published nothing: a repository pushed over HTTP ran
//! its CI, the same repository pushed over SSH did not, and the only
//! visible difference was the transport the user happened to pick.
//!
//! This crate is the shared half. Both transports call
//! [`publish_push`] with the ref updates that were applied, and it does
//! the rest.
//!
//! # Which ref updates count
//!
//! A client *claims* to update some refs; the repository may refuse.
//! `git receive-pack` reports the outcome in its `report-status`
//! section, and both transports therefore only report updates that
//! actually took effect:
//!
//! * HTTP parses the commands out of the request body and then checks
//!   each one against the repository — a ref counts as updated only if
//!   it now points at the new sha.
//! * SSH parses the `report-status` the server wrote back.
//!
//! Either way, a rejected push publishes nothing and triggers no CI.

use gita_core::EventType;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use tracing::{info, warn};

/// A ref update that was applied to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    /// The sha the ref pointed at before, or all zeroes for a creation.
    pub old_sha: String,
    /// The sha the ref points at now, or all zeroes for a deletion.
    pub new_sha: String,
    /// The full ref name, e.g. `refs/heads/main`.
    pub ref_name: String,
}

impl RefUpdate {
    /// Whether this update deletes the ref.
    pub fn is_deletion(&self) -> bool {
        self.new_sha.chars().all(|c| c == '0') && !self.new_sha.is_empty()
    }
}

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
        let Some(file) = git
            .read_file(owner, repo_name, &upd.new_sha, ".gita-ci.yml")
            .ok()
            .flatten()
        else {
            continue; // no CI config at this commit
        };

        // Parse + apply only/except filter before enqueueing. The changed
        // paths are the difference between the pushed commit and the one
        // it replaced, so that `only:`/`except:` path filters have
        // something to match against. Without them an
        // `except: paths: [...]` rule could never fire.
        let changed_paths = changed_paths_for_push(git, owner, repo_name, upd).await;
        let changed_refs: Vec<&str> = changed_paths.iter().map(|p| p.as_str()).collect();
        let pipeline_id = db
            .upsert_pipeline(
                repo_id,
                ".gita-ci.yml",
                &upd.ref_name,
                &upd.new_sha,
                &file.content,
            )
            .await
            .ok();
        let Some(pid) = pipeline_id else { continue };

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

        let run_id = db
            .insert_pipeline_run(pid, repo_id, &upd.ref_name, &upd.new_sha, pusher_id, None)
            .await
            .ok();
        let Some(run_id) = run_id else { continue };
        let req_payload = serde_json::json!({
            "owner": owner,
            "repo": repo_name,
            "run_id": run_id,
            "pipeline_id": pid,
            "ref": upd.ref_name,
            "head_sha": upd.new_sha,
        });

        // Persist the event here rather than letting the bus's persist
        // hook do it, so we can learn its database id and point
        // `pipeline_runs.trigger_event_id` at it. The hook would insert
        // the row asynchronously and could not report the id back — which
        // is why this column was always NULL.
        let mut event = gita_core::Event::new(
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

/// Parse the ref updates out of a `git receive-pack` `report-status`
/// section.
///
/// The section is a sequence of pkt-lines (a four-byte ASCII-hex length
/// that includes the length bytes, then the payload). It looks like:
///
/// ```text
/// 000eunpack ok
/// 003f<old-sha> <new-sha> refs/heads/main
/// 000aok <message>
/// 0000
/// ```
///
/// Only updates the server *accepted* are returned: a line followed by
/// `ng <reason>` reports a refusal, and a ref whose status is missing or
/// unparseable is skipped rather than guessed at.
pub fn parse_report_status(output: &[u8]) -> Vec<RefUpdate> {
    let mut updates = Vec::new();
    let mut pos = 0usize;

    // The first line of the section is "unpack ok" / "unpack <error>".
    // Skip it.
    let Some((_, pos)) = next_pkt_line(output, pos) else {
        return updates;
    };
    let mut pos = pos;

    loop {
        // A flush packet ends the section.
        if output.len() >= pos + 4 && &output[pos..pos + 4] == b"0000" {
            break;
        }
        let Some((line, next)) = next_pkt_line(output, pos) else {
            break;
        };
        pos = next;

        let text = String::from_utf8_lossy(line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        // The status line ("ok" / "ng <reason>") belongs to the update
        // line before it.
        if text.starts_with("ok") || text.starts_with("ng") {
            // A status with no preceding update: nothing to attach it to.
            continue;
        }

        let mut fields = text.split_whitespace();
        let (Some(old_sha), Some(new_sha), Some(ref_name)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // The status of *this* update is the next line. Peek at it.
        let status = if output.len() >= pos + 4 && &output[pos..pos + 4] == b"0000" {
            // A flush where the status should be: treat as unconfirmed.
            None
        } else {
            next_pkt_line(output, pos).and_then(|(line, _)| {
                let s = String::from_utf8_lossy(line);
                let s = s.trim().to_string();
                pos += 4 + s.len() + 1; // pkt-line length + payload + LF
                Some(s)
            })
        };

        match status {
            Some(s) if s.starts_with("ok") => {
                updates.push(RefUpdate {
                    old_sha: old_sha.to_string(),
                    new_sha: new_sha.to_string(),
                    ref_name: ref_name.to_string(),
                });
            }
            // `ng <reason>` — the server refused this ref. Skip it.
            _ => continue,
        }
    }
    updates
}

/// Read one pkt-line at `pos`, returning its payload and the position
/// just after it.
fn next_pkt_line(buf: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    if pos + 4 > buf.len() {
        return None;
    }
    let len_str = std::str::from_utf8(&buf[pos..pos + 4]).ok()?;
    let len = usize::from_str_radix(len_str, 16).ok()?;
    if len == 0 {
        // Flush packet.
        return Some((&buf[pos..pos + 4], pos + 4));
    }
    if len < 4 || pos + len > buf.len() {
        return None;
    }
    Some((&buf[pos + 4..pos + len], pos + len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt_line(s: &str) -> Vec<u8> {
        let mut line = s.as_bytes().to_vec();
        line.push(b'\n');
        let len = line.len() + 4;
        let mut out = format!("{len:04x}").into_bytes();
        out.extend_from_slice(&line);
        out
    }

    #[test]
    fn report_status_yields_the_accepted_updates() {
        let mut buf = pkt_line("unpack ok");
        buf.extend(pkt_line(
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main",
        ));
        buf.extend(pkt_line("ok branch updated"));
        buf.extend(pkt_line(
            "3333333333333333333333333333333333333333 4444444444444444444444444444444444444444 refs/heads/other",
        ));
        buf.extend(pkt_line("ng branch is currently checked out"));
        buf.extend(pkt_line("0000"));

        let updates = parse_report_status(&buf);
        assert_eq!(updates.len(), 1, "only the accepted ref counts: {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
        assert_eq!(
            updates[0].old_sha,
            "1111111111111111111111111111111111111111"
        );
        assert_eq!(
            updates[0].new_sha,
            "2222222222222222222222222222222222222222"
        );
    }

    #[test]
    fn report_status_of_a_refused_push_is_empty() {
        let mut buf = pkt_line("unpack ok");
        buf.extend(pkt_line(
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main",
        ));
        buf.extend(pkt_line("ng permission denied"));
        buf.extend(pkt_line("0000"));

        assert!(parse_report_status(&buf).is_empty());
    }

    #[test]
    fn a_deletion_is_recognised() {
        let zeros = "0".repeat(40);
        let update = RefUpdate {
            old_sha: "1".repeat(40),
            new_sha: zeros.clone(),
            ref_name: "refs/heads/gone".to_string(),
        };
        assert!(update.is_deletion());

        let creation = RefUpdate {
            old_sha: zeros,
            new_sha: "2".repeat(40),
            ref_name: "refs/heads/new".to_string(),
        };
        assert!(!creation.is_deletion());
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(parse_report_status(b"").is_empty());
        assert!(parse_report_status(b"not a pkt line at all").is_empty());
        assert!(parse_report_status(&[0xff, 0xfe, 0xfd]).is_empty());
    }
}
