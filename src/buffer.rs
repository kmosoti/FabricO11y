//! A single-threaded, bounded queue of events.
//!
//! A full queue rejects an event by returning ownership to the caller.

use crate::Event;
use std::collections::VecDeque;
use std::num::NonZeroUsize;

pub struct EventBuffer {
    queue: VecDeque<Event>,
    capacity: NonZeroUsize,
}

impl EventBuffer {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            queue: VecDeque::new(),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity.get()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Enqueue one event, or return that same owned event when full.
    #[allow(
        clippy::result_large_err,
        reason = "return the owned event on rejection without boxing every attempted push"
    )]
    pub fn try_push(&mut self, event: Event) -> Result<(), Event> {
        if self.queue.len() >= self.capacity.get() {
            Err(event)
        } else {
            self.queue.push_back(event);
            Ok(())
        }
    }

    /// Remove up to `max_events` oldest events and transfer ownership to the caller.
    pub fn take_batch(&mut self, max_events: NonZeroUsize) -> Vec<Event> {
        let count = self.queue.len().min(max_events.get());
        self.queue.drain(..count).collect()
    }
}
