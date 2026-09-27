//! Version-one Fabric batch spool. The payload contains actual OTLP protobuf messages.
//!
//! Two sidecars separate an interrupted append from a known failure. `append-in-progress`
//! is synced before any frame byte is written and removed after the commit; if a process
//! dies while it exists, reopen keeps only frames with a valid commit marker and truncates
//! the rest, exactly as for an unmarked tail. Because the data sync completes before the
//! commit marker is written, a persisted marker implies persisted data. `recovery-required`
//! is written only after a write or sync call reports an error. Readable bytes cannot clear
//! that state, so reopen refuses until the spool is rebuilt from a retained source.

use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use prost::Message;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const DATA_MAGIC: &[u8; 4] = b"FAB1";
const COMMIT_MAGIC: &[u8; 4] = b"FAC1";
const MAX_BATCH: usize = 1024 * 1024;
// One coverage-unknown notice, one host failure, and eight bounded gaps from
// each of sixteen log sources.
pub(crate) const MAX_GAPS_PER_BATCH: usize = 2 + 16 * 8;
pub(crate) const MAX_GAP_BYTES: usize = 256;
const HEADER_BYTES: u64 = 16;
const FRAME_OVERHEAD: u64 = HEADER_BYTES + 16;
pub const DEFAULT_SPOOL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, PartialEq, Message)]
pub struct Cursor {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(uint64, tag = "2")]
    pub device: u64,
    #[prost(uint64, tag = "3")]
    pub inode: u64,
    #[prost(uint64, tag = "4")]
    pub offset: u64,
    #[prost(bool, tag = "5")]
    pub skipping_oversize: bool,
    #[prost(uint32, tag = "6")]
    pub prefix_len: u32,
    #[prost(uint32, tag = "7")]
    pub prefix_crc: u32,
}

/// Versioned Fabric envelope. `metrics` and `logs` are serialized OTLP export
/// requests, not Fabric's old Event format. A retry transmits the stored bytes.
#[derive(Clone, PartialEq, Message)]
pub struct Batch {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    #[prost(bytes, tag = "2")]
    pub node_id: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub generation: u64,
    #[prost(uint64, tag = "4")]
    pub sequence: u64,
    #[prost(bytes, tag = "5")]
    pub metrics: Vec<u8>,
    #[prost(bytes, tag = "6")]
    pub logs: Vec<u8>,
    #[prost(message, repeated, tag = "7")]
    pub cursors: Vec<Cursor>,
    #[prost(string, repeated, tag = "8")]
    pub collection_gaps: Vec<String>,
}

impl Batch {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.node_id.len() != 16
            || self.generation == 0
            || self.sequence == 0
        {
            return Err(invalid("invalid Fabric batch identity/version"));
        }
        if self.metrics.is_empty() && self.logs.is_empty() && self.collection_gaps.is_empty() {
            return Err(invalid("empty Fabric batch"));
        }
        if self.metrics.len() > MAX_BATCH
            || self.logs.len() > MAX_BATCH
            || self.metrics.len().saturating_add(self.logs.len()) > MAX_BATCH
            || self.cursors.len() > 16
            || self.cursors.iter().any(|cursor| {
                cursor.path.len() > 4096
                    || cursor.prefix_len > 64
                    || u64::from(cursor.prefix_len) > cursor.offset
            })
            || self.collection_gaps.len() > MAX_GAPS_PER_BATCH
            || self
                .collection_gaps
                .iter()
                .any(|gap| gap.len() > MAX_GAP_BYTES)
        {
            return Err(invalid("Fabric batch field exceeds local profile cap"));
        }
        if !self.metrics.is_empty() {
            ExportMetricsServiceRequest::decode(self.metrics.as_slice())
                .map_err(|e| invalid(e.to_string()))?;
        }
        if !self.logs.is_empty() {
            ExportLogsServiceRequest::decode(self.logs.as_slice())
                .map_err(|e| invalid(e.to_string()))?;
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyncStage {
    Data,
    Marker,
    DirectoryBeforeClear,
    AfterClear,
}

const IN_PROGRESS: &str = "append-in-progress";
const RECOVERY_REQUIRED: &str = "recovery-required";

pub struct Journal {
    dir: PathBuf,
    file: File,
    node_id: [u8; 16],
    generation: u64,
    end: u64,
    next_sequence: u64,
    max_bytes: u64,
    poisoned: bool,
}

pub struct Inspection {
    pub node_id: [u8; 16],
    pub generation: u64,
    pub next_sequence: u64,
    pub committed_bytes: u64,
    pub file_bytes: u64,
    pub recovery_required: bool,
    /// A dead writer left `append-in-progress`; counts are what reopen will keep.
    pub interrupted_append: bool,
}

impl Journal {
    /// Read a bounded, approximate snapshot without taking the writer lock or
    /// repairing a tail. A concurrent append may leave an incomplete suffix.
    pub fn inspect(
        dir: impl AsRef<Path>,
        max_bytes: u64,
        mut visit: impl FnMut(Batch) -> io::Result<()>,
    ) -> io::Result<Inspection> {
        let dir = dir.as_ref();
        if !dir.join("identity").is_file() {
            return Err(invalid("missing journal identity"));
        }
        let (node_id, generation) = read_identity(dir)?;
        let mut file = File::open(dir.join("batches.faj"))?;
        let len = file.metadata()?.len();
        if len > max_bytes {
            return Err(invalid("journal exceeds configured spool cap"));
        }
        // A reported failure is final for read-only inspection: readable bytes
        // are not a durability witness, so nothing is counted as committed.
        let known_failure = dir.join(RECOVERY_REQUIRED);
        if known_failure.exists() {
            return Ok(Inspection {
                node_id,
                generation,
                next_sequence: 1,
                committed_bytes: 0,
                file_bytes: len,
                recovery_required: true,
                interrupted_append: false,
            });
        }
        let in_progress = dir.join(IN_PROGRESS);
        let mut interrupted_append = false;
        if in_progress.exists() {
            // A live writer holds the exclusive lock; retry rather than report a
            // normal append as interrupted. Under our shared lock no writer can
            // open, so the scan below sees a stable file.
            match file.try_lock_shared() {
                Ok(()) => {}
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "journal append in progress; retry inspection",
                    ));
                }
                Err(error) => return Err(error.into()),
            }
            if !in_progress.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "journal append completed during inspection; retry",
                ));
            }
            if known_failure.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "journal failure recorded during inspection; retry",
                ));
            }
            interrupted_append = true;
        }
        let mut at = 0;
        let mut next_sequence = 1_u64;
        while at < len {
            let Some((batch, next)) = read_frame(&mut file, at, len)? else {
                break;
            };
            if batch.node_id != node_id
                || batch.generation != generation
                || batch.sequence != next_sequence
            {
                return Err(invalid("journal identity or sequence mismatch"));
            }
            visit(batch)?;
            at = next;
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or_else(|| invalid("sequence exhausted"))?;
        }
        if known_failure.exists() || (!interrupted_append && in_progress.exists()) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "journal append in progress or recovery required; retry inspection",
            ));
        }
        Ok(Inspection {
            node_id,
            generation,
            next_sequence,
            committed_bytes: at,
            file_bytes: len,
            recovery_required: false,
            interrupted_append,
        })
    }

    pub fn open(dir: impl AsRef<Path>, max_bytes: u64) -> io::Result<Self> {
        if max_bytes < FRAME_OVERHEAD + 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "spool byte cap too small",
            ));
        }
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let dir = dir.canonicalize()?;
        let identity_exists = dir.join("identity").exists();
        let journal_len = match fs::metadata(dir.join("batches.faj")) {
            Ok(metadata) => Some(metadata.len()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        // First open creates the journal file, then publishes the identity by
        // rename. A process killed in between leaves an empty journal and no
        // identity, which is still a fresh spool. Any other mismatch is loss.
        let fresh_interrupted = !identity_exists && journal_len == Some(0);
        if identity_exists != journal_len.is_some() && !fresh_interrupted {
            return Err(io::Error::other(
                "recovery required: identity or journal missing; rebuild from retained source",
            ));
        }
        if dir.join(RECOVERY_REQUIRED).exists() {
            return Err(io::Error::other(
                "recovery required: a journal write or sync reported an error; rebuild from retained source",
            ));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("batches.faj"))?;
        file.try_lock()?;
        File::open(&dir)?.sync_all()?;
        let (node_id, generation) = read_or_create_identity(&dir)?;
        let mut end = 0;
        let mut next_sequence = 1_u64;
        let len = file.metadata()?.len();
        if len > max_bytes {
            return Err(invalid("journal exceeds configured spool cap"));
        }
        while end < len {
            match read_frame(&mut file, end, len)? {
                Some((batch, next)) => {
                    if batch.node_id != node_id
                        || batch.generation != generation
                        || batch.sequence != next_sequence
                    {
                        return Err(invalid("journal identity or sequence mismatch"));
                    }
                    end = next;
                    next_sequence = next_sequence
                        .checked_add(1)
                        .ok_or_else(|| invalid("sequence exhausted"))?;
                }
                None => {
                    file.set_len(end)?;
                    file.sync_all()?;
                    break;
                }
            }
        }
        file.sync_all()?;
        // The tail has been verified and synced; an interrupted append is resolved.
        match fs::remove_file(dir.join(IN_PROGRESS)) {
            Ok(()) => File::open(&dir)?.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(Self {
            dir,
            file,
            node_id,
            generation,
            end,
            next_sequence,
            max_bytes,
            poisoned: false,
        })
    }

    pub fn identity(&self) -> ([u8; 16], u64) {
        (self.node_id, self.generation)
    }
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
    pub fn used_bytes(&self) -> u64 {
        self.end
    }

    pub fn append(&mut self, batch: &Batch) -> io::Result<Batch> {
        self.append_inner(batch.clone(), |_| Ok(()))
    }

    // The private callback lets unit tests simulate a reported sync failure
    // while the bytes can still be read. It is not a runtime configuration.
    fn append_inner(
        &mut self,
        mut batch: Batch,
        mut after_sync: impl FnMut(SyncStage) -> io::Result<()>,
    ) -> io::Result<Batch> {
        if self.poisoned {
            return Err(io::Error::other("journal quarantined after write error"));
        }
        batch.version = 1;
        batch.node_id = self.node_id.to_vec();
        batch.generation = self.generation;
        batch.sequence = self.next_sequence;
        batch.validate()?;
        let bytes = batch.encode_to_vec();
        if bytes.len() > MAX_BATCH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "batch exceeds 1 MiB",
            ));
        }
        let data_end = self
            .end
            .checked_add(HEADER_BYTES + bytes.len() as u64)
            .ok_or_else(|| invalid("offset overflow"))?;
        let commit_end = data_end
            .checked_add(16)
            .ok_or_else(|| invalid("offset overflow"))?;
        if commit_end > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "spool full: collection must stop; coverage unknown until space is reclaimed",
            ));
        }
        // The in-progress sidecar is synced before any frame byte. A process
        // that dies while it exists leaves a tail that reopen verifies.
        let in_progress = self.dir.join(IN_PROGRESS);
        let result = (|| {
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&in_progress)?;
            marker.write_all(b"append in progress; reopen verifies the tail\n")?;
            marker.sync_all()?;
            File::open(&self.dir)?.sync_all()?;
            self.file.seek(SeekFrom::Start(self.end))?;
            let mut header = [0_u8; HEADER_BYTES as usize];
            header[..4].copy_from_slice(DATA_MAGIC);
            header[4..8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            let header_crc = crc32fast::hash(&header[..8]);
            header[8..12].copy_from_slice(&header_crc.to_le_bytes());
            header[12..16].copy_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
            self.file.write_all(&header)?;
            self.file.write_all(&bytes)?;
            self.file.sync_all()?;
            after_sync(SyncStage::Data)?;
            self.file.write_all(COMMIT_MAGIC)?;
            self.file.write_all(&data_end.to_le_bytes())?;
            let mut crc_input = [0_u8; 12];
            crc_input[..4].copy_from_slice(COMMIT_MAGIC);
            crc_input[4..].copy_from_slice(&data_end.to_le_bytes());
            self.file
                .write_all(&crc32fast::hash(&crc_input).to_le_bytes())?;
            self.file.sync_all()?;
            after_sync(SyncStage::Marker)?;
            File::open(&self.dir)?.sync_all()?;
            after_sync(SyncStage::DirectoryBeforeClear)?;
            fs::remove_file(&in_progress)?;
            after_sync(SyncStage::AfterClear)?;
            File::open(&self.dir)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = result {
            // Every error above came from a write, sync or sidecar call on the
            // spool, so its durability is unknown. Record that before reporting.
            self.poisoned = true;
            return Err(match record_failure(&self.dir) {
                Ok(()) => error,
                Err(record) => io::Error::other(format!(
                    "{error}; recording {RECOVERY_REQUIRED} also failed: {record}"
                )),
            });
        }
        self.end = commit_end;
        self.next_sequence += 1;
        Ok(batch)
    }

    pub fn replay(&mut self, mut visit: impl FnMut(Batch) -> io::Result<()>) -> io::Result<usize> {
        if self.poisoned {
            return Err(io::Error::other("journal quarantined"));
        }
        let mut at = 0;
        let mut count = 0;
        while at < self.end {
            let (batch, next) = read_frame(&mut self.file, at, self.end)?
                .ok_or_else(|| invalid("committed frame changed"))?;
            visit(batch)?;
            count += 1;
            at = next;
        }
        Ok(count)
    }
}

/// Persist a known write or sync failure. If this also fails, only the
/// caller's error report carries the knowledge; see the storage view.
fn record_failure(dir: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.join(RECOVERY_REQUIRED))?;
    file.write_all(b"journal write or sync reported an error; rebuild from retained source\n")?;
    file.sync_all()?;
    File::open(dir)?.sync_all()
}

fn read_or_create_identity(dir: &Path) -> io::Result<([u8; 16], u64)> {
    let path = dir.join("identity");
    if !path.exists() {
        // Write, sync and rename so a killed process never leaves a torn identity.
        let mut id = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut id)?;
        let staged = dir.join("identity.tmp");
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&staged)?;
        out.write_all(b"FAI1")?;
        out.write_all(&id)?;
        out.write_all(&1_u64.to_le_bytes())?;
        out.sync_all()?;
        fs::rename(&staged, &path)?;
        File::open(dir)?.sync_all()?;
    }
    read_identity(dir)
}

fn read_identity(dir: &Path) -> io::Result<([u8; 16], u64)> {
    let bytes = fs::read(dir.join("identity"))?;
    if bytes.len() != 28 || &bytes[..4] != b"FAI1" {
        return Err(invalid("invalid node identity"));
    }
    let mut id = [0_u8; 16];
    id.copy_from_slice(&bytes[4..20]);
    let generation = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    if generation == 0 {
        return Err(invalid("invalid stream generation"));
    }
    Ok((id, generation))
}

fn read_frame(file: &mut File, at: u64, len: u64) -> io::Result<Option<(Batch, u64)>> {
    if len - at < HEADER_BYTES {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(at))?;
    let mut header = [0_u8; HEADER_BYTES as usize];
    file.read_exact(&mut header)?;
    if &header[..4] != DATA_MAGIC {
        return Err(invalid(format!("bad alpha frame at {at}")));
    }
    if crc32fast::hash(&header[..8]) != u32::from_le_bytes(header[8..12].try_into().unwrap()) {
        return Err(invalid("alpha frame header checksum mismatch"));
    }
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if size == 0 || size > MAX_BATCH {
        return Err(invalid("invalid alpha frame length"));
    }
    let data_end = at + HEADER_BYTES + size as u64;
    if data_end > len {
        return Ok(None);
    }
    let mut payload = vec![0; size];
    file.read_exact(&mut payload)?;
    if crc32fast::hash(&payload) != u32::from_le_bytes(header[12..16].try_into().unwrap()) {
        return Err(invalid("alpha frame checksum mismatch"));
    }
    let batch = Batch::decode(payload.as_slice()).map_err(|e| invalid(e.to_string()))?;
    batch.validate()?;
    if data_end + 16 > len {
        return Ok(None);
    }
    let mut marker = [0_u8; 16];
    file.read_exact(&mut marker)?;
    if &marker[..4] != COMMIT_MAGIC
        || u64::from_le_bytes(marker[4..12].try_into().unwrap()) != data_end
        || crc32fast::hash(&marker[..12]) != u32::from_le_bytes(marker[12..16].try_into().unwrap())
    {
        return Err(invalid("alpha commit marker mismatch"));
    }
    Ok(Some((batch, data_end + 16)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("alpha-journal-fault-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn batch() -> Batch {
        Batch {
            version: 1,
            node_id: vec![0; 16],
            generation: 1,
            sequence: 1,
            metrics: vec![],
            logs: vec![],
            cursors: vec![],
            collection_gaps: vec!["test".into()],
        }
    }

    fn inspect_counts(dir: &Path) -> (Inspection, usize) {
        let mut visible = 0;
        let status = Journal::inspect(dir, 64 * 1024, |_| {
            visible += 1;
            Ok(())
        })
        .unwrap();
        (status, visible)
    }

    #[test]
    fn reported_sync_errors_quarantine_and_record_known_failure() {
        for stage in [
            SyncStage::Data,
            SyncStage::Marker,
            SyncStage::DirectoryBeforeClear,
            SyncStage::AfterClear,
        ] {
            let scratch = Scratch::new();
            let mut journal = Journal::open(&scratch.0, 64 * 1024).unwrap();
            journal.append(&batch()).unwrap();
            let prior = journal.next_sequence();
            let failed = journal.append_inner(batch(), |at| {
                if at == stage {
                    Err(io::Error::other("injected reported sync failure"))
                } else {
                    Ok(())
                }
            });
            assert!(failed.is_err());
            assert_eq!(journal.next_sequence(), prior);
            assert!(journal.append(&batch()).is_err());
            assert!(journal.replay(|_| Ok(())).is_err());
            assert!(scratch.0.join(RECOVERY_REQUIRED).exists());
            // The known failure is visible even while the quarantined writer lives.
            let (live, live_visible) = inspect_counts(&scratch.0);
            assert!(live.recovery_required);
            assert_eq!((live.committed_bytes, live_visible), (0, 0));
            drop(journal);
            let (inspected, visible) = inspect_counts(&scratch.0);
            assert!(inspected.recovery_required);
            assert_eq!((inspected.committed_bytes, visible), (0, 0));
            assert!(Journal::open(&scratch.0, 64 * 1024).is_err());
        }
    }

    #[test]
    fn process_death_at_each_append_stage_reopens_to_a_verified_prefix() {
        // A panic unwinds past the error branch, so no failure is recorded:
        // the on-disk state is what a killed process leaves behind.
        for (stage, kept) in [
            (SyncStage::Data, false),
            (SyncStage::Marker, true),
            (SyncStage::DirectoryBeforeClear, true),
            (SyncStage::AfterClear, true),
        ] {
            let scratch = Scratch::new();
            let mut journal = Journal::open(&scratch.0, 64 * 1024).unwrap();
            let first = journal.append(&batch()).unwrap();
            let first_end = journal.used_bytes();
            let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                journal
                    .append_inner(batch(), |at| {
                        if at == stage {
                            panic!("simulated process death");
                        }
                        Ok(())
                    })
                    .unwrap();
            }));
            assert!(died.is_err());
            drop(journal);
            assert!(!scratch.0.join(RECOVERY_REQUIRED).exists());
            let in_progress = scratch.0.join(IN_PROGRESS).exists();
            assert_eq!(in_progress, stage != SyncStage::AfterClear);
            let (status, visible) = inspect_counts(&scratch.0);
            assert!(!status.recovery_required);
            assert_eq!(status.interrupted_append, in_progress);
            assert_eq!(visible, if kept { 2 } else { 1 });
            let mut reopened = Journal::open(&scratch.0, 64 * 1024).unwrap();
            assert!(!scratch.0.join(IN_PROGRESS).exists());
            let mut replayed = Vec::new();
            reopened
                .replay(|b| {
                    replayed.push(b);
                    Ok(())
                })
                .unwrap();
            assert_eq!(replayed[0], first);
            assert_eq!(replayed.len(), if kept { 2 } else { 1 });
            if !kept {
                assert_eq!(reopened.used_bytes(), first_end);
            }
            let next = reopened.append(&batch()).unwrap();
            assert_eq!(next.sequence, replayed.len() as u64 + 1);
        }
    }

    #[test]
    fn first_open_killed_before_identity_publication_is_still_fresh() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("batches.faj"), b"").unwrap();
        fs::write(scratch.0.join("identity.tmp"), b"FAI1torn").unwrap();
        let mut journal = Journal::open(&scratch.0, 64 * 1024).unwrap();
        assert_eq!(journal.append(&batch()).unwrap().sequence, 1);
        drop(journal);
        // A non-empty journal without its identity is still refused.
        fs::remove_file(scratch.0.join("identity")).unwrap();
        assert!(Journal::open(&scratch.0, 64 * 1024).is_err());
    }

    #[test]
    fn live_writer_marker_is_retryable_then_completed_batch_is_visible() {
        let scratch = Scratch::new();
        let mut journal = Journal::open(&scratch.0, 64 * 1024).unwrap();
        let committed = journal
            .append_inner(batch(), |stage| {
                if stage == SyncStage::Data {
                    assert_eq!(
                        Journal::inspect(&scratch.0, 64 * 1024, |_| Ok(()))
                            .err()
                            .unwrap()
                            .kind(),
                        io::ErrorKind::WouldBlock
                    );
                }
                Ok(())
            })
            .unwrap();
        let mut visible = Vec::new();
        let status = Journal::inspect(&scratch.0, 64 * 1024, |item| {
            visible.push(item);
            Ok(())
        })
        .unwrap();
        assert!(!status.recovery_required);
        assert_eq!(status.next_sequence, 2);
        assert_eq!(visible, vec![committed]);
    }

    #[test]
    fn plausible_interrupted_tail_truncates_only_uncommitted_bytes() {
        let scratch = Scratch::new();
        let mut journal = Journal::open(&scratch.0, 64 * 1024).unwrap();
        let committed = journal.append(&batch()).unwrap();
        let end = journal.used_bytes();
        drop(journal);
        let mut file = OpenOptions::new()
            .append(true)
            .open(scratch.0.join("batches.faj"))
            .unwrap();
        file.write_all(&DATA_MAGIC[..2]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut reopened = Journal::open(&scratch.0, 64 * 1024).unwrap();
        assert_eq!(reopened.used_bytes(), end);
        assert_eq!(reopened.next_sequence(), 2);
        let mut batches = Vec::new();
        assert_eq!(
            reopened
                .replay(|b| {
                    batches.push(b);
                    Ok(())
                })
                .unwrap(),
            1
        );
        assert_eq!(batches, vec![committed]);
    }
}
