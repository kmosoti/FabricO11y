//! One bounded pass over an explicitly selected newline log file.

use crate::alpha::journal::Cursor;
use std::fs::OpenOptions;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_LINE: usize = 4096;
const MAX_SCAN: usize = 256 * 1024;
const MAX_LINES: usize = 128;
const MAX_GAPS: usize = 8;

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
}

fn issue(kind: &str, path: &str) -> String {
    format!("{kind}: {path}")
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
            (old.offset, old.skipping_oversize)
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
    Ok(ReadResult {
        lines,
        cursor: Cursor {
            path: name.into(),
            device,
            inode,
            offset: committed,
            skipping_oversize: oversize,
        },
        gaps,
    })
}
