//! Derived evidence for immutable, fully included Segments. This is neither an
//! authorization cache nor a file lease. Every selection applies today's scope;
//! partial snapshots still scan exact records, and callers check raw availability.
use crate::{rows::latest_signal_observation_bytes, segment};
use fabric_core::query::Snapshot;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const RESIDENT_BYTES: usize = 8 * 1024 * 1024;
const BUILD_BYTES: usize = 1024 * 1024;
const SEGMENTS: usize = 4096;
// Conservative retained-allocation charges, not process RSS measurements.
const NODE_OVERHEAD: usize = 512;
pub(super) type Selected = ((u64, u64), BTreeMap<String, u64>);
type Signals = [Option<(u64, u64, u64)>; 3]; // receive min/max, newest observation

#[derive(PartialEq, Eq)]
struct Stamp {
    file: [u64; 7],
    groups: (u64, u64),
    expected: (u64, u64, String),
}
impl Stamp {
    fn read(dir: &Path, manifest: &segment::Manifest) -> io::Result<Self> {
        let expected = manifest
            .files
            .get("batches.parquet")
            .ok_or_else(|| io::Error::other("raw file missing from manifest"))?;
        let m = std::fs::metadata(dir.join("batches.parquet"))?;
        Ok(Self {
            file: [
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime() as u64,
                m.mtime_nsec() as u64,
                m.ctime() as u64,
                m.ctime_nsec() as u64,
            ],
            groups: (manifest.first_group, manifest.last_group),
            expected: (expected.bytes, expected.rows, expected.sha256.clone()),
        })
    }
}

struct Cached {
    dir: PathBuf,
    stamp: Stamp,
    nodes: BTreeMap<String, Signals>,
    charge: usize,
}

#[derive(Default)]
pub(super) struct Cache {
    entries: VecDeque<Cached>,
    charged: usize,
    #[cfg(test)]
    scans: usize,
}

fn selected_empty() -> Selected {
    ((u64::MAX, 0), BTreeMap::new())
}

fn add_selected(selected: &mut Selected, label: &str, value: (u64, u64, u64)) {
    selected.0.0 = selected.0.0.min(value.0);
    selected.0.1 = selected.0.1.max(value.1);
    let newest = selected.1.entry(label.to_owned()).or_insert(value.2);
    *newest = (*newest).max(value.2);
}

impl Cache {
    pub(super) fn select(
        &mut self,
        dir: &Path,
        manifest: &segment::Manifest,
        snapshot: Snapshot,
        table: segment::Table,
        allowed: &HashSet<String>,
        node: &Option<String>,
    ) -> io::Result<Selected> {
        let signal = match table {
            segment::Table::Logs => 0,
            segment::Table::Metrics => 1,
            segment::Table::Spans => 2,
        };
        let wanted =
            |label: &str| allowed.contains(label) && node.as_deref().is_none_or(|n| n == label);
        let full =
            snapshot.contains(manifest.first_group) && snapshot.contains(manifest.last_group);
        let stamp = Stamp::read(dir, manifest)?;
        if let Some(index) = self.entries.iter().position(|entry| entry.dir == dir) {
            if self.entries[index].stamp != stamp {
                self.charged -= self.entries.remove(index).unwrap().charge;
            } else if full {
                let entry = &self.entries[index];
                let mut result = selected_empty();
                for (label, signals) in &entry.nodes {
                    if wanted(label)
                        && let Some(value) = signals[signal]
                    {
                        add_selected(&mut result, label, value);
                    }
                }
                return Ok(result);
            }
        }
        #[cfg(test)]
        {
            self.scans += 1;
        }
        let mut selected = selected_empty();
        let mut failure = None;
        let mut charge = 512 + dir.as_os_str().len() + stamp.expected.2.capacity();
        let mut nodes = (full && charge <= BUILD_BYTES).then(BTreeMap::<String, Signals>::new);
        segment::scan_batches_borrowed_one_at_a_time(dir, manifest, |group, entry| {
            let included = snapshot.contains(group) && wanted(entry.label);
            if !snapshot.contains(group) {
                nodes = None;
            }
            // Digest checks still cover every raw row. An excluded record's
            // typed failure must not change the authorized answer.
            if !included && nodes.is_none() {
                return;
            }
            let times = match latest_signal_observation_bytes(entry.batch) {
                Ok(times) => times,
                Err(error) => {
                    nodes = None;
                    if included && failure.is_none() {
                        failure = Some(error);
                    }
                    return;
                }
            };
            if included && let Some(time) = times[signal] {
                add_selected(
                    &mut selected,
                    entry.label,
                    (entry.received_unix_nano, entry.received_unix_nano, time),
                );
            }
            // Admit before allocation; over-budget metadata uses the exact
            // scan just performed and leaves no partially populated cache.
            if let Some(all) = &nodes
                && !all.contains_key(entry.label)
                && charge
                    .saturating_add(NODE_OVERHEAD)
                    .saturating_add(entry.label.len())
                    > BUILD_BYTES
            {
                nodes = None;
            }
            if let Some(all) = &mut nodes {
                let signals = all.entry(entry.label.to_owned()).or_insert_with(|| {
                    charge += NODE_OVERHEAD + entry.label.len();
                    [None; 3]
                });
                for (slot, time) in signals.iter_mut().zip(times) {
                    if let Some(time) = time {
                        let old = slot.unwrap_or((u64::MAX, 0, 0));
                        *slot = Some((
                            old.0.min(entry.received_unix_nano),
                            old.1.max(entry.received_unix_nano),
                            old.2.max(time),
                        ));
                    }
                }
            }
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        if Stamp::read(dir, manifest)? != stamp {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "raw Segment changed during evidence scan",
            ));
        }
        if let Some(nodes) = nodes {
            while self.charged.saturating_add(charge) > RESIDENT_BYTES
                || self.entries.len() >= SEGMENTS
            {
                self.charged -= self.entries.pop_front().unwrap().charge;
            }
            self.entries.push_back(Cached {
                dir: dir.to_owned(),
                stamp,
                nodes,
                charge,
            });
            self.charged += charge;
        }
        Ok(selected)
    }
}

#[cfg(test)]
mod tests;
