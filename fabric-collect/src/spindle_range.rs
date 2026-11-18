use admission_kernel::telemetry::projection_frontier_advance;
use fabric_core::SpindleProvenance;
use fabric_store::CatalogStore;
use std::time::{Duration, Instant};

/// Consecutive staged Spindle IDs that came from the same authenticated origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpindleProjectionRange {
    origin: String,
    first: u64,
    last: u64,
}

impl SpindleProjectionRange {
    pub fn include(range: &mut Option<Self>, key: &SpindleProvenance) -> Result<(), String> {
        if let Some(range) = range {
            range.extend(&key.origin, key.record_id, key.record_id)?;
        } else {
            *range = Some(Self {
                origin: key.origin.clone(),
                first: key.record_id,
                last: key.record_id,
            });
        }
        Ok(())
    }

    pub(crate) fn append(range: &mut Option<Self>, next: Option<Self>) -> Result<(), String> {
        if let Some(next) = next {
            if let Some(range) = range {
                range.extend(&next.origin, next.first, next.last)?;
            } else {
                *range = Some(next);
            }
        }
        Ok(())
    }

    fn extend(&mut self, origin: &str, first: u64, last: u64) -> Result<(), String> {
        if self.origin != origin || self.last.checked_add(1) != Some(first) {
            return Err("Spindle projection range changes origin or skips a record".into());
        }
        self.last = last;
        Ok(())
    }

    #[must_use]
    pub fn last_id(&self) -> u64 {
        self.last
    }

    /// Persists the frontier after every Parquet publication in this range succeeded.
    /// The caller holds the projection writer lock.
    pub fn commit_published(&self, catalog: &CatalogStore) -> Result<u64, String> {
        let frontier = CatalogStore::read_spindle_projection_frontier(
            catalog.layout(),
            Instant::now() + Duration::from_millis(500),
        )?;
        let origin_matches = frontier.origin.as_deref() == Some(self.origin.as_str());
        if origin_matches && self.last <= frontier.record_id {
            // A prior catalog commit may have succeeded before its result was observed.
            return Ok(frontier.record_id);
        }
        let next = projection_frontier_advance(
            frontier.record_id,
            self.first,
            self.last,
            origin_matches,
            true,
        );
        if next != self.last || next == frontier.record_id {
            return Err("published Spindle range does not extend the bound frontier".into());
        }
        catalog.set_watermark("spindle_watermark_id", next)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(origin: &str, first: u64, last: u64) -> SpindleProjectionRange {
        SpindleProjectionRange {
            origin: origin.into(),
            first,
            last,
        }
    }

    #[test]
    fn published_range_requires_the_next_bound_origin_prefix() {
        let base = std::env::var_os("TMPDIR")
            .map(std::path::PathBuf::from)
            .filter(|path| !path.starts_with("/tmp") && !path.starts_with("/dev/shm"))
            .unwrap_or_else(|| fabric_core::DEFAULT_TMP_DIR.into());
        let dir = tempfile::tempdir_in(base).unwrap();
        let catalog = CatalogStore::open(dir.path()).unwrap();
        let _writer_lock = fabric_store::try_acquire_writer_lock(catalog.layout()).unwrap();

        assert!(range("origin-a", 1, 1).commit_published(&catalog).is_err());
        catalog.bind_spindle_projection("origin-a").unwrap();
        assert!(range("origin-b", 1, 1).commit_published(&catalog).is_err());
        assert!(range("origin-a", 2, 3).commit_published(&catalog).is_err());
        assert_eq!(
            catalog.read_watermark("spindle_watermark_id").unwrap(),
            None
        );

        assert_eq!(
            range("origin-a", 1, 2).commit_published(&catalog).unwrap(),
            2
        );
        assert_eq!(
            range("origin-a", 1, 2).commit_published(&catalog).unwrap(),
            2
        );
        assert!(range("origin-a", 2, 4).commit_published(&catalog).is_err());
        assert_eq!(
            range("origin-a", 3, 4).commit_published(&catalog).unwrap(),
            4
        );
    }
}
