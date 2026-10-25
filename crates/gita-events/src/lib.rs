//! Event bus for Gita.
//!
//! The event bus is the nervous system of the AI-native design. Every
//! meaningful activity (push, PR opened, issue created, ...) is
//! published as a structured event. AI agents and internal modules can
//! subscribe to the stream and react.

pub mod bus;
pub mod log;

pub use bus::EventBus;
pub use log::{EventLogSubscriber, EventMap, InMemoryEventLog};

#[cfg(test)]
mod tests {
    use super::*;
    use gita_core::{Event, EventType};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn publish_and_subscribe_receives_event() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        bus.publish(EventType::Push, Some(1), Some(2), serde_json::json!({}));

        let received = rx.try_recv().unwrap();
        assert_eq!(received.event_type, EventType::Push);
        assert_eq!(received.repo_id, Some(1));
        assert_eq!(received.actor_id, Some(2));
    }

    #[test]
    fn publish_calls_persist_hook() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let bus = EventBus::new(Some(Arc::new(move |_e: Event| {
            calls2.fetch_add(1, Ordering::SeqCst);
        })));

        bus.publish(EventType::IssueCreated, None, None, serde_json::json!({}));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn publish_without_subscribers_does_not_panic() {
        let bus = EventBus::default();
        bus.publish(EventType::RepoCreated, Some(1), None, serde_json::json!({}));
        // no panic = pass
    }

    /// A pre-persisted event is handed to subscribers with its database id
    /// intact, and the persist hook still sees it — the hook is what
    /// decides whether the row needs inserting, using the id as the marker.
    #[test]
    fn publish_persisted_keeps_the_id_and_calls_the_hook() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let bus = EventBus::new(Some(Arc::new(move |e: Event| {
            seen2.lock().unwrap().push(e);
        })));
        let mut rx = bus.subscribe();

        let mut event = Event::new(
            EventType::PipelineRequested,
            Some(1),
            Some(2),
            serde_json::json!({}),
        );
        event.id = 42;
        let returned = bus.publish_persisted(event);

        assert_eq!(returned.id, 42);
        let received = rx.try_recv().unwrap();
        assert_eq!(received.id, 42);
        assert_eq!(received.event_type, EventType::PipelineRequested);

        let hooked = seen.lock().unwrap();
        assert_eq!(hooked.len(), 1);
        assert_eq!(hooked[0].id, 42);
    }

    /// An event published through the ordinary path has no database id yet.
    #[test]
    fn publish_leaves_the_id_unset() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let event = bus.publish(EventType::Push, Some(1), Some(2), serde_json::json!({}));
        assert_eq!(event.id, 0);
        assert_eq!(rx.try_recv().unwrap().id, 0);
    }

    #[tokio::test]
    async fn in_memory_log_pushes_and_returns_recent() {
        let log = Arc::new(InMemoryEventLog::default());
        let bus = EventBus::default();
        let _handle = EventLogSubscriber::spawn_listener(bus.clone(), log.clone());

        for i in 0..3 {
            bus.publish(
                EventType::Push,
                Some(i),
                Some(i),
                serde_json::json!({"i": i}),
            );
        }

        // Wait until the spawned listener has drained all three events.
        for _ in 0..100 {
            if log.len() == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let recent = log.recent(10);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].repo_id, Some(2)); // newest first
        assert_eq!(recent[2].repo_id, Some(0)); // oldest last
    }

    #[test]
    fn recent_snapshot_returns_empty_without_persist() {
        let bus = EventBus::default();
        assert!(bus.recent_snapshot(10).is_empty());
    }

    // Use a helper to avoid a flaky test in slow CI: the broadcast channel
    // keeps at most 1024 events; verify the log bound is respected.
    #[test]
    fn in_memory_log_bounds_length() {
        let log = Arc::new(InMemoryEventLog::default());
        let bus = EventBus::new(Some(Arc::new({
            let log = log.clone();
            move |e: Event| log.push(e)
        })));
        for i in 0..20_000 {
            bus.publish(EventType::Push, Some(i), None, serde_json::json!({}));
        }
        assert!(log.len() <= 10_000);
    }

    /// Real Lagged recovery path. Create a bus, fire enough events to
    /// overflow the broadcast channel capacity (1024), confirm the
    /// subscriber sees `RecvError::Lagged`, and then verify that the
    /// subscriber can continue to receive subsequent events.
    ///
    /// Note: tokio's `try_recv` silently skips lagged messages without
    /// reporting them — only the async `recv().await` returns
    /// `RecvError::Lagged`. We use that here.
    #[tokio::test]
    async fn subscriber_recovers_from_lagged() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        // Publish way more than 1024 events so the buffer overwrites the
        // unread end while the subscriber isn't draining.
        for i in 0..2000 {
            bus.publish(EventType::Push, Some(i), None, serde_json::json!({"i": i}));
        }

        // async recv should immediately surface Lagged.
        let first = rx.recv().await;
        assert!(
            matches!(
                first,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
            ),
            "expected Lagged, got {first:?}"
        );

        // Publish one more and make sure the receiver can keep going.
        bus.publish(
            EventType::IssueCreated,
            None,
            None,
            serde_json::json!({"after": "lag"}),
        );

        let mut saw_after = false;
        for _ in 0..2000 {
            match rx.recv().await {
                Ok(ev) if ev.event_type == EventType::IssueCreated => {
                    saw_after = true;
                    break;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        assert!(saw_after, "should be able to keep receiving after Lagged");
    }

    /// When every `Sender` is dropped, subscribers see `RecvError::Closed`
    /// exactly once and the listener loop in `EventLogSubscriber` exits.
    #[tokio::test]
    async fn subscribers_observe_closed_after_drop() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        // Drop the only Sender that the original bus owns.
        drop(bus);

        // async recv yields Closed once the channel is empty.
        let result = rx.recv().await;
        assert!(
            matches!(
                result,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ),
            "expected Closed after bus drop, got {result:?}"
        );
    }
}
