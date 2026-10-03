//! Background sealing and retention (retained-history contract).
//!
//! Oldest first, each sealed server journal file becomes one segment. Only
//! after the segment is committed does the commit thread checkpoint stream
//! state and delete the journal file. Retention then deletes whole segments,
//! oldest first, while the newest record of the oldest segment is older than
//! `retention_s` or segments exceed `retention_bytes` in total. Which
//! Segments go is decided by `fabric_core::retention`; this module deletes
//! them.

use crate::segment;
use crate::store::Intake;
use crate::store::SystemClock;
use fabric_app::retention::apply_retention;
pub use fabric_core::retention::Retention;
use fabric_core::retention::SegmentFacts;
use fabric_ports::{SegmentStore, StoreFailed};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn sealed_labels(journal: &Path) -> io::Result<Vec<u64>> {
    let mut labels: Vec<u64> = std::fs::read_dir(journal)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_prefix("sealed-")?
                .strip_suffix(".faj")?
                .parse()
                .ok()
        })
        .collect();
    labels.sort_unstable();
    Ok(labels)
}

/// The default number of Segments built at once: half the CPUs, one to four. Each
/// build of a 64 MiB journal file peaks near 5.5 times the file in memory today
/// (ADR-0022 will flatten that), so four stay inside the server's 3 GiB ceiling.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, 4)
}

/// One pass: seal every sealed journal file, `workers` at a time, then apply
/// retention. Segments are independent, so they are built in parallel; journal files
/// are reclaimed strictly oldest first, and not past the first file whose Segment
/// could not be built (ADR-0025).
pub fn pass(
    state_dir: &Path,
    intake: &Intake,
    retention: Retention,
    workers: usize,
) -> io::Result<()> {
    let journal: PathBuf = state_dir.join("journal");
    let segmented: BTreeSet<u64> = segment::list(state_dir)?
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    let sealed = sealed_labels(&journal)?;
    let pending: Vec<u64> = sealed
        .iter()
        .copied()
        .filter(|l| !segmented.contains(l))
        .collect();
    let mut failed: Option<(u64, io::Error)> = None;
    for chunk in pending.chunks(workers.max(1)) {
        let results: Vec<(u64, io::Result<()>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&label| {
                    let journal = &journal;
                    scope.spawn(move || {
                        let groups =
                            segment::read_sealed(&journal.join(format!("sealed-{label:020}.faj")))?;
                        segment::build(state_dir, label, &groups).map(|_| ())
                    })
                })
                .collect();
            chunk
                .iter()
                .zip(handles)
                .map(|(&label, h)| {
                    (
                        label,
                        h.join()
                            .unwrap_or_else(|_| Err(io::Error::other("sealing thread panicked"))),
                    )
                })
                .collect()
        });
        for (label, result) in results {
            if let Err(error) = result
                && failed.as_ref().is_none_or(|(l, _)| label < *l)
            {
                failed = Some((label, error));
            }
        }
        if failed.is_some() {
            break;
        }
    }
    for label in sealed {
        if failed.as_ref().is_some_and(|(l, _)| label >= *l) {
            break;
        }
        intake.reclaim(label)?;
    }
    if let Some((label, error)) = failed {
        return Err(io::Error::new(
            error.kind(),
            format!("sealing {label}: {error}"),
        ));
    }
    let segments_dir = segment::segments_dir(state_dir)?;
    let mut store = SegmentDir {
        state_dir,
        segments_dir,
    };
    apply_retention(&mut store, &SystemClock, retention)
        .map(|_| ())
        .map_err(|StoreFailed(why)| io::Error::other(why))
}

/// The Segment directory as the retention use case's `SegmentStore` port.
struct SegmentDir<'a> {
    state_dir: &'a Path,
    segments_dir: PathBuf,
}

impl SegmentStore for SegmentDir<'_> {
    fn sealed(&self) -> Result<Vec<(u64, SegmentFacts)>, StoreFailed> {
        let listed = segment::list(self.state_dir).map_err(|e| StoreFailed(e.to_string()))?;
        Ok(listed
            .into_iter()
            .map(|(label, m)| {
                let facts = SegmentFacts {
                    received_max_ns: m.received_max_ns,
                    bytes: m.files.values().map(|f| f.bytes).sum(),
                };
                (label, facts)
            })
            .collect())
    }

    /// Rename first so a crash never leaves a half-deleted Segment visible.
    fn delete(&mut self, label: u64) -> Result<(), StoreFailed> {
        let dir = self.segments_dir.join(segment::segment_name(label));
        let deleting = self.segments_dir.join(format!(".deleting-{label:020}"));
        let fail = |e: io::Error| StoreFailed(format!("deleting segment {label}: {e}"));
        std::fs::rename(&dir, &deleting).map_err(fail)?;
        std::fs::File::open(&self.segments_dir)
            .and_then(|d| d.sync_all())
            .map_err(fail)?;
        std::fs::remove_dir_all(&deleting).map_err(fail)
    }
}

/// Run passes every second until `stop` is set; the thread then drops its
/// intake so the commit thread can end.
pub fn spawn(
    state_dir: PathBuf,
    intake: Intake,
    retention: Retention,
    stop: Arc<AtomicBool>,
    workers: usize,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("fabric-sealer".into())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Err(error) = pass(&state_dir, &intake, retention, workers) {
                    eprintln!("fabric-server: sealing: {error}");
                }
                for _ in 0..10 {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
}
