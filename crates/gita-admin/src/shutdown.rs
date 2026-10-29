//! Shutting the server down without losing work.
//!
//! A server that is killed rather than stopped loses whatever it was in
//! the middle of. Two things in Gita are in the middle of things when a
//! request finishes:
//!
//! * the event row for what just happened, and
//! * the webhooks that event should fan out to.
//!
//! Both are written from background tasks, so a `SIGTERM` that kills
//! the process outright loses them — a push that happened is a push
//! that the event log does not know about, and a webhook that was not
//! delivered is a webhook the user was told about and never received.
//! `docker stop` sends exactly that signal.
//!
//! [`BackgroundTasks`] keeps track of those tasks so shutdown can wait
//! for them, and [`shutdown_signal`] turns `SIGINT`/`SIGTERM` into a
//! signal the server can act on instead of a sudden death.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Tracks background tasks so that shutdown can wait for them.
///
/// A task spawned through this tracker is awaited by
/// [`BackgroundTasks::drain`]. Tasks spawned after the drain started
/// are waited for too, so a request that is still finishing when the
/// signal arrives does not get its event dropped on the floor.
#[derive(Clone, Default)]
pub struct BackgroundTasks {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// How many tracked tasks have not finished yet.
    active: AtomicUsize,
    /// Signalled whenever a task finishes.
    finished: Notify,
}

impl BackgroundTasks {
    /// A fresh tracker with nothing running.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a task and keep track of it.
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let inner = self.inner.clone();
        inner.active.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            future.await;
            inner.active.fetch_sub(1, Ordering::SeqCst);
            inner.finished.notify_waiters();
        });
    }

    /// Wait until every tracked task has finished.
    ///
    /// Tasks spawned while the drain is in progress are waited for as
    /// well: the loop re-checks the count after every wake-up, and only
    /// returns when the count is zero.
    pub async fn drain(&self) {
        loop {
            // Register for the notification *before* re-reading the count,
            // so a task that finishes between the two cannot slip past.
            let notified = self.inner.finished.notified();
            if self.inner.active.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// How many tracked tasks have not finished yet.
    pub fn active(&self) -> usize {
        self.inner.active.load(Ordering::SeqCst)
    }
}

/// Resolve when the process is asked to stop.
///
/// Listens for `SIGINT` (Ctrl-C) and `SIGTERM` (what `docker stop` and
/// most process supervisors send). The first signal wins; a second one
/// is left to the default handler, so an operator who hits Ctrl-C twice
/// still gets an immediate kill.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("could not listen for SIGINT: {e}");
            // Nothing else will ever resolve this future, so park here
            // rather than shutting down by accident.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("could not listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_waits_for_a_running_task() {
        let tasks = BackgroundTasks::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks.spawn(async move {
            let _ = rx.await;
        });
        assert_eq!(tasks.active(), 1);

        // The task is still running, so the drain must not return yet.
        let drain = tokio::time::timeout(std::time::Duration::from_millis(50), tasks.drain()).await;
        assert!(drain.is_err(), "drain returned while a task was running");

        tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), tasks.drain())
            .await
            .expect("drain should finish once the task is done");
        assert_eq!(tasks.active(), 0);
    }

    #[tokio::test]
    async fn drain_returns_immediately_when_nothing_is_running() {
        let tasks = BackgroundTasks::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), tasks.drain())
            .await
            .expect("drain should return at once");
    }

    #[tokio::test]
    async fn drain_waits_for_a_task_spawned_while_draining() {
        let tasks = BackgroundTasks::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tasks.spawn(async move {
            let _ = rx.await;
        });

        // A task spawned after the drain started must be waited for too.
        let tracker = tasks.clone();
        let drain = tokio::spawn(async move { tracker.drain().await });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        tasks.spawn(async move {
            let _ = rx2.await;
        });

        tx.send(()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        tx2.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), drain)
            .await
            .expect("drain should not return early")
            .expect("drain task should not panic");
    }
}
