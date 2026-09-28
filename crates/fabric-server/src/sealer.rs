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
pub use fabric_core::retention::Retention;
use fabric_core::retention::{SegmentFacts, segments_to_delete};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One pass: seal every sealed journal file, then apply retention.
pub fn pass(state_dir: &Path, intake: &Intake, retention: Retention) -> io::Result<()> {
    let journal: PathBuf = state_dir.join("journal");
    let segmented: BTreeSet<u64> = segment::list(state_dir)?
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    for label in sealed_labels(&journal)? {
        if !segmented.contains(&label) {
            let groups = segment::read_sealed(&journal.join(format!("sealed-{label:020}.faj")))?;
            segment::build(state_dir, label, &groups)?;
        }
        intake.reclaim(label)?;
    }
    let segments_dir = segment::segments_dir(state_dir)?;
    let segments = segment::list(state_dir)?;
    let facts: Vec<SegmentFacts> = segments
        .iter()
        .map(|(_, m)| SegmentFacts {
            received_max_ns: m.received_max_ns,
            bytes: m.files.values().map(|f| f.bytes).sum(),
        })
        .collect();
    let doomed = segments_to_delete(&facts, retention, now_ns());
    for (label, _) in segments.iter().take(doomed) {
        let dir = segments_dir.join(segment::segment_name(*label));
        let deleting = segments_dir.join(format!(".deleting-{label:020}"));
        std::fs::rename(&dir, &deleting)?;
        std::fs::File::open(&segments_dir)?.sync_all()?;
        std::fs::remove_dir_all(&deleting)?;
    }
    Ok(())
}

/// Run passes every second until `stop` is set; the thread then drops its
/// intake so the commit thread can end.
pub fn spawn(
    state_dir: PathBuf,
    intake: Intake,
    retention: Retention,
    stop: Arc<AtomicBool>,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("fabric-sealer".into())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Err(error) = pass(&state_dir, &intake, retention) {
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
