//! Q4 experimental physical descriptor reuse; never a retained-file lease.
use crate::tail::ManifestHandle;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

pub(crate) type Descriptor = (PathBuf, ManifestHandle);

pub(crate) struct Snapshot {
    pub labels: Vec<u64>,
    pub descriptors: Vec<Descriptor>,
    pub covered: Vec<(u64, u64)>,
    pub charged_bytes: usize,
}

#[derive(Default)]
pub(crate) struct Cache {
    current: Option<Arc<Snapshot>>,
    retired: Vec<Weak<Snapshot>>,
    pub hits: u64,
    pub builds: u64,
    pub fallbacks: u64,
}

impl Cache {
    const HOLDERS: usize = 4;
    const BYTES: usize = 8 * 1024 * 1024;

    pub fn stats(&self) -> (u64, u64, u64, usize, usize) {
        let holders = self
            .current
            .as_ref()
            .map_or(0, |s| Arc::strong_count(s) - 1)
            + self.retired.iter().map(Weak::strong_count).sum::<usize>();
        let bytes = self
            .current
            .as_ref()
            .map_or(0, |s| s.charged_bytes)
            .saturating_add(
                self.retired
                    .iter()
                    .filter_map(Weak::upgrade)
                    .map(|s| s.charged_bytes)
                    .fold(0usize, usize::saturating_add),
            );
        (self.hits, self.builds, self.fallbacks, holders, bytes)
    }

    pub fn invalidate(&mut self, labels: &[u64]) {
        self.retired.retain(|s| s.strong_count() > 0);
        if self.current.as_ref().is_some_and(|s| s.labels != labels) {
            let old = self.current.take().unwrap();
            if Arc::strong_count(&old) > 1 {
                self.retired.push(Arc::downgrade(&old));
            }
        }
    }

    pub fn lookup(&mut self, labels: &[u64]) -> Option<Arc<Snapshot>> {
        self.invalidate(labels);
        let current = self.current.as_ref()?;
        let held: usize = self.retired.iter().map(Weak::strong_count).sum();
        if Arc::strong_count(current) - 1 + held >= Self::HOLDERS {
            self.fallbacks += 1;
            return None;
        }
        self.hits += 1;
        Some(current.clone())
    }

    pub fn publish(&mut self, mut snapshot: Snapshot) -> Option<Arc<Snapshot>> {
        self.retired.retain(|s| s.strong_count() > 0);
        let held: usize = self.retired.iter().map(Weak::strong_count).sum();
        let retired_bytes: usize = self
            .retired
            .iter()
            .filter_map(Weak::upgrade)
            .map(|s| s.charged_bytes)
            .fold(0usize, usize::saturating_add);
        // Hard structural caps are independent of the approximate byte charge.
        if snapshot.descriptors.capacity() > 256
            || snapshot.labels.capacity() > 256
            || snapshot.covered.capacity() > 256
            || snapshot.descriptors.iter().any(|(path, m)| {
                path.capacity() > 4096
                    || m.freshness.len() > 256
                    || m.files.len() > 256
                    || m.freshness.keys().any(|s| s.capacity() > 4096)
                    || m.files
                        .iter()
                        .any(|(k, v)| k.capacity() > 4096 || v.sha256.capacity() > 4096)
            })
        {
            self.fallbacks += 1;
            return None;
        }
        // Conservative target-stdlib charge: reserve 1 KiB per BTree entry,
        // plus all owned string capacities and descriptor-vector capacities.
        // This intentionally charges shared manifests again per generation.
        let mut charge = std::mem::size_of::<Snapshot>()
            + snapshot.labels.capacity() * std::mem::size_of::<u64>()
            + snapshot.covered.capacity() * std::mem::size_of::<(u64, u64)>()
            + snapshot.descriptors.capacity() * std::mem::size_of::<Descriptor>();
        for (path, manifest) in &snapshot.descriptors {
            charge += path.capacity() + std::mem::size_of_val(&**manifest);
            charge += manifest.freshness.len() * 1024 + manifest.files.len() * 1024;
            charge += manifest
                .freshness
                .keys()
                .map(String::capacity)
                .sum::<usize>();
            charge += manifest
                .files
                .iter()
                .map(|(k, v)| k.capacity() + v.sha256.capacity())
                .sum::<usize>();
        }
        snapshot.charged_bytes = charge;
        if held >= Self::HOLDERS
            || self.retired.len() >= Self::HOLDERS
            || charge.saturating_add(retired_bytes) > Self::BYTES
        {
            self.fallbacks += 1;
            return None;
        }
        // A lookup miss due to holder admission must not create another owner.
        if self.current.is_some() {
            self.fallbacks += 1;
            return None;
        }
        let snapshot = Arc::new(snapshot);
        self.current = Some(snapshot.clone());
        self.builds += 1;
        Some(snapshot)
    }
}

pub(crate) enum SegmentSources {
    Owned(Vec<Descriptor>),
    Reused {
        snapshot: Arc<Snapshot>,
        selected: Vec<usize>,
    },
}

impl SegmentSources {
    pub fn reused(snapshot: Arc<Snapshot>, newest: u64) -> Self {
        let selected = snapshot
            .descriptors
            .iter()
            .enumerate()
            .filter_map(|(i, (_, m))| (m.first_group <= newest).then_some(i))
            .collect();
        Self::Reused { snapshot, selected }
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Descriptor> {
        (0..self.len()).map(|i| &self[i])
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Owned(v) => v.len(),
            Self::Reused { selected, .. } => selected.len(),
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn retain(&mut self, keep: impl Fn(&Descriptor) -> bool) {
        match self {
            Self::Owned(v) => v.retain(keep),
            Self::Reused { snapshot, selected } => {
                selected.retain(|i| keep(&snapshot.descriptors[*i]))
            }
        }
    }
}

impl std::ops::Index<usize> for SegmentSources {
    type Output = Descriptor;
    fn index(&self, i: usize) -> &Self::Output {
        match self {
            Self::Owned(v) => &v[i],
            Self::Reused { snapshot, selected } => &snapshot.descriptors[selected[i]],
        }
    }
}

impl<'a> IntoIterator for &'a SegmentSources {
    type Item = &'a Descriptor;
    type IntoIter = SegmentIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        SegmentIter {
            sources: self,
            next: 0,
        }
    }
}

pub(crate) struct SegmentIter<'a> {
    sources: &'a SegmentSources,
    next: usize,
}
impl<'a> Iterator for SegmentIter<'a> {
    type Item = &'a Descriptor;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.sources.len() {
            return None;
        }
        let result = &self.sources[self.next];
        self.next += 1;
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::Manifest;
    use std::collections::BTreeMap;

    fn snapshot(count: usize, large: bool) -> Snapshot {
        let manifest = Arc::new(Manifest {
            version: 1,
            journal_label: 1,
            first_group: 1,
            last_group: 1,
            records: 1,
            received_min_ns: 1,
            received_max_ns: 1,
            freshness: if large {
                (0..128)
                    .map(|i| (format!("{i:04}{}", "x".repeat(4000)), 1))
                    .collect()
            } else {
                BTreeMap::new()
            },
            files: BTreeMap::new(),
        });
        Snapshot {
            labels: (0..count as u64).collect(),
            covered: vec![(1, 1)],
            descriptors: (0..count)
                .map(|_| {
                    (
                        PathBuf::from("segment"),
                        ManifestHandle::Shared(manifest.clone()),
                    )
                })
                .collect(),
            charged_bytes: 0,
        }
    }

    #[test]
    fn coupled_structural_byte_caps_stale_generation_and_release() {
        let mut cache = Cache::default();
        assert!(cache.publish(snapshot(257, false)).is_none());
        assert!(cache.publish(snapshot(64, true)).is_none());
        let reader = cache.publish(snapshot(1, false)).unwrap();
        let weak = Arc::downgrade(&reader);
        assert!(
            cache.lookup(&[999]).is_none(),
            "stale label set must not authorize reuse"
        );
        assert!(
            weak.upgrade().is_some(),
            "held reader must survive invalidation"
        );
        drop(reader);
        assert!(
            weak.upgrade().is_none(),
            "weak retirement list must not retain metadata"
        );
    }
}
