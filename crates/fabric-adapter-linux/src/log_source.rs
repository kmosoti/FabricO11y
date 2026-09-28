//! One bounded pass over an explicitly selected newline log file.

use crate::spindle::spool::Cursor;
use fabric_core::collection::{CursorCheck, CursorFacts, FileFacts, InvalidCursor, check_cursor};
use std::fs::OpenOptions;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_LINE: usize = 4096;
const MAX_SCAN: usize = 256 * 1024;
const MAX_LINES: usize = 128;
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

/// Whether a committed cursor still describes this open file: same device
/// and inode, not shorter, and the same witnessed consumed prefix. Leaves the
/// file position after the prefix when it checks one.
fn cursor_still_valid(
    file: &mut std::fs::File,
    metadata: &std::fs::Metadata,
    old: &Cursor,
) -> io::Result<bool> {
    let facts = CursorFacts {
        device: old.device,
        inode: old.inode,
        offset: old.offset,
        prefix_len: old.prefix_len,
    };
    let now = FileFacts {
        device: metadata.dev(),
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
    Ok(match prior {
        Some(old) if cursor_still_valid(&mut file, &metadata, old)? => metadata.len() - old.offset,
        _ => metadata.len(),
    })
}

pub fn read_lines(
    path: &Path,
    prior: Option<&Cursor>,
    body_budget: usize,
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
    let mut gaps = Vec::new();
    let (start, mut oversize) = match prior {
        Some(old) if old.device == device && old.inode == inode && metadata.len() >= old.offset => {
            if !cursor_still_valid(&mut file, &metadata, old)? {
                gaps.push(issue(
                    "log consumed prefix changed; previous tail unknown",
                    name,
                ));
                (0, false)
            } else {
                (old.offset, old.skipping_oversize)
            }
        }
        Some(old) if old.device == device && old.inode == inode => {
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
        } else if body_bytes + line.len() > body_budget {
            break;
        } else {
            match String::from_utf8(std::mem::take(&mut line)) {
                Ok(body) => {
                    body_bytes += body.len();
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
        },
        gaps,
        backlog_bytes,
    })
}
