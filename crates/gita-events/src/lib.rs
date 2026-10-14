//! Event bus for Gita.
//!
//! The event bus is the nervous system of the AI-native design. Every
//! meaningful activity (push, PR opened, issue created, ...) is
//! published as a structured event. AI agents and internal modules can
//! subscribe to the stream and react.

use chrono::Utc;
use gita_core::{Event, EventType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::warn;

/// A publisher for events. This is a handle that can be cloned and
/// shared across the application.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    /// Optional persistence callback (e.g. write event to database).
    /// Kept as an `Arc` so the bus can be cloned cheaply.
    persist: Option<Arc<dyn Fn(Event) + Send + Sync>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(None)
    }
}

impl EventBus {
    /// Create a new event bus with an optional persistence callback.
    pub fn new(persist: Option<Arc<dyn Fn(Event) + Send + Sync>>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx, persist }
    }

    /// Publish an event to all subscribers and to the persistence hook.
    pub fn publish(
        &self,
        event_type: EventType,
        repo_id: Option<i64>,
        actor_id: Option<i64>,
        payload: serde_json::Value,
    ) -> Event {
        let event = Event {
            id: 0,
            event_type,
            repo_id,
            actor_id,
            payload,
            created_at: Utc::now(),
        };
        if let Some(persist) = &self.persist {
            persist(event.clone());
        }
        // Ignore errors: a full channel or no receivers is not fatal.
        let _ = self.tx.send(event.clone());
        event
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Take a snapshot of recent events (e.g. for the /api/events endpoint).
    /// This is a best-effort helper; the authoritative source is the database.
    pub fn recent_snapshot(&self, _limit: usize) -> Vec<Event> {
        warn!("recent_snapshot is a no-op without a database-backed event log");
        Vec::new()
    }
}

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
    log: Arc<InMemoryEventLog>,
    _rx: tokio::sync::broadcast::Receiver<Event>,
}

impl EventLogSubscriber {
    pub fn new(bus: &EventBus, log: Arc<InMemoryEventLog>) -> Self {
        let rx = bus.subscribe();
        Self { log, _rx: rx }
    }

    pub fn spawn_listener(bus: EventBus, log: Arc<InMemoryEventLog>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = bus.subscribe();
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