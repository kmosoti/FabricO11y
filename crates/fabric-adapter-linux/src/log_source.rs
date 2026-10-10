//! One bounded pass over an explicitly selected newline log file.

use fabric_core::collection::{CursorCheck, CursorFacts, FileFacts, InvalidCursor, check_cursor};
use fabric_frame::envelope::{BtrfsIdentity, Cursor};
use std::fs::OpenOptions;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_LINE: usize = 4096;
const MAX_SCAN: usize = 1024 * 1024;
const MAX_LINES: usize = 8192;
const MAX_GAPS: usize = 8;
const PREFIX_BYTES: usize = 64;

pub struct Line {
    pub body: String,
    pub path: String,
    pub device: u64,
    pub inode: u64,
    pub start: u64,
    pub end: u64,
}

pub struct ReadResult {
    pub lines: Vec<Line>,
    pub cursor: Cursor,
    pub gaps: Vec<String>,
    /// Bytes after the returned cursor at the time of the read.
    pub backlog_bytes: u64,
}

fn issue(kind: &str, path: &str) -> String {
    format!("{kind}: {path}")
}

/// Compare namespaces before using an old inode or consumed offset. Absence
/// in the new observation means a successfully identified non-Btrfs filesystem;
/// a failed probe is propagated before this comparison, never mapped to absence.
fn same_file(old: &Cursor, device: u64, inode: u64, identity: Option<&BtrfsIdentity>) -> bool {
    let same_namespace = match (&old.btrfs_identity, identity) {
        (Some(old), Some(now)) => old == now,
        (Some(_), None) => false,
        (None, _) => old.device == device,
    };
    same_namespace && old.inode == inode
}

/// Whether a committed cursor still describes this open file: same filesystem
/// and inode, not shorter, and the same witnessed consumed prefix. Leaves the
/// file position after the prefix when it checks one.
fn cursor_still_valid(
    file: &mut std::fs::File,
    metadata: &std::fs::Metadata,
    old: &Cursor,
    identity: Option<&BtrfsIdentity>,
) -> io::Result<bool> {
    if !same_file(old, metadata.dev(), metadata.ino(), identity) {
        return Ok(false);
    }
    let facts = CursorFacts {
        device: old.device,
        inode: old.inode,
        offset: old.offset,
        prefix_len: old.prefix_len,
    };
    let now = FileFacts {
        // Namespace equality was independently established above; use one
        // comparison key for the unchanged pure length/prefix decision.
        device: old.device,
        inode: metadata.ino(),
        len: metadata.len(),
    };
    match check_cursor(facts, now) {
        Ok(CursorCheck::Restart) => Ok(false),
        Ok(CursorCheck::Continue) => Ok(true),
        Ok(CursorCheck::VerifyPrefix { len }) => {
            let mut prefix = vec![0; len as usize];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut prefix)?;
            Ok(crc32fast::hash(&prefix) == old.prefix_crc)
        }
        Err(InvalidCursor) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid log cursor prefix",
        )),
    }
}

/// Bytes the reader would still have to read: after the cursor when it is
/// still valid, otherwise the whole file (it would restart from zero).
pub fn unread_bytes(path: &Path, prior: Option<&Cursor>) -> io::Result<u64> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log source is not a regular file",
        ));
    }
    let identity = crate::btrfs_identity::read(&file)?;
    Ok(match prior {
        Some(old) if cursor_still_valid(&mut file, &metadata, old, identity.as_ref())? => {
            metadata.len() - old.offset
        }
        _ => metadata.len(),
    })
}

pub fn read_lines(
    path: &Path,
    prior: Option<&Cursor>,
    body_budget: usize,
) -> io::Result<ReadResult> {
    read_lines_costed(path, prior, body_budget, 0)
}

/// As `read_lines`, with each accepted line also costing `per_line` bytes of the
/// budget: the encoding overhead its record adds beyond the body (its path and
/// offsets), so a caller can bound the encoded size of what it collects.
pub fn read_lines_costed(
    path: &Path,
    prior: Option<&Cursor>,
    body_budget: usize,
    per_line: usize,
) -> io::Result<ReadResult> {
    read_lines_with_identity(
        path,
        prior,
        body_budget,
        per_line,
        crate::btrfs_identity::read,
    )
}

fn read_lines_with_identity(
    path: &Path,
    prior: Option<&Cursor>,
    body_budget: usize,
    per_line: usize,
    identify: impl FnOnce(&std::fs::File) -> io::Result<Option<BtrfsIdentity>>,
) -> io::Result<ReadResult> {
    let name = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-UTF8 log path"))?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log source is not a regular file",
        ));
    }
    let device = metadata.dev();
    let inode = metadata.ino();
    let identity = identify(&file)?;
    let mut gaps = Vec::new();
    let (start, mut oversize) = match prior {
        Some(old)
            if same_file(old, device, inode, identity.as_ref()) && metadata.len() >= old.offset =>
        {
            if !cursor_still_valid(&mut file, &metadata, old, identity.as_ref())? {
                gaps.push(issue(
                    "log consumed prefix changed; previous tail unknown",
                    name,
                ));
                (0, false)
            } else {
                (old.offset, old.skipping_oversize)
            }
        }
        Some(old) if same_file(old, device, inode, identity.as_ref()) => {
            gaps.push(issue("log truncated; previous tail unknown", name));
            (0, false)
        }
        Some(_) => {
            gaps.push(issue("log rotated; old tail unknown", name));
            (0, false)
        }
        None => (0, false),
    };
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::with_capacity(4096, file);
    let mut lines = Vec::new();
    let mut line = Vec::with_capacity(MAX_LINE);
    let mut at = start;
    let mut committed = start;
    let mut scanned = 0;
    let mut body_bytes = 0;
    let mut byte = [0_u8; 1];
    while scanned < MAX_SCAN
        && lines.len() < MAX_LINES
        && gaps.len() < MAX_GAPS
        && reader.read(&mut byte)? != 0
    {
        scanned += 1;
        at += 1;
        if byte[0] != b'\n' {
            if !oversize {
                if line.len() < MAX_LINE {
                    line.push(byte[0]);
                } else {
                    oversize = true;
                    line.clear();
                    gaps.push(issue("oversize log line skipped", name));
                }
            }
            continue;
        }
        if oversize {
            committed = at;
        } else if body_bytes + line.len() + per_line > body_budget {
            break;
        } else {
            match String::from_utf8(std::mem::take(&mut line)) {
                Ok(body) => {
                    body_bytes += body.len() + per_line;
                    lines.push(Line {
                        body,
                        path: name.to_owned(),
                        device,
                        inode,
                        start: committed,
                        end: at,
                    });
                }
                Err(_) => gaps.push(issue("invalid UTF-8 log line skipped", name)),
            }
            committed = at;
        }
        line.clear();
        oversize = false;
    }
    if oversize {
        // The gap and this skip position commit with the batch. A later pass,
        // including after restart, continues to the newline without returning
        // any suffix of the discarded oversized line as a new record.
        committed = at;
    }
    let backlog_bytes = metadata.len().saturating_sub(committed);
    let prefix_len = committed.min(PREFIX_BYTES as u64) as usize;
    let prefix_crc = if prefix_len == 0 {
        0
    } else {
        let mut prefix = vec![0; prefix_len];
        reader.get_mut().seek(SeekFrom::Start(0))?;
        reader.get_mut().read_exact(&mut prefix)?;
        crc32fast::hash(&prefix)
    };
    Ok(ReadResult {
        lines,
        cursor: Cursor {
            path: name.into(),
            device,
            inode,
            offset: committed,
            skipping_oversize: oversize,
            prefix_len: prefix_len as u32,
            prefix_crc,
            btrfs_identity: identity,
        },
        gaps,
        backlog_bytes,
    })
}

#[cfg(test)]
mod btrfs_cursor_tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let root = PathBuf::from(
                std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher required"),
            )
            .join(format!(
                "btrfs-cursor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn log(&self) -> PathBuf {
            self.0.join("selected.log")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn identity() -> BtrfsIdentity {
        BtrfsIdentity {
            uuid: vec![7; 16],
            subvolume_id: 256,
        }
    }
    fn read(path: &Path, old: Option<&Cursor>, id: Option<BtrfsIdentity>) -> ReadResult {
        read_lines_with_identity(path, old, 4096, 0, |_| Ok(id)).unwrap()
    }

    #[test]
    fn actual_fedora_51_to_32_counterexample_keeps_three_consumed_lines_consumed() {
        // Origin: failed Fedora44 lifecycle continuation02. The old device-only
        // comparison reread three lines at inode2505 after st_dev51 became32.
        let synthetic = Cursor {
            device: 51,
            inode: 2505,
            btrfs_identity: Some(identity()),
            ..Default::default()
        };
        assert!(same_file(&synthetic, 32, 2505, Some(&identity())));
        let legacy = Cursor {
            btrfs_identity: None,
            ..synthetic
        };
        assert!(!same_file(&legacy, 32, 2505, Some(&identity())));
        let scratch = Scratch::new();
        let path = scratch.log();
        std::fs::write(&path, b"first\nsecond\nthird\n").unwrap();
        let mut old = read(&path, None, Some(identity())).cursor;
        old.device = old.device.wrapping_add(1); // metadata's current device differs
        let after_reboot = read(&path, Some(&old), Some(identity()));
        assert!(after_reboot.lines.is_empty());
        assert!(after_reboot.gaps.is_empty());
        assert_eq!(after_reboot.cursor.offset, old.offset);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"fourth\n")
            .unwrap();
        let appended = read(&path, Some(&after_reboot.cursor), Some(identity()));
        assert_eq!(appended.lines.len(), 1);
        assert_eq!(appended.lines[0].body, "fourth");
        assert_eq!(appended.lines[0].start, old.offset);
    }

    #[test]
    fn uuid_subvolume_inode_and_known_filesystem_replacement_never_inherit_offset() {
        let scratch = Scratch::new();
        let path = scratch.log();
        std::fs::write(&path, b"first\nsecond\nthird\n").unwrap();
        let old = read(&path, None, Some(identity())).cursor;
        let mut different_uuid = identity();
        different_uuid.uuid[0] ^= 1;
        let mut different_subvolume = identity();
        different_subvolume.subvolume_id += 1;
        for changed in [Some(different_uuid), Some(different_subvolume), None] {
            let replacement = read(&path, Some(&old), changed);
            assert_eq!(replacement.lines.len(), 3);
            assert_eq!(replacement.gaps.len(), 1);
            assert!(replacement.gaps[0].starts_with("log rotated"));
        }
        let mut other_inode = old;
        other_inode.inode = other_inode.inode.wrapping_add(1);
        assert_eq!(
            read(&path, Some(&other_inode), Some(identity()))
                .lines
                .len(),
            3
        );
    }

    #[test]
    fn stable_namespace_still_checks_prefix_and_length() {
        let scratch = Scratch::new();
        let path = scratch.log();
        std::fs::write(&path, b"first\nsecond\nthird\n").unwrap();
        let old = read(&path, None, Some(identity())).cursor;
        std::fs::write(&path, b"FIRST\nSECOND\nTHIRD\n").unwrap();
        let replaced = read(&path, Some(&old), Some(identity()));
        assert_eq!(replaced.lines.len(), 3);
        assert!(replaced.gaps[0].starts_with("log consumed prefix changed"));
        std::fs::write(&path, b"short\n").unwrap();
        let truncated = read(&path, Some(&old), Some(identity()));
        assert_eq!(truncated.lines.len(), 1);
        assert!(truncated.gaps[0].starts_with("log truncated"));
    }

    #[test]
    fn legacy_migration_requires_raw_identity_and_probe_failure_never_returns_new_cursor() {
        let scratch = Scratch::new();
        let path = scratch.log();
        std::fs::write(&path, b"first\nsecond\nthird\n").unwrap();
        let old = read(&path, None, None).cursor;
        let migrated = read(&path, Some(&old), Some(identity()));
        assert!(migrated.lines.is_empty());
        assert_eq!(migrated.cursor.offset, old.offset);
        assert_eq!(migrated.cursor.btrfs_identity, Some(identity()));
        let mut already_renumbered = old.clone();
        already_renumbered.device = already_renumbered.device.wrapping_add(1);
        let uncertain = read(&path, Some(&already_renumbered), Some(identity()));
        assert_eq!(uncertain.lines.len(), 3);
        assert!(uncertain.gaps[0].starts_with("log rotated"));
        let prior = migrated.cursor;
        let saved = prior.clone();
        let error = read_lines_with_identity(&path, Some(&prior), 4096, 0, |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(error.err().unwrap().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(prior, saved);
    }
}
