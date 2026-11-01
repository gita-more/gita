//! In-memory event history helpers used by tests and by the HTTP layer
//! for long-polling / recent events.

use std::collections::HashMap;
use std::sync::Arc;

use gita_core::{Event, EventType};

/// A convenience wrapper for in-memory event history used by tests
/// and by the HTTP layer for long-polling / recent events.
#[derive(Debug, Default)]
pub struct InMemoryEventLog {
    events: std::sync::Mutex<Vec<Event>>,
}

impl InMemoryEventLog {
    pub fn push(&self, event: Event) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
            // Bound the in-memory buffer.
            if guard.len() > 10_000 {
                let excess = guard.len() - 10_000;
                guard.drain(0..excess);
            }
        }
    }

    pub fn recent(&self, limit: usize) -> Vec<Event> {
        self.events
            .lock()
            .map(|guard| guard.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.events.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A subscriber that keeps an in-memory log of events.
/// Useful for tests and for the server's own `/api/events` endpoint.
#[derive(Debug)]
pub struct EventLogSubscriber {
    // Only held for ownership: keeps the Arc<InMemoryEventLog> alive while
    // the subscriber exists. The listener path uses the log via spawn_listener.
    #[allow(dead_code)]
    log: Arc<InMemoryEventLog>,
    _rx: tokio::sync::broadcast::Receiver<Event>,
}

impl EventLogSubscriber {
    pub fn new(bus: &crate::bus::EventBus, log: Arc<InMemoryEventLog>) -> Self {
        let rx = bus.subscribe();
        Self { log, _rx: rx }
    }

    pub fn spawn_listener(
        bus: crate::bus::EventBus,
        log: Arc<InMemoryEventLog>,
    ) -> tokio::task::JoinHandle<()> {
        // Subscribe synchronously *before* spawning the task so that events
        // published immediately after this call are not missed (broadcast
        // drops messages when no receiver is registered yet).
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => log.push(event),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

/// A simple typed event map used for routing in tests.
pub type EventMap = HashMap<EventType, Vec<Event>>;

#[cfg(test)]
mod log_tests {
    use super::*;
    use crate::bus::EventBus;

    fn event(id: i64) -> Event {
        let mut e = Event::new(EventType::Push, Some(1), Some(2), serde_json::json!({}));
        e.id = id;
        e
    }

    /// An empty log reports nothing, and asking it for more than it
    /// holds yields what it holds.
    #[test]
    fn an_empty_log_reports_nothing() {
        let log = InMemoryEventLog::default();
        assert!(log.is_empty());
        assert!(log.recent(10).is_empty());
        assert_eq!(log.len(), 0);
    }

    /// The most recent events come back, newest first, at most as many
    /// as were asked for.
    #[test]
    fn the_recent_events_come_back_newest_first() {
        let log = InMemoryEventLog::default();
        log.push(event(1));
        log.push(event(2));
        log.push(event(3));

        let recent = log.recent(2);
        let ids: Vec<i64> = recent.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 2], "newest first, at most the limit");

        let recent = log.recent(10);
        let ids: Vec<i64> = recent.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 2, 1], "a limit past the end yields everything");
    }

    /// A subscriber that never spawns its listener keeps nothing: the
    /// log only fills once the listener task is running.
    #[tokio::test]
    async fn an_unspawned_subscriber_records_nothing() {
        let bus = EventBus::default();
        let log = Arc::new(InMemoryEventLog::default());

        let _subscriber = EventLogSubscriber::new(&bus, log.clone());
        bus.publish(EventType::Push, Some(1), Some(2), serde_json::json!({}));

        // Give the channel a moment; the subscriber holds a receiver
        // but nobody drains it.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            log.is_empty(),
            "only spawn_listener records events, got {:?}",
            log.recent(10)
        );
    }
}
