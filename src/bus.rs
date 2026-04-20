use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::events::{Event, EventSource};

#[derive(Debug, Clone)]
pub struct EventEnvelope {
    pub id: u64,
    pub at: DateTime<Utc>,
    pub source: EventSource,
    pub payload: Event,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<EventEnvelope>,
    next_id: Arc<AtomicU64>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn publish(&self, source: EventSource, payload: Event) {
        let envelope = EventEnvelope {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            at: Utc::now(),
            source,
            payload,
        };
        let _ = self.tx.send(envelope);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.tx.subscribe()
    }
}
