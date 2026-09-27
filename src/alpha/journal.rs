//! Version-one Fabric batch spool on the shared [frame log](super::frame).
//!
//! Each frame holds one encoded `Batch`; its sequence equals its position in
//! the stream. The spool keeps a durable ACK cursor. Batches at or below it
//! have been acknowledged by the server after its own durable commit, so whole
//! sealed files wholly at or below it can be deleted. The node sends the batch
//! after the cursor as the exact stored bytes. Crash and failure semantics are
//! those of the frame log ([ADR-0011](../../docs/decisions/ADR-0011-separate-interrupted-append-from-known-failure.md));
//! delivery follows [ADR-0013](../../docs/decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md).

use crate::alpha::frame::{FileRef, FrameLog, FramePos, SyncStage};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use prost::Message;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const MAX_BATCH: usize = 1024 * 1024;
// One coverage-unknown notice, one host failure, and eight bounded gaps from
// each of sixteen log sources.
pub(crate) const MAX_GAPS_PER_BATCH: usize = 2 + 16 * 8;
pub(crate) const MAX_GAP_BYTES: usize = 256;
pub const DEFAULT_SPOOL_BYTES: u64 = 256 * 1024 * 1024;
/// Seal the active file once it reaches this size, at the next metrics batch.
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
const ACKED: &str = "acked";

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

pub struct Journal {
    dir: PathBuf,
    log: FrameLog,
    node_id: [u8; 16],
    generation: u64,
    next_sequence: u64,
    acked: u64,
    /// Position of batch `acked + 1`, or the log end when none is pending.
    send_pos: FramePos,
    /// Sequence of the first batch in the active file (or the next one).
    active_first: u64,
    rotate_bytes: u64,
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
    /// Batches at or below this sequence have been acknowledged.
    pub acked_through: u64,
}

/// Checks identity and sequence continuity while a spool is scanned, and
/// records where the first unacknowledged batch and the active file start.
struct Scan {
    node_id: [u8; 16],
    generation: u64,
    acked: u64,
    first: Option<u64>,
    next: u64,
    send_pos: Option<FramePos>,
    active_first: Option<u64>,
    last_file: Option<FileRef>,
}

impl Scan {
    fn new(node_id: [u8; 16], generation: u64, acked: u64) -> Self {
        Self {
            node_id,
            generation,
            acked,
            first: None,
            next: 0,
            send_pos: None,
            active_first: None,
            last_file: None,
        }
    }

    fn visit(&mut self, payload: &[u8], pos: FramePos) -> io::Result<Batch> {
        let batch = Batch::decode(payload).map_err(|e| invalid(e.to_string()))?;
        batch.validate()?;
        if batch.node_id != self.node_id || batch.generation != self.generation {
            return Err(invalid("journal identity mismatch"));
        }
        match self.first {
            None => {
                self.first = Some(batch.sequence);
            }
            Some(_) if batch.sequence != self.next => {
                return Err(invalid("journal sequence mismatch"));
            }
            Some(_) => {}
        }
        // A sealed file's label is the sequence of its first batch.
        if self.last_file != Some(pos.file) {
            match pos.file {
                FileRef::Sealed(label) if label != batch.sequence || pos.offset != 0 => {
                    return Err(invalid("sealed log label does not match its first batch"));
                }
                FileRef::Active => self.active_first = Some(batch.sequence),
                FileRef::Sealed(_) => {}
            }
            self.last_file = Some(pos.file);
        }
        if batch.sequence == self.acked + 1 {
            self.send_pos = Some(pos);
        }
        self.next = batch
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("sequence exhausted"))?;
        Ok(batch)
    }

    /// Next sequence after the scan, checking the ACK cursor against it.
    fn finish(&self) -> io::Result<u64> {
        match self.first {
            None => Ok(self.acked + 1),
            Some(first) => {
                if first > self.acked + 1 {
                    return Err(invalid(
                        "spool lost unacknowledged batches; rebuild from retained source",
                    ));
                }
                if self.acked >= self.next {
                    return Err(invalid("ACK cursor is beyond the committed spool"));
                }
                Ok(self.next)
            }
        }
    }
}

fn read_acked(dir: &Path, generation: u64) -> io::Result<u64> {
    let bytes = match fs::read(dir.join(ACKED)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if bytes.len() != 20 || &bytes[..4] != b"FAK1" {
        return Err(invalid("invalid ACK cursor"));
    }
    if u64::from_le_bytes(bytes[4..12].try_into().unwrap()) != generation {
        return Err(invalid("ACK cursor belongs to another generation"));
    }
    Ok(u64::from_le_bytes(bytes[12..20].try_into().unwrap()))
}

fn write_acked(dir: &Path, generation: u64, through: u64) -> io::Result<()> {
    let staged = dir.join("acked.tmp");
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staged)?;
    out.write_all(b"FAK1")?;
    out.write_all(&generation.to_le_bytes())?;
    out.write_all(&through.to_le_bytes())?;
    out.sync_all()?;
    fs::rename(&staged, dir.join(ACKED))?;
    File::open(dir)?.sync_all()
}

impl Journal {
    /// Read a bounded snapshot without taking the writer lock or repairing a
    /// tail. A concurrent append, rotation or reclaim makes it retryable.
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
        let acked = read_acked(dir, generation)?;
        let mut scan = Scan::new(node_id, generation, acked);
        let snapshot = FrameLog::inspect(dir, max_bytes, MAX_BATCH, |payload, pos| {
            visit(scan.visit(payload, pos)?)
        })?;
        let next_sequence = if snapshot.recovery_required {
            1
        } else {
            scan.finish()?
        };
        Ok(Inspection {
            node_id,
            generation,
            next_sequence,
            committed_bytes: snapshot.committed_bytes,
            file_bytes: snapshot.file_bytes,
            recovery_required: snapshot.recovery_required,
            interrupted_append: snapshot.interrupted_append,
            acked_through: acked,
        })
    }

    pub fn open(dir: impl AsRef<Path>, max_bytes: u64) -> io::Result<Self> {
        Self::open_rotating(dir, max_bytes, ROTATE_BYTES)
    }

    fn open_rotating(dir: impl AsRef<Path>, max_bytes: u64, rotate_bytes: u64) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let dir = dir.canonicalize()?;
        let identity_exists = dir.join("identity").exists();
        let (active_len, sealed) = FrameLog::present(&dir)?;
        // First open creates the journal file, then publishes the identity by
        // rename. A process killed in between leaves an empty journal and no
        // identity, which is still a fresh spool. Any other mismatch is loss.
        let has_frames = active_len.unwrap_or(0) > 0 || sealed > 0;
        let log_exists = active_len.is_some() || sealed > 0;
        if (identity_exists && !log_exists) || (!identity_exists && has_frames) {
            return Err(io::Error::other(
                "recovery required: identity or journal missing; rebuild from retained source",
            ));
        }
        let identity = if identity_exists {
            Some(read_identity(&dir)?)
        } else {
            None
        };
        let (node_id, generation) = identity.unwrap_or(([0; 16], 1));
        let acked = if identity_exists {
            read_acked(&dir, generation)?
        } else {
            0
        };
        let mut scan = Scan::new(node_id, generation, acked);
        let log = FrameLog::open(&dir, max_bytes, MAX_BATCH, |payload, pos| {
            if identity.is_none() {
                return Err(invalid("journal frames without identity"));
            }
            scan.visit(payload, pos).map(|_| ())
        })?;
        let (node_id, generation) = match identity {
            Some(identity) => identity,
            None => read_or_create_identity(&dir)?,
        };
        let next_sequence = scan.finish()?;
        let send_pos = scan.send_pos.unwrap_or_else(|| log.end_pos());
        Ok(Self {
            dir,
            active_first: scan.active_first.unwrap_or(next_sequence),
            log,
            node_id,
            generation,
            next_sequence,
            acked,
            send_pos,
            rotate_bytes,
        })
    }

    pub fn identity(&self) -> ([u8; 16], u64) {
        (self.node_id, self.generation)
    }
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
    pub fn used_bytes(&self) -> u64 {
        self.log.used_bytes()
    }
    pub fn acked_through(&self) -> u64 {
        self.acked
    }

    pub fn append(&mut self, batch: &Batch) -> io::Result<Batch> {
        self.append_inner(batch.clone(), |_| Ok(()))
    }

    // The private callback lets unit tests simulate a reported sync failure
    // or a process death after each stage. It is not a runtime configuration.
    fn append_inner(
        &mut self,
        mut batch: Batch,
        after_sync: impl FnMut(SyncStage) -> io::Result<()>,
    ) -> io::Result<Batch> {
        if self.log.is_poisoned() {
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
        // Rotate only before a metrics batch, so every retained file begins
        // with one and replay after reclaim still sees counter start state.
        if !batch.metrics.is_empty() && self.log.active_bytes() >= self.rotate_bytes {
            let old_end = self.log.active_bytes();
            self.log.rotate(self.active_first)?;
            self.send_pos = FrameLog::remap(self.send_pos, self.active_first, old_end);
            self.active_first = batch.sequence;
        }
        let pos = self.log.append_with(&bytes, after_sync)?;
        if self.acked + 1 == batch.sequence {
            self.send_pos = pos;
        }
        if pos.offset == 0 {
            // First frame of an empty active file: after first open or rotation.
            self.active_first = batch.sequence;
        }
        self.next_sequence += 1;
        Ok(batch)
    }

    pub fn replay(&mut self, mut visit: impl FnMut(Batch) -> io::Result<()>) -> io::Result<usize> {
        let mut pos = self.log.start_pos();
        let mut count = 0;
        while let Some((payload, next)) = self.log.read_at(pos)? {
            visit(Batch::decode(payload.as_slice()).map_err(|e| invalid(e.to_string()))?)?;
            count += 1;
            pos = next;
        }
        Ok(count)
    }

    /// The oldest unacknowledged batch as its exact stored bytes.
    pub fn next_unacked(&self) -> io::Result<Option<(u64, Vec<u8>)>> {
        match self.log.read_at(self.send_pos)? {
            None => Ok(None),
            Some((payload, _)) => {
                let batch =
                    Batch::decode(payload.as_slice()).map_err(|e| invalid(e.to_string()))?;
                if batch.sequence != self.acked + 1 {
                    return Err(invalid("send cursor does not match ACK cursor"));
                }
                Ok(Some((batch.sequence, payload)))
            }
        }
    }

    /// Record a durable server acknowledgement through `through`, then delete
    /// sealed files that hold only acknowledged batches.
    pub fn record_ack(&mut self, through: u64) -> io::Result<()> {
        if through <= self.acked {
            return Ok(());
        }
        if through >= self.next_sequence {
            return Err(invalid(
                "server acknowledged a sequence this spool never committed",
            ));
        }
        write_acked(&self.dir, self.generation, through)?;
        let mut pos = self.send_pos;
        let mut seq = self.acked + 1;
        while seq <= through {
            let (_, next) = self
                .log
                .read_at(pos)?
                .ok_or_else(|| invalid("send cursor ran past committed spool"))?;
            pos = next;
            seq += 1;
        }
        self.acked = through;
        self.send_pos = self.log.normalize(pos);
        loop {
            let labels = self.log.sealed_labels();
            let Some(_) = labels.first() else {
                break;
            };
            let successor = labels.get(1).copied().unwrap_or(self.active_first);
            if successor > through + 1 {
                break;
            }
            self.log.remove_oldest_sealed()?;
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alpha::frame::{IN_PROGRESS, RECOVERY_REQUIRED};
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
        file.write_all(&b"FAB1"[..2]).unwrap();
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

    fn metrics_batch() -> Batch {
        use opentelemetry_proto::tonic::metrics::v1::ResourceMetrics;
        let mut b = batch();
        b.collection_gaps.clear();
        b.metrics = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics::default()],
        }
        .encode_to_vec();
        b
    }

    fn sealed_on_disk(dir: &Path) -> Vec<u64> {
        let mut labels: Vec<u64> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.strip_prefix("sealed-")?
                    .strip_suffix(".faj")?
                    .parse()
                    .ok()
            })
            .collect();
        labels.sort_unstable();
        labels
    }

    #[test]
    fn acknowledged_prefix_is_sent_in_order_then_reclaimed_by_whole_files() {
        let scratch = Scratch::new();
        let mut journal = Journal::open_rotating(&scratch.0, 1024 * 1024, 150).unwrap();
        let mut committed = Vec::new();
        for n in 0..10 {
            let next = if n % 3 == 2 { batch() } else { metrics_batch() };
            committed.push(journal.append(&next).unwrap());
        }
        let labels = sealed_on_disk(&scratch.0);
        assert!(labels.len() >= 2, "rotation expected, got {labels:?}");
        // The first unacknowledged batch is sent as its exact stored bytes.
        let (seq, bytes) = journal.next_unacked().unwrap().unwrap();
        assert_eq!((seq, bytes), (1, committed[0].encode_to_vec()));
        journal.record_ack(3).unwrap();
        assert_eq!(journal.next_unacked().unwrap().unwrap().0, 4);
        // Every retained sealed file still holds a batch above the cursor.
        let retained = sealed_on_disk(&scratch.0);
        let mut bounds = retained.clone();
        bounds.push(journal.active_first);
        for pair in bounds.windows(2) {
            assert!(
                pair[1] > 4,
                "file {} holds only acknowledged batches",
                pair[0]
            );
        }
        assert!(retained.len() < labels.len());
        drop(journal);

        let mut reopened = Journal::open_rotating(&scratch.0, 1024 * 1024, 150).unwrap();
        assert_eq!(reopened.acked_through(), 3);
        assert_eq!(reopened.next_sequence(), 11);
        let (seq, bytes) = reopened.next_unacked().unwrap().unwrap();
        assert_eq!((seq, bytes), (4, committed[3].encode_to_vec()));
        let mut replayed = Vec::new();
        reopened
            .replay(|b| {
                replayed.push(b.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(*replayed.last().unwrap(), 10);
        assert!(replayed[0] <= 4);
        assert!(reopened.record_ack(11).is_err());
        reopened.record_ack(10).unwrap();
        assert!(reopened.next_unacked().unwrap().is_none());
        assert_eq!(reopened.append(&batch()).unwrap().sequence, 11);
        assert_eq!(reopened.next_unacked().unwrap().unwrap().0, 11);
        drop(reopened);
        let status = Journal::inspect(&scratch.0, 1024 * 1024, |_| Ok(())).unwrap();
        assert_eq!((status.acked_through, status.next_sequence), (10, 12));
    }

    #[test]
    fn missing_unacknowledged_sealed_file_refuses_reopen() {
        let scratch = Scratch::new();
        let mut journal = Journal::open_rotating(&scratch.0, 1024 * 1024, 100).unwrap();
        for _ in 0..6 {
            journal.append(&metrics_batch()).unwrap();
        }
        drop(journal);
        let oldest = sealed_on_disk(&scratch.0)[0];
        fs::remove_file(scratch.0.join(format!("sealed-{oldest:020}.faj"))).unwrap();
        assert!(Journal::open_rotating(&scratch.0, 1024 * 1024, 100).is_err());
    }

    #[test]
    fn rotation_interrupted_after_rename_reopens_with_a_fresh_active_file() {
        let scratch = Scratch::new();
        let mut journal = Journal::open_rotating(&scratch.0, 1024 * 1024, 1 << 30).unwrap();
        journal.append(&metrics_batch()).unwrap();
        journal.append(&metrics_batch()).unwrap();
        drop(journal);
        // State left by a process killed between rename and create.
        fs::rename(
            scratch.0.join("batches.faj"),
            scratch.0.join(format!("sealed-{:020}.faj", 1)),
        )
        .unwrap();
        let mut reopened = Journal::open_rotating(&scratch.0, 1024 * 1024, 1 << 30).unwrap();
        assert_eq!(reopened.next_sequence(), 3);
        assert_eq!(reopened.append(&metrics_batch()).unwrap().sequence, 3);
        assert_eq!(reopened.replay(|_| Ok(())).unwrap(), 3);
        // A sealed label that does not match its first batch is corruption.
        drop(reopened);
        fs::rename(
            scratch.0.join(format!("sealed-{:020}.faj", 1)),
            scratch.0.join(format!("sealed-{:020}.faj", 2)),
        )
        .unwrap();
        assert!(Journal::open_rotating(&scratch.0, 1024 * 1024, 1 << 30).is_err());
    }
}
