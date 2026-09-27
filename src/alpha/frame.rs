//! Rotating framed log shared by the node spool and the server journal.
//!
//! A frame is a 16-byte header (magic, payload length, CRC32 of those 8 bytes,
//! payload CRC32), the payload, and a 16-byte commit marker (magic, data end,
//! CRC32). An append writes header and payload, syncs, writes the marker and
//! syncs again, so a persisted marker implies persisted data.
//!
//! Frames are appended to the active file `batches.faj`. The owner may seal it
//! by renaming it to `sealed-<label>.faj` and starting a new active file; the
//! label is chosen by the owner (the node uses the first batch sequence in the
//! file). Sealed files are immutable and scanned strictly: an incomplete frame
//! in one is corruption. Only the active file may carry an uncommitted tail.
//!
//! Two sidecars separate an interrupted append from a known failure. See
//! [ADR-0011](../../docs/decisions/ADR-0011-separate-interrupted-append-from-known-failure.md).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const DATA_MAGIC: &[u8; 4] = b"FAB1";
const COMMIT_MAGIC: &[u8; 4] = b"FAC1";
pub const HEADER_BYTES: u64 = 16;
pub const MARKER_BYTES: u64 = 16;
pub const FRAME_OVERHEAD: u64 = HEADER_BYTES + MARKER_BYTES;
pub const ACTIVE: &str = "batches.faj";
pub const IN_PROGRESS: &str = "append-in-progress";
pub const RECOVERY_REQUIRED: &str = "recovery-required";
const SEALED_PREFIX: &str = "sealed-";
const SUFFIX: &str = ".faj";

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn retry(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncStage {
    Data,
    Marker,
    DirectoryBeforeClear,
    AfterClear,
}

/// Which file a frame lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRef {
    Sealed(u64),
    Active,
}

/// The start of a frame, or the end of the log when it equals `end_pos`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramePos {
    pub file: FileRef,
    pub offset: u64,
}

struct Sealed {
    label: u64,
    bytes: u64,
}

pub struct FrameLog {
    dir: PathBuf,
    active: File,
    active_end: u64,
    sealed: Vec<Sealed>,
    max_bytes: u64,
    max_payload: usize,
    poisoned: bool,
}

/// Read-only view of a log directory.
pub struct Snapshot {
    pub committed_bytes: u64,
    pub file_bytes: u64,
    pub recovery_required: bool,
    pub interrupted_append: bool,
}

// Unlock explicitly: a descriptor duplicated into a forked child of another
// thread would otherwise keep the lock alive until that child execs.
impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = self.active.unlock();
    }
}

struct SharedLock<'a>(&'a File);

impl Drop for SharedLock<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn sealed_name(label: u64) -> String {
    format!("{SEALED_PREFIX}{label:020}{SUFFIX}")
}

/// Sorted sealed labels. Any other `.faj` name is refused.
fn list_sealed(dir: &Path) -> io::Result<Vec<(u64, u64, u64)>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == ACTIVE || !name.ends_with(SUFFIX) {
            continue;
        }
        let label = name
            .strip_prefix(SEALED_PREFIX)
            .and_then(|rest| rest.strip_suffix(SUFFIX))
            .filter(|digits| digits.len() == 20)
            .and_then(|digits| digits.parse::<u64>().ok())
            .ok_or_else(|| invalid(format!("unexpected log file {name}")))?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            return Err(invalid(format!("sealed log {name} is not a regular file")));
        }
        found.push((label, metadata.len(), metadata.ino()));
    }
    found.sort_unstable();
    Ok(found)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Persist a known write or sync failure. If this also fails, only the
/// caller's error report carries the knowledge; see ADR-0011.
fn record_failure(dir: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.join(RECOVERY_REQUIRED))?;
    file.write_all(b"log write or sync reported an error; rebuild from retained source\n")?;
    file.sync_all()?;
    sync_dir(dir)
}

/// Read one frame at `at` in a file of length `len`. `None` means an
/// incomplete tail: a truncated header, payload or marker. A complete header
/// or marker that fails its checks is corruption.
pub fn read_frame(
    mut file: &File,
    at: u64,
    len: u64,
    max_payload: usize,
) -> io::Result<Option<(Vec<u8>, u64)>> {
    if len - at < HEADER_BYTES {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(at))?;
    let mut header = [0_u8; HEADER_BYTES as usize];
    file.read_exact(&mut header)?;
    if &header[..4] != DATA_MAGIC {
        return Err(invalid(format!("bad frame magic at {at}")));
    }
    if crc32fast::hash(&header[..8]) != u32::from_le_bytes(header[8..12].try_into().unwrap()) {
        return Err(invalid("frame header checksum mismatch"));
    }
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if size == 0 || size > max_payload {
        return Err(invalid("invalid frame length"));
    }
    let data_end = at + HEADER_BYTES + size as u64;
    if data_end > len {
        return Ok(None);
    }
    let mut payload = vec![0; size];
    file.read_exact(&mut payload)?;
    if crc32fast::hash(&payload) != u32::from_le_bytes(header[12..16].try_into().unwrap()) {
        return Err(invalid("frame payload checksum mismatch"));
    }
    if data_end + MARKER_BYTES > len {
        return Ok(None);
    }
    let mut marker = [0_u8; MARKER_BYTES as usize];
    file.read_exact(&mut marker)?;
    if &marker[..4] != COMMIT_MAGIC
        || u64::from_le_bytes(marker[4..12].try_into().unwrap()) != data_end
        || crc32fast::hash(&marker[..12]) != u32::from_le_bytes(marker[12..16].try_into().unwrap())
    {
        return Err(invalid("commit marker mismatch"));
    }
    Ok(Some((payload, data_end + MARKER_BYTES)))
}

/// Visit every complete frame of a sealed file; any incomplete frame is corruption.
fn scan_sealed(
    file: &File,
    label: u64,
    len: u64,
    max_payload: usize,
    visit: &mut impl FnMut(&[u8], FramePos) -> io::Result<()>,
) -> io::Result<()> {
    let mut at = 0;
    while at < len {
        let (payload, next) = read_frame(file, at, len, max_payload)?
            .ok_or_else(|| invalid(format!("incomplete frame in sealed log {label}")))?;
        visit(
            &payload,
            FramePos {
                file: FileRef::Sealed(label),
                offset: at,
            },
        )?;
        at = next;
    }
    Ok(())
}

/// Visit complete frames of the active file; returns the committed end.
fn scan_active(
    file: &File,
    len: u64,
    max_payload: usize,
    visit: &mut impl FnMut(&[u8], FramePos) -> io::Result<()>,
) -> io::Result<u64> {
    let mut at = 0;
    while at < len {
        let Some((payload, next)) = read_frame(file, at, len, max_payload)? else {
            break;
        };
        visit(
            &payload,
            FramePos {
                file: FileRef::Active,
                offset: at,
            },
        )?;
        at = next;
    }
    Ok(at)
}

impl FrameLog {
    /// Whether a log exists: `(active file length, sealed file count)`.
    pub fn present(dir: &Path) -> io::Result<(Option<u64>, usize)> {
        let active = match fs::metadata(dir.join(ACTIVE)) {
            Ok(metadata) => Some(metadata.len()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let sealed = match list_sealed(dir) {
            Ok(list) => list.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        Ok((active, sealed))
    }

    /// Open, lock and verify the log in an existing directory, visiting every
    /// committed frame in order. Refuses a recorded failure. Resolves an
    /// interrupted append by truncating the active file's incomplete tail.
    pub fn open(
        dir: &Path,
        max_bytes: u64,
        max_payload: usize,
        visit: impl FnMut(&[u8], FramePos) -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::open_with(dir, max_bytes, max_payload, visit, || Ok(()))
    }

    /// `before_settle_sync` lets tests inject a reported error while open
    /// settles the tail. It is not a runtime configuration.
    fn open_with(
        dir: &Path,
        max_bytes: u64,
        max_payload: usize,
        mut visit: impl FnMut(&[u8], FramePos) -> io::Result<()>,
        mut before_settle_sync: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        if max_bytes < FRAME_OVERHEAD + 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log byte cap too small",
            ));
        }
        if dir.join(RECOVERY_REQUIRED).exists() {
            return Err(io::Error::other(
                "recovery required: a log write or sync reported an error; rebuild from retained source",
            ));
        }
        // With an interrupted append pending, open is settling the durability
        // of a tail. Any error while doing so is a reported error on that tail,
        // so it takes the known-failure path (ADR-0011) instead of letting a
        // later open trust the same readable bytes.
        let settling = dir.join(IN_PROGRESS).exists();
        let known = |error: io::Error| -> io::Error {
            if !settling {
                return error;
            }
            match record_failure(dir) {
                Ok(()) => error,
                Err(record) => io::Error::new(
                    error.kind(),
                    format!("{error}; recording {RECOVERY_REQUIRED} also failed: {record}"),
                ),
            }
        };
        // A sealed file may exist without an active one only after a rotation
        // was interrupted between rename and create; creating it is safe.
        let active = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(ACTIVE))?;
        active.try_lock()?;
        sync_dir(dir).map_err(known)?;
        let listed = list_sealed(dir)?;
        let active_len = active.metadata()?.len();
        let total = listed
            .iter()
            .try_fold(active_len, |sum, (_, bytes, _)| sum.checked_add(*bytes))
            .ok_or_else(|| invalid("log size overflow"))?;
        if total > max_bytes {
            return Err(invalid("log exceeds configured byte cap"));
        }
        let mut sealed = Vec::with_capacity(listed.len());
        for (label, bytes, _) in listed {
            let file = File::open(dir.join(sealed_name(label)))?;
            scan_sealed(&file, label, bytes, max_payload, &mut visit)?;
            sealed.push(Sealed { label, bytes });
        }
        let end = scan_active(&active, active_len, max_payload, &mut visit)?;
        (|| {
            if end < active_len {
                active.set_len(end)?;
            }
            before_settle_sync()?;
            active.sync_all()?;
            match fs::remove_file(dir.join(IN_PROGRESS)) {
                Ok(()) => sync_dir(dir),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        })()
        .map_err(known)?;
        Ok(Self {
            dir: dir.to_owned(),
            active,
            active_end: end,
            sealed,
            max_bytes,
            max_payload,
            poisoned: false,
        })
    }

    /// Read-only, lock-free inspection. A live writer's append or rotation
    /// makes the result retryable rather than inconsistent.
    pub fn inspect(
        dir: &Path,
        max_bytes: u64,
        max_payload: usize,
        mut visit: impl FnMut(&[u8], FramePos) -> io::Result<()>,
    ) -> io::Result<Snapshot> {
        let known_failure = dir.join(RECOVERY_REQUIRED);
        let active = File::open(dir.join(ACTIVE))?;
        let active_meta = active.metadata()?;
        let listed = list_sealed(dir)?;
        let file_bytes = listed
            .iter()
            .try_fold(active_meta.len(), |sum, (_, bytes, _)| {
                sum.checked_add(*bytes)
            })
            .ok_or_else(|| invalid("log size overflow"))?;
        if file_bytes > max_bytes {
            return Err(invalid("log exceeds configured byte cap"));
        }
        // A failure is final for inspection: readable bytes prove nothing.
        if known_failure.exists() {
            return Ok(Snapshot {
                committed_bytes: 0,
                file_bytes,
                recovery_required: true,
                interrupted_append: false,
            });
        }
        if listed.iter().any(|(_, _, ino)| *ino == active_meta.ino()) {
            return Err(retry("log rotated during inspection; retry"));
        }
        let in_progress = dir.join(IN_PROGRESS);
        let mut interrupted_append = false;
        // Held until inspection returns, so the scan below sees a stable file.
        let mut _shared = None;
        if in_progress.exists() {
            // A live writer holds the exclusive lock; retry rather than report a
            // normal append as interrupted.
            _shared = Some(match active.try_lock_shared() {
                Ok(()) => SharedLock(&active),
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(retry("log append in progress; retry inspection"));
                }
                Err(error) => return Err(error.into()),
            });
            if !in_progress.exists() {
                return Err(retry("log append completed during inspection; retry"));
            }
            if known_failure.exists() {
                return Err(retry("log failure recorded during inspection; retry"));
            }
            interrupted_append = true;
        }
        let mut committed_bytes = 0_u64;
        for (label, bytes, _) in &listed {
            let file = match File::open(dir.join(sealed_name(*label))) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(retry("sealed log reclaimed during inspection; retry"));
                }
                Err(error) => return Err(error),
            };
            scan_sealed(&file, *label, *bytes, max_payload, &mut visit)?;
            committed_bytes += bytes;
        }
        committed_bytes += scan_active(&active, active_meta.len(), max_payload, &mut visit)?;
        let relisted: Vec<u64> = list_sealed(dir)?
            .iter()
            .map(|(label, _, _)| *label)
            .collect();
        let still_active = fs::metadata(dir.join(ACTIVE))
            .map(|m| m.ino() == active_meta.ino())
            .unwrap_or(false);
        if relisted
            != listed
                .iter()
                .map(|(label, _, _)| *label)
                .collect::<Vec<_>>()
            || !still_active
        {
            return Err(retry("log rotated or reclaimed during inspection; retry"));
        }
        if known_failure.exists() || (!interrupted_append && in_progress.exists()) {
            return Err(retry(
                "log append in progress or recovery required; retry inspection",
            ));
        }
        Ok(Snapshot {
            committed_bytes,
            file_bytes,
            recovery_required: false,
            interrupted_append,
        })
    }

    pub fn used_bytes(&self) -> u64 {
        self.sealed.iter().map(|s| s.bytes).sum::<u64>() + self.active_end
    }

    pub fn active_bytes(&self) -> u64 {
        self.active_end
    }

    pub fn sealed_labels(&self) -> Vec<u64> {
        self.sealed.iter().map(|s| s.label).collect()
    }

    pub fn end_pos(&self) -> FramePos {
        FramePos {
            file: FileRef::Active,
            offset: self.active_end,
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn quarantine(&mut self, error: io::Error) -> io::Error {
        self.poisoned = true;
        match record_failure(&self.dir) {
            Ok(()) => error,
            Err(record) => io::Error::new(
                error.kind(),
                format!("{error}; recording {RECOVERY_REQUIRED} also failed: {record}"),
            ),
        }
    }

    /// Append one payload with the two-sync commit. Returns its position.
    pub fn append(&mut self, payload: &[u8]) -> io::Result<FramePos> {
        self.append_with(payload, |_| Ok(()))
    }

    /// The callback lets tests inject a reported error or a process death
    /// after each sync stage. It is not a runtime configuration.
    pub fn append_with(
        &mut self,
        payload: &[u8],
        mut after_sync: impl FnMut(SyncStage) -> io::Result<()>,
    ) -> io::Result<FramePos> {
        if self.poisoned {
            return Err(io::Error::other("log quarantined after write error"));
        }
        if payload.is_empty() || payload.len() > self.max_payload {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame payload empty or over cap",
            ));
        }
        let at = self.active_end;
        let data_end = at + HEADER_BYTES + payload.len() as u64;
        let commit_end = data_end + MARKER_BYTES;
        if self.used_bytes() + (commit_end - at) > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "log full: collection must stop; coverage unknown until space is reclaimed",
            ));
        }
        let in_progress = self.dir.join(IN_PROGRESS);
        let result = (|| {
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&in_progress)?;
            marker.write_all(b"append in progress; reopen verifies the tail\n")?;
            marker.sync_all()?;
            sync_dir(&self.dir)?;
            self.active.seek(SeekFrom::Start(at))?;
            let mut header = [0_u8; HEADER_BYTES as usize];
            header[..4].copy_from_slice(DATA_MAGIC);
            header[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            let header_crc = crc32fast::hash(&header[..8]);
            header[8..12].copy_from_slice(&header_crc.to_le_bytes());
            header[12..16].copy_from_slice(&crc32fast::hash(payload).to_le_bytes());
            self.active.write_all(&header)?;
            self.active.write_all(payload)?;
            self.active.sync_all()?;
            after_sync(SyncStage::Data)?;
            let mut commit = [0_u8; MARKER_BYTES as usize];
            commit[..4].copy_from_slice(COMMIT_MAGIC);
            commit[4..12].copy_from_slice(&data_end.to_le_bytes());
            let commit_crc = crc32fast::hash(&commit[..12]);
            commit[12..16].copy_from_slice(&commit_crc.to_le_bytes());
            self.active.write_all(&commit)?;
            self.active.sync_all()?;
            after_sync(SyncStage::Marker)?;
            sync_dir(&self.dir)?;
            after_sync(SyncStage::DirectoryBeforeClear)?;
            fs::remove_file(&in_progress)?;
            after_sync(SyncStage::AfterClear)?;
            sync_dir(&self.dir)
        })();
        if let Err(error) = result {
            return Err(self.quarantine(error));
        }
        self.active_end = commit_end;
        Ok(FramePos {
            file: FileRef::Active,
            offset: at,
        })
    }

    /// Seal the active file under `label` and start an empty active file.
    /// A position inside the old active file is remapped by `remap`.
    pub fn rotate(&mut self, label: u64) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("log quarantined after write error"));
        }
        if self.sealed.last().is_some_and(|s| s.label >= label) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sealed labels must increase",
            ));
        }
        let result = (|| {
            fs::rename(self.dir.join(ACTIVE), self.dir.join(sealed_name(label)))?;
            sync_dir(&self.dir)?;
            let fresh = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(self.dir.join(ACTIVE))?;
            fresh.try_lock()?;
            sync_dir(&self.dir)?;
            Ok::<File, io::Error>(fresh)
        })();
        let fresh = match result {
            Ok(fresh) => fresh,
            Err(error) => return Err(self.quarantine(error)),
        };
        let old = std::mem::replace(&mut self.active, fresh);
        let _ = old.unlock();
        self.sealed.push(Sealed {
            label,
            bytes: self.active_end,
        });
        self.active_end = 0;
        Ok(())
    }

    /// Translate a position taken before `rotate(label)` from an old active
    /// file of length `old_end`.
    pub fn remap(pos: FramePos, label: u64, old_end: u64) -> FramePos {
        match pos.file {
            FileRef::Active if pos.offset >= old_end => FramePos {
                file: FileRef::Active,
                offset: 0,
            },
            FileRef::Active => FramePos {
                file: FileRef::Sealed(label),
                offset: pos.offset,
            },
            FileRef::Sealed(_) => pos,
        }
    }

    /// Delete the oldest sealed file. The owner decides it is reclaimable.
    pub fn remove_oldest_sealed(&mut self) -> io::Result<()> {
        let Some(oldest) = self.sealed.first() else {
            return Ok(());
        };
        fs::remove_file(self.dir.join(sealed_name(oldest.label)))?;
        sync_dir(&self.dir)?;
        self.sealed.remove(0);
        Ok(())
    }

    /// Read the frame at `pos` and the position of the next frame, crossing
    /// from a sealed file to its successor. `None` at the end of the log.
    pub fn read_at(&self, pos: FramePos) -> io::Result<Option<(Vec<u8>, FramePos)>> {
        if self.poisoned {
            return Err(io::Error::other("log quarantined"));
        }
        let mut pos = pos;
        loop {
            match pos.file {
                FileRef::Active => {
                    if pos.offset >= self.active_end {
                        return Ok(None);
                    }
                    let (payload, next) =
                        read_frame(&self.active, pos.offset, self.active_end, self.max_payload)?
                            .ok_or_else(|| invalid("committed frame changed"))?;
                    return Ok(Some((
                        payload,
                        FramePos {
                            file: FileRef::Active,
                            offset: next,
                        },
                    )));
                }
                FileRef::Sealed(label) => {
                    let index = self
                        .sealed
                        .iter()
                        .position(|s| s.label == label)
                        .ok_or_else(|| invalid("position refers to a reclaimed sealed log"))?;
                    let bytes = self.sealed[index].bytes;
                    if pos.offset >= bytes {
                        pos = match self.sealed.get(index + 1) {
                            Some(next) => FramePos {
                                file: FileRef::Sealed(next.label),
                                offset: 0,
                            },
                            None => FramePos {
                                file: FileRef::Active,
                                offset: 0,
                            },
                        };
                        continue;
                    }
                    let file = File::open(self.dir.join(sealed_name(label)))?;
                    let (payload, next) = read_frame(&file, pos.offset, bytes, self.max_payload)?
                        .ok_or_else(|| invalid("sealed frame changed"))?;
                    return Ok(Some((
                        payload,
                        FramePos {
                            file: FileRef::Sealed(label),
                            offset: next,
                        },
                    )));
                }
            }
        }
    }

    /// Move a position at the end of a sealed file to its successor's start,
    /// so it never refers to a file that holds none of its future frames.
    pub fn normalize(&self, pos: FramePos) -> FramePos {
        let FileRef::Sealed(label) = pos.file else {
            return pos;
        };
        let Some(index) = self.sealed.iter().position(|s| s.label == label) else {
            return pos;
        };
        if pos.offset < self.sealed[index].bytes {
            return pos;
        }
        match self.sealed.get(index + 1) {
            Some(next) => FramePos {
                file: FileRef::Sealed(next.label),
                offset: 0,
            },
            None => FramePos {
                file: FileRef::Active,
                offset: 0,
            },
        }
    }

    /// The position of the first frame in the log.
    pub fn start_pos(&self) -> FramePos {
        match self.sealed.first() {
            Some(first) => FramePos {
                file: FileRef::Sealed(first.label),
                offset: 0,
            },
            None => FramePos {
                file: FileRef::Active,
                offset: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("alpha-frame-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn reported_error_while_settling_an_interrupted_append_is_recorded() {
        for (interrupted, expect_recorded) in [(true, true), (false, false)] {
            let dir = scratch(&format!("settle-{interrupted}"));
            let mut log = FrameLog::open(&dir, 1 << 20, 1024, |_, _| Ok(())).unwrap();
            log.append(b"committed").unwrap();
            drop(log);
            if interrupted {
                fs::write(dir.join(IN_PROGRESS), b"x").unwrap();
            }
            let failed = FrameLog::open_with(
                &dir,
                1 << 20,
                1024,
                |_, _| Ok(()),
                || Err(io::Error::other("injected fsync error")),
            );
            assert!(failed.is_err());
            assert_eq!(dir.join(RECOVERY_REQUIRED).exists(), expect_recorded);
            // A recorded failure keeps refusing; without one, a clean open works.
            assert_eq!(
                FrameLog::open(&dir, 1 << 20, 1024, |_, _| Ok(())).is_err(),
                expect_recorded
            );
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}
