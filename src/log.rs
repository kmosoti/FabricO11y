//! A single-writer, append-only event log for the local delivery lesson.
//!
//! A successful `append` is the receiver-side durability boundary: event bytes
//! are synced before a commit marker is written and synced.
//! `append` borrows the event so a failed or ambiguous write leaves the caller
//! responsible for retrying it. Recovery discards an unmarked final event;
//! detected corruption in fields already present is an error. A storage sync
//! failure leaves the file's durability uncertain; do not auto-resume that
//! path without recovery from an independent trusted copy.

use crate::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: [u8; 4] = *b"FOL2";
const COMMIT_MAGIC: [u8; 4] = *b"FOC2";
const HEADER_BYTES: usize = 16;
const COMMIT_BYTES: usize = 16;
// Six 8-byte identity/time fields, a 4-byte empty attribute count, and the
// smallest payload: Log tag plus a 4-byte empty body length.
const MIN_RECORD_BYTES: usize = 57;
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Compare exactly the bytes that this log format would persist for two events.
///
/// `Event`'s ordinary floating-point equality considers `0.0` and `-0.0`
/// equal, while the log preserves their distinct bits. Prefix recovery needs
/// the stronger comparison to avoid accepting a different stored record.
pub fn same_record_contents(left: &Event, right: &Event) -> io::Result<bool> {
    Ok(encode_event(left)? == encode_event(right)?)
}

/// Owns one local log file and its advisory exclusive lock.
///
/// `open` requires read/write access because it checks every record and may
/// truncate an incomplete tail. All writers must respect this lock.
pub struct EventLog {
    file: File,
    end: u64,
    poisoned: bool,
}

impl EventLog {
    /// Open or create a log, validate its records, and remove an incomplete tail.
    /// A record with a valid header and fully present but malformed payload is
    /// never silently skipped.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()?;

        // The name of a newly created log is part of its durability contract.
        // Resolve symlinks after creation so we sync the actual file's parent,
        // not just the directory that contains a link to it. Ancestor and
        // symlink names must already be durable, and the path must not be
        // concurrently renamed while the log is open.
        sync_parent(&path.canonicalize()?)?;
        let end = recover(&mut file)?;
        // A complete event without a commit marker is discarded by recover.
        // Sync the resulting validated prefix before exposing it for replay.
        file.sync_all()?;
        Ok(Self {
            file,
            end,
            poisoned: false,
        })
    }

    /// Commit one event. Success is the local acknowledgement boundary.
    ///
    /// On an I/O error the file may contain part or all of the event and
    /// marker, so this handle refuses further operations. A failed sync can
    /// also leave earlier writes uncertain; reopening and validating bytes
    /// alone does not establish their durability. Recover from an independent
    /// trusted copy on a healthy storage path before retrying the event.
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        self.ensure_healthy()?;
        let payload = encode_event(event)?;
        let len = u32::try_from(payload.len()).expect("encoded record is bounded to 16 MiB");
        let mut header = [0_u8; HEADER_BYTES];
        header[..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&len.to_le_bytes());
        let header_crc = crc32fast::hash(&header[..8]);
        header[8..12].copy_from_slice(&header_crc.to_le_bytes());
        header[12..16].copy_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        let data_end = self
            .end
            .checked_add(HEADER_BYTES as u64 + u64::from(len))
            .ok_or_else(|| invalid_input("log offset overflow"))?;
        let committed_end = data_end
            .checked_add(COMMIT_BYTES as u64)
            .ok_or_else(|| invalid_input("log offset overflow"))?;
        let marker = commit_marker(data_end);

        let result = (|| {
            self.file.seek(SeekFrom::Start(self.end))?;
            self.file.write_all(&header)?;
            self.file.write_all(&payload)?;
            self.file.sync_all()?;
            self.file.write_all(&marker)?;
            self.file.sync_all()?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.end = committed_end;
                Ok(())
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }

    /// Visit committed records in file order without collecting them in memory.
    pub fn replay(&mut self, mut visit: impl FnMut(Event) -> io::Result<()>) -> io::Result<usize> {
        self.ensure_healthy()?;
        let mut offset = 0;
        let mut count = 0;
        while offset < self.end {
            match read_record(&mut self.file, offset, self.end)? {
                Frame::Complete(event, next) => {
                    visit(event)?;
                    offset = next;
                    count += 1;
                }
                Frame::Incomplete => {
                    return Err(invalid_data("committed log changed during replay"));
                }
            }
        }
        Ok(count)
    }

    fn ensure_healthy(&self) -> io::Result<()> {
        if self.poisoned {
            Err(io::Error::other(
                "log write failed; durability uncertain until rebuilt from a trusted source",
            ))
        } else {
            Ok(())
        }
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()
}

enum Frame {
    Complete(Event, u64),
    Incomplete,
}

fn recover(file: &mut File) -> io::Result<u64> {
    let file_len = file.metadata()?.len();
    let mut offset = 0;
    while offset < file_len {
        match read_record(file, offset, file_len)? {
            Frame::Complete(_, next) => offset = next,
            Frame::Incomplete => {
                file.set_len(offset)?;
                file.sync_all()?;
                break;
            }
        }
    }
    Ok(offset)
}

fn read_record(file: &mut File, offset: u64, file_len: u64) -> io::Result<Frame> {
    let Frame::Complete(event, data_end) = read_frame(file, offset, file_len)? else {
        return Ok(Frame::Incomplete);
    };
    let Some(committed_end) = read_commit_marker(file, data_end, file_len)? else {
        return Ok(Frame::Incomplete);
    };
    Ok(Frame::Complete(event, committed_end))
}

fn check_partial_frame(file: &mut File, offset: u64, available: usize) -> io::Result<()> {
    debug_assert!(available < HEADER_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0_u8; HEADER_BYTES];
    file.read_exact(&mut header[..available])?;
    let magic_bytes = available.min(MAGIC.len());
    if header[..magic_bytes] != MAGIC[..magic_bytes] {
        return Err(invalid_data(format!(
            "bad partial log magic at byte {offset}"
        )));
    }
    if available >= 8 {
        let len = u32::from_le_bytes(header[4..8].try_into().unwrap());
        if (len as usize) < MIN_RECORD_BYTES || len as usize > MAX_RECORD_BYTES {
            return Err(invalid_data(format!(
                "invalid partial log record length at byte {offset}"
            )));
        }
        let expected_crc = crc32fast::hash(&header[..8]).to_le_bytes();
        let checksum_bytes = available.saturating_sub(8).min(4);
        if header[8..8 + checksum_bytes] != expected_crc[..checksum_bytes] {
            return Err(invalid_data(format!(
                "bad partial log header checksum at byte {offset}"
            )));
        }
    }
    Ok(())
}

fn commit_marker(data_end: u64) -> [u8; COMMIT_BYTES] {
    let mut marker = [0_u8; COMMIT_BYTES];
    marker[..4].copy_from_slice(&COMMIT_MAGIC);
    marker[4..12].copy_from_slice(&data_end.to_le_bytes());
    let crc = crc32fast::hash(&marker[..12]);
    marker[12..16].copy_from_slice(&crc.to_le_bytes());
    marker
}

fn check_partial_marker(file: &mut File, data_end: u64, available: usize) -> io::Result<()> {
    debug_assert!(available < COMMIT_BYTES);
    file.seek(SeekFrom::Start(data_end))?;
    let mut prefix = [0_u8; COMMIT_BYTES];
    file.read_exact(&mut prefix[..available])?;
    if prefix[..available] != commit_marker(data_end)[..available] {
        return Err(invalid_data(format!(
            "bad partial commit marker at byte {data_end}"
        )));
    }
    Ok(())
}

fn read_frame(file: &mut File, offset: u64, file_len: u64) -> io::Result<Frame> {
    if file_len - offset < HEADER_BYTES as u64 {
        check_partial_frame(file, offset, (file_len - offset) as usize)?;
        return Ok(Frame::Incomplete);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0; HEADER_BYTES];
    file.read_exact(&mut header)?;

    if header[..4] != MAGIC {
        return Err(invalid_data(format!("bad log magic at byte {offset}")));
    }
    let len = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let expected_header_crc = u32::from_le_bytes(header[8..12].try_into().unwrap());
    if crc32fast::hash(&header[..8]) != expected_header_crc {
        return Err(invalid_data(format!(
            "bad log header checksum at byte {offset}"
        )));
    }
    if (len as usize) < MIN_RECORD_BYTES || len as usize > MAX_RECORD_BYTES {
        return Err(invalid_data(format!(
            "invalid log record length at byte {offset}"
        )));
    }
    let next = offset
        .checked_add(HEADER_BYTES as u64 + u64::from(len))
        .ok_or_else(|| invalid_data("log offset overflow"))?;
    if next > file_len {
        return Ok(Frame::Incomplete);
    }

    let expected_crc = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let mut payload = vec![0; len as usize];
    file.read_exact(&mut payload)?;
    if crc32fast::hash(&payload) != expected_crc {
        return Err(invalid_data(format!("bad log checksum at byte {offset}")));
    }
    let event = decode_event(&payload)
        .map_err(|error| invalid_data(format!("invalid event at byte {offset}: {error}")))?;
    Ok(Frame::Complete(event, next))
}

fn read_commit_marker(file: &mut File, data_end: u64, file_len: u64) -> io::Result<Option<u64>> {
    let remaining = file_len - data_end;
    if remaining < COMMIT_BYTES as u64 {
        check_partial_marker(file, data_end, remaining as usize)?;
        return Ok(None);
    }
    file.seek(SeekFrom::Start(data_end))?;
    let mut marker = [0_u8; COMMIT_BYTES];
    file.read_exact(&mut marker)?;
    if marker[..4] != COMMIT_MAGIC {
        return Err(invalid_data(format!("bad commit magic at byte {data_end}")));
    }
    let recorded_end = u64::from_le_bytes(marker[4..12].try_into().unwrap());
    let expected_crc = u32::from_le_bytes(marker[12..16].try_into().unwrap());
    if recorded_end != data_end || crc32fast::hash(&marker[..12]) != expected_crc {
        return Err(invalid_data(format!(
            "bad commit marker at byte {data_end}"
        )));
    }
    Ok(Some(data_end + COMMIT_BYTES as u64))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct Encoder(Vec<u8>);

impl Encoder {
    fn bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RECORD_BYTES {
            return Err(invalid_input("event exceeds maximum log record size"));
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> io::Result<()> {
        self.bytes(&[value])
    }

    fn u32(&mut self, value: u32) -> io::Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> io::Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn i64(&mut self, value: i64) -> io::Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn string(&mut self, value: &str) -> io::Result<()> {
        let len = u32::try_from(value.len())
            .map_err(|_| invalid_input("string exceeds u32 length limit"))?;
        self.u32(len)?;
        self.bytes(value.as_bytes())
    }
}

fn encode_event(event: &Event) -> io::Result<Vec<u8>> {
    let mut out = Encoder(Vec::new());
    out.u64(event.id.0)?;
    out.u64(event.tenant.0)?;
    out.u64(event.source.0)?;
    out.u64(event.resource.0)?;
    out.i64(event.event_time.0)?;
    out.i64(event.observed_time.0)?;
    let attributes = u32::try_from(event.attributes.len())
        .map_err(|_| invalid_input("too many attributes for log record"))?;
    out.u32(attributes)?;
    for attribute in &event.attributes {
        out.string(&attribute.key)?;
        match &attribute.value {
            Scalar::Bool(value) => {
                out.u8(0)?;
                out.u8(u8::from(*value))?;
            }
            Scalar::I64(value) => {
                out.u8(1)?;
                out.i64(*value)?;
            }
            Scalar::U64(value) => {
                out.u8(2)?;
                out.u64(*value)?;
            }
            Scalar::F64(value) => {
                out.u8(3)?;
                out.u64(value.to_bits())?;
            }
            Scalar::String(value) => {
                out.u8(4)?;
                out.string(value)?;
            }
        }
    }
    match &event.payload {
        Payload::Log { body } => {
            out.u8(0)?;
            out.string(body)?;
        }
        Payload::Gauge { name, value, unit } => {
            out.u8(1)?;
            out.string(name)?;
            out.u64(value.to_bits())?;
            out.string(unit)?;
        }
    }
    Ok(out.0)
}

struct Decoder<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid_data("truncated event payload"))?;
        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec())
            .map_err(|_| invalid_data("event string is not UTF-8"))
    }
}

fn decode_event(bytes: &[u8]) -> io::Result<Event> {
    let mut input = Decoder { bytes, pos: 0 };
    let id = EventId(input.u64()?);
    let tenant = TenantId(input.u64()?);
    let source = SourceId(input.u64()?);
    let resource = ResourceId(input.u64()?);
    let event_time = EventTime(input.i64()?);
    let observed_time = ObservedTime(input.i64()?);
    let count = input.u32()?;
    let mut attributes = Vec::new();
    for _ in 0..count {
        let key = input.string()?;
        let value = match input.u8()? {
            0 => match input.u8()? {
                0 => Scalar::Bool(false),
                1 => Scalar::Bool(true),
                _ => return Err(invalid_data("invalid bool value")),
            },
            1 => Scalar::I64(input.i64()?),
            2 => Scalar::U64(input.u64()?),
            3 => Scalar::F64(f64::from_bits(input.u64()?)),
            4 => Scalar::String(input.string()?),
            _ => return Err(invalid_data("unknown scalar tag")),
        };
        attributes.push(Attribute { key, value });
    }
    let payload = match input.u8()? {
        0 => Payload::Log {
            body: input.string()?,
        },
        1 => Payload::Gauge {
            name: input.string()?,
            value: f64::from_bits(input.u64()?),
            unit: input.string()?,
        },
        _ => return Err(invalid_data("unknown payload tag")),
    };
    if input.pos != bytes.len() {
        return Err(invalid_data("trailing bytes in event payload"));
    }
    Ok(Event {
        id,
        tenant,
        source,
        resource,
        event_time,
        observed_time,
        attributes,
        payload,
    })
}
