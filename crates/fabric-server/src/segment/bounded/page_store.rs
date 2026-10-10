//! Scratch-only storage of already encoded Parquet blobs; encoding stays upstream.
use axum::body::Bytes;
use parquet::arrow::arrow_writer::{PageKey, PageStore, PageStoreArgs, PageStoreFactory};
use parquet::errors::{ParquetError, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

pub(super) const fn enabled(setting: Option<&str>) -> bool {
    match setting {
        None => true,
        Some(value) => match value.as_bytes() {
            b"1" => true,
            _ => panic!("FABRIC_PAGE_STORE_EXPERIMENT must be unset or 1"),
        },
    }
}

#[derive(Debug, Default)]
struct Observations {
    bytes: AtomicU64,
    live: AtomicU64,
    peak: AtomicU64,
    largest: AtomicU64,
    puts: AtomicU64,
    takes: AtomicU64,
    cleanup_error: AtomicBool,
}

#[derive(Debug)]
pub(super) struct Factory {
    dir: PathBuf,
    next: AtomicU64,
    observations: Arc<Observations>,
}

impl Factory {
    pub(super) fn new(path: &Path) -> io::Result<Self> {
        let dir = path.with_extension("pages");
        fs::create_dir(&dir)?;
        Ok(Self {
            dir,
            next: AtomicU64::new(0),
            observations: Arc::default(),
        })
    }

    fn store(&self, column: usize) -> io::Result<DiskStore> {
        let id = self.next.fetch_add(1, Relaxed);
        let path = self.dir.join(format!("{id}-{column}.pages"));
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        Ok(DiskStore {
            file: Some(file),
            path,
            end: 0,
            locations: Vec::new(),
            remaining: 0,
            stored: 0,
            observations: self.observations.clone(),
        })
    }

    /// Called after ArrowWriter finishes, before the manifest can be published.
    pub(super) fn finish(&self) -> io::Result<()> {
        if self.observations.cleanup_error.load(Relaxed) {
            return Err(io::Error::other("Parquet page scratch cleanup failed"));
        }
        fs::remove_dir(&self.dir)?; // A remaining blob/file makes success impossible.
        #[cfg(feature = "responsibility-alloc-probe")]
        {
            let o = &self.observations;
            eprintln!(
                "fabric-page-store: {}",
                serde_json::json!({
                    "table":self.dir.file_stem(), "completed_blob_bytes":o.bytes.load(Relaxed),
                    "peak_outstanding_blob_bytes":o.peak.load(Relaxed),
                    "largest_blob_bytes":o.largest.load(Relaxed),
                    "puts":o.puts.load(Relaxed),"takes":o.takes.load(Relaxed),
                    "outstanding_blob_bytes":o.live.load(Relaxed),"cleanup":true
                })
            );
        }
        Ok(())
    }
}

impl PageStoreFactory for Factory {
    fn create(&self, args: &PageStoreArgs<'_>) -> Result<Box<dyn PageStore>> {
        Ok(Box::new(self.store(args.column_index())?))
    }
}

struct DiskStore {
    file: Option<File>,
    path: PathBuf,
    end: u64,
    // One descriptor per blob of this fixed-size row group's column chunk.
    // The store is dropped at group splice, so descriptors do not accumulate
    // across segment groups. No payload is retained in this vector.
    locations: Vec<Option<(u64, usize)>>,
    remaining: usize,
    stored: u64,
    observations: Arc<Observations>,
}

impl DiskStore {
    fn remove(&mut self) -> io::Result<()> {
        drop(self.file.take());
        fs::remove_file(&self.path)
    }
}

impl PageStore for DiskStore {
    fn memory_size(&self) -> usize {
        self.locations.capacity() * std::mem::size_of::<Option<(u64, usize)>>()
    }

    fn put(&mut self, value: Bytes) -> Result<PageKey> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| ParquetError::General("closed page store".into()))?;
        file.seek(SeekFrom::Start(self.end))?;
        file.write_all(&value)?;
        let key = PageKey::new(self.locations.len() as u64);
        self.locations.push(Some((self.end, value.len())));
        self.end = self
            .end
            .checked_add(value.len() as u64)
            .ok_or_else(|| ParquetError::General("page offset overflow".into()))?;
        self.remaining += 1;
        self.stored += value.len() as u64;
        let o = &self.observations;
        o.bytes.fetch_add(value.len() as u64, Relaxed);
        let live = o.live.fetch_add(value.len() as u64, Relaxed) + value.len() as u64;
        o.peak.fetch_max(live, Relaxed);
        o.largest.fetch_max(value.len() as u64, Relaxed);
        o.puts.fetch_add(1, Relaxed);
        Ok(key)
    }

    fn take(&mut self, key: PageKey) -> Result<Bytes> {
        let location = usize::try_from(key.get())
            .ok()
            .and_then(|index| self.locations.get_mut(index))
            .ok_or_else(|| ParquetError::General("unknown page key".into()))?;
        let (offset, len) = location
            .take()
            .ok_or_else(|| ParquetError::General("page key already consumed".into()))?;
        let mut bytes = vec![0; len];
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| ParquetError::General("closed page store".into()))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut bytes)?;
        self.remaining -= 1;
        self.stored -= len as u64;
        self.observations.live.fetch_sub(len as u64, Relaxed);
        self.observations.takes.fetch_add(1, Relaxed);
        if self.remaining == 0 {
            // Surface deletion failure through ArrowWriter, not only a Drop log.
            self.remove()?;
        }
        Ok(Bytes::from(bytes))
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        self.observations.live.fetch_sub(self.stored, Relaxed);
        if (self.file.is_some() || self.path.exists())
            && let Err(error) = self.remove()
        {
            self.observations.cleanup_error.store(true, Relaxed);
            eprintln!(
                "fabric-server: page scratch cleanup {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{RecordBatch, StringArray};
    use parquet::arrow::arrow_writer::{ArrowWriter, ArrowWriterOptions};
    use parquet::file::properties::WriterProperties;
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let root = PathBuf::from(
                std::env::var_os("FABRIC_SCRATCH_ROOT").expect("disk scratch required"),
            );
            let path = root.join(format!(
                "page-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn factory(&self) -> Factory {
            Factory::new(&self.0.join("logs.parquet")).unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn page_store_selector_defaults_on_and_rejects_invalid_values() {
        assert!(enabled(None));
        assert!(enabled(Some("1")));
        for setting in ["", "0", "true", "01", " 1"] {
            assert!(std::panic::catch_unwind(|| enabled(Some(setting))).is_err());
        }
    }

    #[test]
    fn page_store_round_trip_reorders_and_rejects_stale_keys() {
        let scratch = Scratch::new();
        let factory = scratch.factory();
        let mut store = factory.store(0).unwrap();
        let a = store.put(Bytes::from_static(b"data\0page")).unwrap();
        let b = store.put(Bytes::from_static(b"dictionary")).unwrap();
        assert_eq!(store.take(b).unwrap(), b"dictionary"[..]);
        assert!(store.take(b).is_err());
        assert!(store.take(PageKey::new(99)).is_err());
        assert_eq!(store.take(a).unwrap(), b"data\0page"[..]);
        assert!(!store.path.exists());
        drop(store);
        assert_eq!(factory.observations.live.load(Relaxed), 0);
        factory.finish().unwrap();
    }

    #[test]
    fn page_store_real_file_write_read_and_cleanup_errors_are_rejected() {
        for fault in ["write", "read", "cleanup"] {
            let scratch = Scratch::new();
            let factory = scratch.factory();
            let mut store = factory.store(0).unwrap();
            if fault == "write" {
                store.file = Some(File::open(&store.path).unwrap());
                assert!(store.put(Bytes::from_static(b"must not vanish")).is_err());
            } else {
                let key = store.put(Bytes::from_static(b"required bytes")).unwrap();
                if fault == "read" {
                    store.file.as_ref().unwrap().set_len(0).unwrap();
                } else {
                    fs::remove_file(&store.path).unwrap();
                    fs::create_dir(&store.path).unwrap();
                }
                assert!(store.take(key).is_err());
            }
            drop(store);
            if fault == "cleanup" {
                assert!(factory.finish().is_err());
            } else {
                factory.finish().unwrap();
            }
        }
    }

    #[test]
    fn page_store_complete_file_bytes_match_independent_memory_writer() {
        let scratch = Scratch::new();
        let factory = Arc::new(scratch.factory());
        let strings: Vec<String> = (0..2300)
            .map(|i| {
                if i % 2 == 0 {
                    "tie".into()
                } else {
                    format!("{i:08}-{}", "abc0123456789".repeat(100))
                }
            })
            .collect();
        let batch = RecordBatch::try_from_iter([(
            "text",
            Arc::new(StringArray::from_iter_values(strings.iter())) as arrow_array::ArrayRef,
        )])
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_row_count(Some(1024))
            .set_dictionary_page_size_limit(1024)
            .set_data_page_size_limit(4096)
            .build();
        let write = |disk| {
            let mut options = ArrowWriterOptions::new().with_properties(properties.clone());
            if disk {
                options = options.with_page_store_factory(factory.clone());
            }
            let mut writer =
                ArrowWriter::try_new_with_options(Vec::new(), batch.schema(), options).unwrap();
            for start in (0..batch.num_rows()).step_by(127) {
                writer
                    .write(&batch.slice(start, (batch.num_rows() - start).min(127)))
                    .unwrap();
            }
            writer.into_inner().unwrap()
        };
        assert_eq!(write(false), write(true));
        assert!(factory.observations.bytes.load(Relaxed) > 0);
        assert_eq!(
            factory.observations.puts.load(Relaxed),
            factory.observations.takes.load(Relaxed)
        );
        factory.finish().unwrap();
        assert!(!factory.dir.exists());
    }
}
