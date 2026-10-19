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