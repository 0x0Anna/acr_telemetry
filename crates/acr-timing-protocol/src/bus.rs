//! In-process broadcast bus (UDP adapter can wrap the same JSON later).

use std::sync::{Arc, Mutex};

use crate::events::TimingEvent;

#[derive(Clone)]
pub struct EventSender {
    inner: Arc<Mutex<Vec<EventReceiver>>>,
}

#[derive(Clone)]
pub struct EventReceiver {
    queue: Arc<Mutex<Vec<TimingEvent>>>,
}

impl EventSender {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn subscribe(&self) -> EventReceiver {
        let rx = EventReceiver {
            queue: Arc::new(Mutex::new(Vec::new())),
        };
        self.inner.lock().unwrap().push(rx.clone());
        rx
    }

    pub fn publish(&self, event: TimingEvent) {
        let subs = self.inner.lock().unwrap();
        for rx in subs.iter() {
            rx.queue.lock().unwrap().push(event.clone());
        }
    }
}

impl EventReceiver {
    pub fn drain(&self) -> Vec<TimingEvent> {
        let mut q = self.queue.lock().unwrap();
        std::mem::take(&mut *q)
    }
}
