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
