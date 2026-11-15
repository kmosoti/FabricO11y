//! Fixed-capacity non-blocking `mpsc` telemetry queue with explicit drop counters.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// Bounded async channel wrapper enforcing drop-with-counter backpressure via `try_send`.
#[derive(Debug, Clone)]
pub struct BoundedTelemetryQueue<T> {
    tx: mpsc::Sender<T>,
    dropped: Arc<AtomicU64>,
    capacity: usize,
}

impl<T> BoundedTelemetryQueue<T> {
    /// Creates a bounded queue and its receiver handle plus shared drop counter.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<T>, Arc<AtomicU64>) {
        let cap = capacity.max(8);
        let (tx, rx) = mpsc::channel(cap);
        let dropped = Arc::new(AtomicU64::new(0));
        (
            Self {
                tx,
                dropped: dropped.clone(),
                capacity: cap,
            },
            rx,
            dropped,
        )
    }

    /// Non-blocking submission that never stalls the caller; counts saturated drops.
    pub fn try_emit(&self, item: T) -> bool {
        match self.tx.try_send(item) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_emit_never_blocks_and_counts_drops() {
        let (queue, mut rx, counter) = BoundedTelemetryQueue::new(8);
        for i in 0..12u32 {
            let _ = queue.try_emit(i);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 4);
        assert_eq!(rx.try_recv().unwrap(), 0);
    }
}
