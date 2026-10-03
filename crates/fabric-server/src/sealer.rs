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
/// could not be built (ADR-0025). Reclaim runs before builds and after each
/// worker group, so a later group does not hold an already published prefix.
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
    build_and_reclaim(
        &sealed,
        segmented,
        workers,
        |label| {
            let groups = segment::read_sealed(&journal.join(format!("sealed-{label:020}.faj")))?;
            segment::build(state_dir, label, &groups).map(|_| ())
        },
        |label| intake.reclaim(label),
    )?;
    let segments_dir = segment::segments_dir(state_dir)?;
    let mut store = SegmentDir {
        state_dir,
        segments_dir,
    };
    apply_retention(&mut store, &SystemClock, retention)
        .map(|_| ())
        .map_err(|StoreFailed(why)| io::Error::other(why))
}

// The sorted snapshot is fixed for this pass. Publication can finish out of
// order, but only its contiguous oldest prefix may release journal capacity.
// Keep these callbacks private: they expose scheduling to deterministic tests,
// while production still uses the existing Segment and Intake effects.
fn build_and_reclaim(
    sealed: &[u64],
    mut published: BTreeSet<u64>,
    workers: usize,
    build: impl Fn(u64) -> io::Result<()> + Sync,
    mut reclaim: impl FnMut(u64) -> io::Result<()>,
) -> io::Result<()> {
    let pending: Vec<u64> = sealed
        .iter()
        .copied()
        .filter(|l| !published.contains(l))
        .collect();
    let mut next = 0;
    reclaim_prefix(sealed, &published, &mut next, &mut reclaim)?;
    for chunk in pending.chunks(workers.max(1)) {
        let results: Vec<(u64, io::Result<()>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&label| {
                    let build = &build;
                    scope.spawn(move || build(label))
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
        let mut failed = None;
        for (label, result) in results {
            match result {
                Ok(()) => {
                    published.insert(label);
                }
                Err(error) if failed.is_none() => failed = Some((label, error)),
                Err(_) => {}
            }
        }
        reclaim_prefix(sealed, &published, &mut next, &mut reclaim)?;
        if let Some((label, error)) = failed {
            return Err(io::Error::new(
                error.kind(),
                format!("sealing {label}: {error}"),
            ));
        }
    }
    Ok(())
}

fn reclaim_prefix(
    sealed: &[u64],
    published: &BTreeSet<u64>,
    next: &mut usize,
    reclaim: &mut impl FnMut(u64) -> io::Result<()>,
) -> io::Result<()> {
    while let Some(&label) = sealed.get(*next) {
        if !published.contains(&label) {
            break;
        }
        reclaim(label)?;
        *next += 1;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Answer, CommitMode, Store, Submission, identify_strand};
    use fabric_frame::envelope::Batch;
    use prost::Message;
    use std::sync::{Mutex, mpsc};

    #[test]
    fn published_prefix_is_reclaimed_before_building() {
        let reclaimed = Mutex::new(Vec::new());
        build_and_reclaim(
            &[1, 2, 3, 4],
            BTreeSet::from([1, 2, 4]),
            2,
            |label| {
                assert_eq!(label, 3);
                assert_eq!(*reclaimed.lock().unwrap(), [1, 2]);
                Ok(())
            },
            |label| {
                reclaimed.lock().unwrap().push(label);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*reclaimed.lock().unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn completed_group_reclaims_while_later_group_is_blocked() {
        for workers in [1, 2] {
            let reclaimed = Mutex::new(Vec::new());
            let (release, released) = mpsc::channel();
            let released = Mutex::new(released);
            let (entered, waiting) = mpsc::channel();
            std::thread::scope(|scope| {
                let run = scope.spawn(|| {
                    build_and_reclaim(
                        &[1, 2, 3, 4],
                        BTreeSet::new(),
                        workers,
                        |label| {
                            if label == workers as u64 + 1 {
                                entered.send(()).unwrap();
                                released
                                    .lock()
                                    .unwrap()
                                    .recv_timeout(Duration::from_secs(10))
                                    .unwrap();
                            }
                            Ok(())
                        },
                        |label| {
                            reclaimed.lock().unwrap().push(label);
                            Ok(())
                        },
                    )
                });
                let arrived = waiting.recv_timeout(Duration::from_secs(5));
                let prefix = reclaimed.lock().unwrap().clone();
                // Release even on timeout: the assertion must not strand a worker.
                let _ = release.send(());
                let result = run.join().unwrap();
                assert!(arrived.is_ok(), "later worker did not reach its barrier");
                result.unwrap();
                assert_eq!(prefix, (1..=workers as u64).collect::<Vec<_>>());
            });
            assert_eq!(*reclaimed.lock().unwrap(), [1, 2, 3, 4]);
        }
    }

    #[test]
    fn failure_or_panic_never_reclaims_across_a_hole() {
        for panic in [false, true] {
            let built = Mutex::new(Vec::new());
            let mut reclaimed = Vec::new();
            let error = build_and_reclaim(
                &[1, 2, 3, 4],
                BTreeSet::from([1]),
                2,
                |label| {
                    built.lock().unwrap().push(label);
                    if label == 2 {
                        assert!(!panic, "injected worker panic");
                        return Err(io::Error::other("injected build failure"));
                    }
                    Ok(())
                },
                |label| {
                    reclaimed.push(label);
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(error.to_string().starts_with("sealing 2:"));
            assert_eq!(reclaimed, [1]);
            let mut built = built.into_inner().unwrap();
            built.sort_unstable();
            assert_eq!(built, [2, 3]);
        }
    }

    #[test]
    fn successful_prefix_in_a_failed_group_is_reclaimed() {
        let mut reclaimed = Vec::new();
        let error = build_and_reclaim(
            &[1, 2, 3, 4],
            BTreeSet::new(),
            2,
            |label| {
                if label == 2 {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            },
            |label| {
                reclaimed.push(label);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.to_string().starts_with("sealing 2:"));
        assert_eq!(reclaimed, [1]);
    }

    #[test]
    fn reclaim_error_stops_new_groups_and_retry_skips_published_labels() {
        let built = Mutex::new(BTreeSet::new());
        let mut attempted = Vec::new();
        let error = build_and_reclaim(
            &[1, 2, 3, 4],
            BTreeSet::new(),
            2,
            |label| {
                built.lock().unwrap().insert(label);
                Ok(())
            },
            |label| {
                attempted.push(label);
                Err(io::Error::other("checkpoint failed"))
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "checkpoint failed");
        assert_eq!(attempted, [1]);
        let published = built.into_inner().unwrap();
        assert_eq!(published, BTreeSet::from([1, 2]));
        let retry_built = Mutex::new(BTreeSet::new());
        let mut reclaimed = Vec::new();
        build_and_reclaim(
            &[1, 2, 3, 4],
            published,
            2,
            |label| {
                retry_built.lock().unwrap().insert(label);
                Ok(())
            },
            |label| {
                reclaimed.push(label);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(retry_built.into_inner().unwrap(), BTreeSet::from([3, 4]));
        assert_eq!(reclaimed, [1, 2, 3, 4]);
    }

    fn submit(intake: &Intake, bytes: &[u8]) -> Answer {
        let (strand, sequence) = identify_strand(bytes).unwrap();
        let (reply, answer) = tokio::sync::oneshot::channel();
        intake.submit(Submission {
            label: "fixture".into(),
            strand,
            sequence,
            bytes: bytes.to_vec(),
            reply,
        });
        answer.blocking_recv().unwrap()
    }

    #[test]
    fn real_checkpoint_failure_preserves_journal_and_retry_preserves_exact_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "fabric-reclaim-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let limit = 16 * 1024 * 1024;
        let (intake, thread) = Store::open_with(&dir, limit, 1024, CommitMode::INDIVIDUAL)
            .unwrap()
            .spawn_joinable()
            .unwrap();
        let batches: Vec<_> = (1..=5)
            .map(|sequence| {
                Batch {
                    version: 1,
                    node_id: vec![7; 16],
                    generation: 1,
                    sequence,
                    collection_gaps: vec!["fixture".repeat(32); 8],
                    ..Default::default()
                }
                .encode_to_vec()
            })
            .collect();
        for (i, bytes) in batches.iter().enumerate() {
            assert_eq!(submit(&intake, bytes), Answer::Ack(i as u64 + 1));
        }
        let journal = dir.join("journal");
        let sealed = sealed_labels(&journal).unwrap();
        assert_eq!(sealed.len(), 4);
        let first = sealed[0];
        let groups =
            segment::read_sealed(&journal.join(format!("sealed-{first:020}.faj"))).unwrap();
        segment::build(&dir, first, &groups).unwrap();
        let retention = Retention {
            max_age_s: u64::MAX,
            max_bytes: u64::MAX,
        };
        // File::create cannot replace this owned directory: checkpoint fails
        // before deletion, without disk exhaustion or privileged fault injection.
        std::fs::create_dir(dir.join("streams.json.tmp")).unwrap();
        assert!(pass(&dir, &intake, retention, 2).is_err());
        assert_eq!(sealed_labels(&journal).unwrap(), sealed);
        assert_eq!(segment::list(&dir).unwrap().len(), 1);
        std::fs::remove_dir(dir.join("streams.json.tmp")).unwrap();
        pass(&dir, &intake, retention, 2).unwrap();
        assert!(sealed_labels(&journal).unwrap().is_empty());
        drop(intake);
        thread.join().unwrap();
        let store = Store::open(&dir, limit, CommitMode::INDIVIDUAL).unwrap();
        assert_eq!(store.committed_through(&([7; 16], 1)), 5);
        let (intake, thread) = store.spawn_joinable().unwrap();
        assert_eq!(submit(&intake, &batches[4]), Answer::Ack(5));
        let mut different = Batch::decode(batches[4].as_slice()).unwrap();
        different.collection_gaps.push("different bytes".into());
        assert_eq!(
            submit(&intake, &different.encode_to_vec()),
            Answer::Conflict(5)
        );
        drop(intake);
        thread.join().unwrap();
        let mut replayed = Vec::new();
        Store::replay(&dir, limit, |entry| {
            replayed.push(entry.batch.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(replayed, batches);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
