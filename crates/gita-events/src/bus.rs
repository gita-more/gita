//! The event bus: a publisher handle that can be cloned and shared
//! across the application.

use std::sync::Arc;

use gita_core::{Event, EventType};
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
        let event = Event::new(event_type, repo_id, actor_id, payload);
        if let Some(persist) = &self.persist {
            persist(event.clone());
        }
        // Ignore errors: a full channel or no receivers is not fatal.
        let _ = self.tx.send(event.clone());
        event
    }

    /// Publish an event the caller has already persisted.
    ///
    /// `event.id` carries the id of the row the caller inserted. The
    /// persistence hook uses a non-zero id as the "already inserted"
    /// marker and skips the insert, so the row is not written twice.
    ///
    /// This exists because the persist hook is synchronous while the
    /// database write is not: the hook cannot return the id it just
    /// generated. A caller that needs the id — to set a foreign key, say
    /// `pipeline_runs.trigger_event_id` — has to insert the row itself
    /// and then publish the event it built.
    pub fn publish_persisted(&self, event: Event) -> Event {
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
