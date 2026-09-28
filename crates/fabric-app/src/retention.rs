//! Use case: apply retention to sealed history.
//!
//! The core decides how many of the oldest Segments exceed the limits
//! (`fabric_core::retention`); this use case reads the clock once, asks the
//! store for its Segments, and deletes exactly those, oldest first. A failed
//! delete stops the pass; the next pass decides again from what remains.

use fabric_core::retention::{Retention, SegmentFacts, segments_to_delete};
use fabric_ports::{Clock, SegmentStore, StoreFailed};

/// Returns the number of Segments deleted.
pub fn apply_retention<S: SegmentStore, C: Clock>(
    store: &mut S,
    clock: &C,
    limits: Retention,
) -> Result<usize, StoreFailed> {
    let sealed = store.sealed()?;
    let facts: Vec<SegmentFacts> = sealed.iter().map(|(_, facts)| *facts).collect();
    let doomed = segments_to_delete(&facts, limits, clock.now_unix_nano());
    for (label, _) in sealed.iter().take(doomed) {
        store.delete(*label)?;
    }
    Ok(doomed)
}
