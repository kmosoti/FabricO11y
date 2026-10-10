//! Storage-owned derived read state. Durable custody remains journal/Segments.
//! Views own metadata handles; their lifetimes never authorize expired snapshots.
use crate::coupled_catalog::{Cache, SegmentSources, Snapshot};
use crate::segment::{self, GroupBounds, Table};
use crate::store::Group;
use crate::tail::{TailEntry, TailReader, WalkState, interrupted};
use fabric_core::query::Window;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterKind {
    None,
    Logs,
    Spans,
}

pub(crate) struct CatalogRequest<'a> {
    pub oldest: Option<u64>,
    pub newest: u64,
    pub table: Table,
    pub window: &'a Window,
    pub node: &'a Option<String>,
    pub authorized_nodes: Option<&'a HashSet<String>>,
    pub filter: FilterKind,
}

/// Everything a query reads, fixed for one snapshot.
pub(crate) struct Sources {
    pub segments: SegmentSources,
    /// Journal groups inside the snapshot that no segment covers.
    pub journal: Vec<Group>,
    pub oldest_group: u64,
    /// Walk plan: selected tail entries left undecoded until the walk reaches them,
    /// the live journal files, receive bounds and freshness over the whole tail of the
    /// snapshot (which `journal` then no longer holds), and each Segment's row-group
    /// bounds for the queried table, or the error that made them unreadable.
    pub tail: Vec<TailEntry>,
    pub tail_paths: HashMap<u64, PathBuf>,
    pub tail_evidence: Option<((u64, u64), BTreeMap<String, u64>)>,
    pub bounds: Vec<Result<GroupBounds, String>>,
    /// Walk plan, logs queries with a needle of three or more bytes: each Segment's
    /// verified row-group text filters, if it has them (ADR-0024 part 2).
    pub filters: Vec<Option<std::sync::Arc<Vec<crate::text_filter::GroupFilter>>>>,
    /// Walk plan: complete tail blocks holding selected entries, each with those
    /// entries (by frame offset and index) and the bounds of their queried rows.
    pub blocks: Vec<TailBlockSource>,
}

/// A tail block as a source of one query (ADR-0024 part 3).
pub(crate) struct TailBlockSource {
    pub block: std::sync::Arc<crate::tail::TailBlock>,
    pub wanted: std::collections::HashSet<(u64, u32)>,
    pub bounds: (u64, u64),
}

pub(crate) struct ReadCatalog {
    state_dir: PathBuf,
    state: Mutex<WalkState>,
    shared_metadata: bool,
    reuse: Option<Mutex<Cache>>,
}

impl ReadCatalog {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_owned(),
            state: Mutex::new(WalkState::default()),
            shared_metadata: false,
            reuse: None,
        }
    }

    pub fn use_shared_metadata(&mut self) {
        self.shared_metadata = true;
    }

    pub fn use_descriptor_reuse(&mut self) {
        self.shared_metadata = true;
        self.reuse = Some(Mutex::new(Cache::default()));
    }

    pub fn reuse_stats(&self) -> Option<(u64, u64, u64, usize, usize)> {
        self.reuse
            .as_ref()
            .map(|cache| cache.lock().unwrap_or_else(|e| e.into_inner()).stats())
    }

    pub fn set_block_records(&self, records: usize) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .block_records = records;
    }

    pub fn blocks(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .blocks
            .len()
    }

    /// Explicit synchronous preparation; never called from commit/ACK paths.
    /// Work is proportional to discovered files and newly appended complete frames.
    pub fn refresh(&self, newest: u64) -> io::Result<()> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("catalog_refresh_inclusive");
        #[cfg(feature = "phase-probe")]
        let _wait = fabric_frame::probe::span("catalog_lock_wait");
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(feature = "phase-probe")]
        drop(_wait);
        #[cfg(feature = "phase-probe")]
        let _hold = fabric_frame::probe::span("catalog_lock_hold");
        let listed = state.segments(&self.state_dir, self.shared_metadata)?;
        let before: HashSet<_> = listed.iter().map(|(label, _)| *label).collect();
        let mut covered: Vec<_> = listed
            .into_iter()
            .filter(|(_, m)| m.first_group <= newest)
            .map(|(_, m)| (m.first_group, m.last_group))
            .collect();
        covered.sort_unstable();
        state.extend(&self.state_dir.join("journal"), &covered)?;
        let after: HashSet<_> = segment::labels(&self.state_dir)?.into_iter().collect();
        if before != after {
            return Err(interrupted("segment coverage changed during discovery"));
        }
        if let Some(cache) = &self.reuse {
            let mut labels: Vec<_> = after.into_iter().collect();
            labels.sort_unstable();
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invalidate(&labels);
        }
        Ok(())
    }

    pub fn view(&self, request: CatalogRequest<'_>) -> io::Result<Sources> {
        self.view_inner(request, || {})
    }

    #[cfg(test)]
    pub(crate) fn view_at_discovery_cut(
        &self,
        request: CatalogRequest<'_>,
        after_segments: impl FnOnce(),
    ) -> io::Result<Sources> {
        self.view_inner(request, after_segments)
    }

    fn view_inner(
        &self,
        request: CatalogRequest<'_>,
        after_segments: impl FnOnce(),
    ) -> io::Result<Sources> {
        let CatalogRequest {
            oldest,
            newest,
            table,
            window,
            node,
            authorized_nodes,
            filter,
        } = request;
        #[cfg(feature = "phase-probe")]
        let _wait = fabric_frame::probe::span("catalog_lock_wait");
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(feature = "phase-probe")]
        drop(_wait);
        #[cfg(feature = "phase-probe")]
        let _hold = fabric_frame::probe::span("catalog_lock_hold");
        let mut cache = self
            .reuse
            .as_ref()
            .map(|c| c.lock().unwrap_or_else(|e| e.into_inner()));
        let reused = if let Some(cache) = &mut cache {
            let mut labels = segment::labels(&self.state_dir)?;
            labels.sort_unstable();
            cache.lookup(&labels)
        } else {
            None
        };
        let snapshot = if let Some(snapshot) = reused {
            Some(snapshot)
        } else if let Some(cache) = &mut cache {
            let segments_dir = segment::segments_dir(&self.state_dir)?;
            let listed = state.segments(&self.state_dir, self.shared_metadata)?;
            let mut labels: Vec<_> = listed.iter().map(|(label, _)| *label).collect();
            labels.sort_unstable();
            let mut covered: Vec<_> = listed
                .iter()
                .map(|(_, m)| (m.first_group, m.last_group))
                .collect();
            covered.sort_unstable();
            let descriptors = listed
                .into_iter()
                .map(|(label, m)| (segments_dir.join(segment::segment_name(label)), m))
                .collect();
            cache.publish(Snapshot {
                labels,
                descriptors,
                covered,
                charged_bytes: 0,
            })
        } else {
            None
        };
        let (before, mut segments, covered) = if let Some(snapshot) = snapshot {
            let before: HashSet<_> = snapshot.labels.iter().copied().collect();
            let covered: Vec<_> = snapshot
                .covered
                .iter()
                .copied()
                .filter(|(first, _)| *first <= newest)
                .collect();
            (before, SegmentSources::reused(snapshot, newest), covered)
        } else {
            let segments_dir = segment::segments_dir(&self.state_dir)?;
            let listed = state.segments(&self.state_dir, self.shared_metadata)?;
            let before: HashSet<_> = listed.iter().map(|(label, _)| *label).collect();
            let mut covered = Vec::new();
            let mut segments = Vec::new();
            for (label, manifest) in listed {
                if manifest.first_group > newest {
                    continue;
                }
                covered.push((manifest.first_group, manifest.last_group));
                segments.push((segments_dir.join(segment::segment_name(label)), manifest));
            }
            covered.sort_unstable();
            (before, SegmentSources::Owned(segments), covered)
        };
        drop(cache);
        after_segments();
        let tail_paths = state.extend(&self.state_dir.join("journal"), &covered)?;
        // Publication/reclaim may have happened between the two listings. Even
        // a cold index must reject that split view and let History retry.
        let after: HashSet<_> = segment::labels(&self.state_dir)?.into_iter().collect();
        if before != after {
            return Err(interrupted("segment coverage changed during discovery"));
        }
        let oldest_group = segments
            .iter()
            .map(|(_, m)| m.first_group)
            .chain(
                state
                    .entries
                    .iter()
                    .map(|e| e.group)
                    .filter(|g| *g <= newest),
            )
            .min()
            .unwrap_or(newest + 1);
        let floor = oldest.unwrap_or(oldest_group);
        segments.retain(|(_, m)| m.last_group >= floor);
        let kind = |e: &TailEntry| match table {
            Table::Logs => e.logs,
            Table::Metrics => e.metrics,
            Table::Spans => e.spans,
        };
        let mut received = (u64::MAX, 0_u64);
        let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
        let mut lazy = Vec::new();
        let mut eager = Vec::new();
        let mut blocks: Vec<TailBlockSource> = Vec::new();
        let mut block_at: HashMap<u32, usize> = HashMap::new();
        for e in &state.entries {
            if e.group > newest || e.group < floor {
                continue;
            }
            let label = &state.labels[e.label as usize];
            if authorized_nodes.is_some_and(|allowed| {
                !allowed.contains(label) || node.as_deref().is_some_and(|n| n != label)
            }) {
                continue;
            }
            if authorized_nodes.is_none() || kind(e) != crate::tail::NONE {
                received = (received.0.min(e.received), received.1.max(e.received));
            }
            let newest_time = if authorized_nodes.is_some() {
                kind(e).1
            } else {
                e.logs.1.max(e.metrics.1).max(e.spans.1)
            };
            if (authorized_nodes.is_some() && kind(e) != crate::tail::NONE)
                || (authorized_nodes.is_none()
                    && (e.logs != crate::tail::NONE
                        || e.metrics != crate::tail::NONE
                        || e.spans != crate::tail::NONE))
            {
                let f = freshness.entry(label.clone()).or_default();
                *f = (*f).max(newest_time);
            }
            if node.as_deref().is_some_and(|n| n != label) {
                continue;
            }
            if e.has_gaps && window.contains(e.received) {
                eager.push(*e);
            } else {
                let (min, max) = kind(e);
                if max >= window.from_ns && min < window.to_ns {
                    match e
                        .block
                        .and_then(|b| state.blocks.get(&b).map(|block| (b, block)))
                    {
                        Some((b, block)) => {
                            let j = *block_at.entry(b).or_insert_with(|| {
                                blocks.push(TailBlockSource {
                                    block: block.clone(),
                                    wanted: std::collections::HashSet::new(),
                                    bounds: crate::tail::NONE,
                                });
                                blocks.len() - 1
                            });
                            let s = &mut blocks[j];
                            s.wanted.insert((e.offset, e.index));
                            s.bounds = (s.bounds.0.min(min), s.bounds.1.max(max));
                        }
                        None => lazy.push(*e),
                    }
                }
            }
        }
        let bounds = segments
            .iter()
            .map(|(dir, manifest)| {
                state
                    .bounds(dir, manifest, table)
                    .map_err(|e| e.to_string())
            })
            .collect();
        let needle = filter == FilterKind::Logs;
        let trace = filter == FilterKind::Spans;
        let filters = if needle {
            segments
                .iter()
                .map(|(dir, manifest)| state.filters(dir, manifest))
                .collect()
        } else if trace {
            segments
                .iter()
                .map(|(dir, manifest)| state.spans_filters(dir, manifest))
                .collect()
        } else {
            Vec::new()
        };
        drop(state);
        #[cfg(feature = "phase-probe")]
        drop(_hold);
        let mut reader = TailReader::new(&tail_paths);
        let mut journal: Vec<Group> = Vec::new();
        let mut eager_bytes = 0usize;
        for e in &eager {
            let group = reader.group(e.file_first, e.offset)?;
            if group.group_sequence != e.group {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tail entry group identity mismatch",
                ));
            }
            let entry = group
                .entries
                .get(e.index as usize)
                .ok_or_else(|| interrupted("journal moved"))?;
            eager_bytes = eager_bytes.saturating_add(entry.batch.len());
            if authorized_nodes.is_some() && (eager_bytes > 8 * 1024 * 1024 || journal.len() > 4096)
            {
                return Err(io::Error::other(
                    "scoped gap evidence budget exceeded; narrow the window",
                ));
            }
            let entry = entry.clone();
            match journal.last_mut() {
                Some(g) if g.group_sequence == e.group => g.entries.push(entry),
                _ => journal.push(Group {
                    group_sequence: e.group,
                    entries: vec![entry],
                }),
            }
        }
        Ok(Sources {
            segments,
            journal,
            oldest_group,
            tail: lazy,
            tail_paths,
            tail_evidence: Some((received, freshness)),
            bounds,
            filters,
            blocks,
        })
    }
}

#[cfg(test)]
#[path = "read_catalog_race_test.rs"]
mod race_tests;
