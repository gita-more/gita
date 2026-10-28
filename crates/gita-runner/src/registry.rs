//! Shared registry of in-flight jobs so an external "cancel run" request
//! can find and kill them.
//!
//! The runner is process-local: when `gita server worker register` + a
//! `POST /runs/{id}/cancel` happens, the HTTP layer just flips status to
//! `canceled` in the database. The runner learns about the cancel by
//! polling the run's status while the child is running, and uses this
//! registry to issue `child.kill()` against the right process.
//!
//! Note: this registry is intentionally process-local. Multi-runner
//! cancel of one runner's jobs requires that runner to be running; the
//! other runner's jobs aren't tracked here.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Child;
use tokio::sync::Mutex;

/// `(Child, JoinHandle<...>)` — the spawned process plus its stderr/stdout
/// drain tasks. We keep all three together so a cancel-kill and the
/// subsequent wait all happen on the same handle.
pub struct JobHandle {
    pub child: Child,
}

/// Maps `run_id → (job_name → JobHandle)`.
#[derive(Clone, Default)]
pub struct JobRegistry {
    inner: Arc<Mutex<HashMap<i64, HashMap<String, JobHandle>>>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a child under (run_id, job_name). Replaces any prior
    /// entry for that pair.
    pub async fn register(&self, run_id: i64, job_name: &str, child: Child) {
        let mut g = self.inner.lock().await;
        g.entry(run_id)
            .or_default()
            .insert(job_name.to_string(), JobHandle { child });
    }

    /// Take the child out of the registry so the caller can `.wait()`
    /// on it. After this the cancel watcher can no longer kill this job
    /// via `kill_run`. Callers that want both behaviors should call
    /// `kill_run` directly while the child is still registered.
    pub async fn take(&self, run_id: i64, job_name: &str) -> Option<Child> {
        let mut g = self.inner.lock().await;
        g.get_mut(&run_id)
            .and_then(|m| m.remove(job_name))
            .map(|h| h.child)
    }

    /// Kill every child currently registered for this run. Children
    /// are `.start_kill()`-ed (non-blocking) so the caller's `wait()`
    /// still observes the kill and finalizes normally. Returns the
    /// number of children killed.
    pub async fn kill_run(&self, run_id: i64) -> usize {
        let mut g = self.inner.lock().await;
        let Some(map) = g.get_mut(&run_id) else {
            return 0;
        };
        let mut killed = 0;
        for (_, h) in map.iter_mut() {
            if h.child.start_kill().is_ok() {
                killed += 1;
            }
        }
        killed
    }

    /// Remove the run's entry entirely (after all children have exited
    /// and been taken).
    pub async fn drop_run(&self, run_id: i64) {
        let mut g = self.inner.lock().await;
        g.remove(&run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    /// A child that stays alive until it is killed.
    fn sleeper() -> Child {
        Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sleep")
    }

    #[tokio::test]
    async fn a_registered_child_is_returned_by_take() {
        let registry = JobRegistry::new();
        registry.register(1, "build", sleeper()).await;

        let child = registry.take(1, "build").await.expect("the child is there");
        assert!(child.id().is_some(), "the child is still running");

        // Taking removes the entry, so the cancel watcher can no longer
        // reach this job.
        assert!(registry.take(1, "build").await.is_none());
        assert_eq!(registry.kill_run(1).await, 0);
    }

    #[tokio::test]
    async fn kill_run_kills_every_child_of_the_run() {
        let registry = JobRegistry::new();
        registry.register(1, "build", sleeper()).await;
        registry.register(1, "test", sleeper()).await;
        registry.register(2, "build", sleeper()).await;

        assert_eq!(registry.kill_run(1).await, 2);
        // Another run's children are untouched.
        assert_eq!(registry.kill_run(2).await, 1);
        assert_eq!(registry.kill_run(3).await, 0);
    }

    #[tokio::test]
    async fn a_killed_child_actually_dies() {
        let registry = JobRegistry::new();
        registry.register(1, "build", sleeper()).await;

        assert_eq!(registry.kill_run(1).await, 1);

        let mut child = registry.take(1, "build").await.expect("the child is there");
        let status = child.wait().await.expect("the child exited");
        assert!(
            !status.success(),
            "a killed child must not report success: {status:?}"
        );
    }

    #[tokio::test]
    async fn registering_the_same_job_twice_replaces_the_entry() {
        let registry = JobRegistry::new();
        registry.register(1, "build", sleeper()).await;
        registry.register(1, "build", sleeper()).await;

        // Only one child is tracked, so one kill.
        assert_eq!(registry.kill_run(1).await, 1);
    }

    #[tokio::test]
    async fn dropping_a_run_removes_all_its_jobs() {
        let registry = JobRegistry::new();
        registry.register(1, "build", sleeper()).await;
        registry.register(1, "test", sleeper()).await;

        registry.drop_run(1).await;
        assert_eq!(registry.kill_run(1).await, 0);
    }
}
