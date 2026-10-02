//! Level 8: the FOB1 block.
//!
//! A block is one to 65,536 records laid out column by column so that each
//! column can use the level below that fits it: times through second-order
//! deltas, strands and positions through first-order deltas, strings and
//! node ids through dictionaries, cells through their typed forms. The
//! layout:
//!
//! ```text
//! "FOB1"
//! varint record count
//! varint node count, then 16-byte node ids           (dictionary, first-use order)
//! varint string count, then length-prefixed strings  (dictionary, first-use order)
//! column 1  times:      first value, then second-order delta codes
//! column 2  strands:    per record  node id, Δgeneration, Δsequence, Δindex
//! column 3  tags:       per record  one byte: kind (2 bits), locators, parent
//! column 4  locators:   per flagged record  trace id, span id, [parent span id]
//! column 5  attributes: per record  count, then (key id, value tag, value)
//! column 6  payloads:   per record  the signal's cells
//! CRC-32 (IEEE, level 1) of everything above, little-endian
//! ```
//!
//! Canonicality is the sum of the levels' rules plus three of this level's
//! own: reserved tag bits are zero, the block ends exactly after the CRC,
//! and the dictionaries are referenced in the order the columns are read
//! (so the table order is a function of the records). Hence every valid
//! block has one byte string and every accepted byte string is that
//! encoding of its decoding.

use crate::bits::unpack_le;
use crate::bytes::{Cursor, DecodeError, put_u32_le};
use crate::cells::{get_attributes, get_number, put_attributes, put_number};
use crate::crc32;
use crate::delta::{SecondOrderDecoder, SecondOrderEncoder, step, unstep};
use crate::dictionary::{Intern, Lookup};
use crate::record::{
    Locators, MAX_SEVERITY, Observation, PointKind, Signal, SpanKind, Status, Strand, check,
};
use crate::varint;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Block magic: the format name and version.
pub const MAGIC: &[u8; 4] = b"FOB1";
/// The most records one block may hold.
pub const MAX_RECORDS: usize = 65_536;

const TAG_KIND_MASK: u8 = 0b0000_0011; // 0 log, 1 point, 2 span
const TAG_LOCATORS: u8 = 0b0000_0100;
const TAG_PARENT: u8 = 0b0000_1000;
const TAG_RESERVED: u8 = 0b1111_0000;
const POINT_GAUGE: u8 = 0;
const POINT_SUM: u8 = 1;
const POINT_COUNTER: u8 = 2;

/// Why a block of records cannot be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    Empty,
    TooManyRecords(usize),
    /// Record `index` is not canonical for the stated reason.
    Invalid {
        index: usize,
        reason: &'static str,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Empty => write!(f, "a block holds at least one record"),
            EncodeError::TooManyRecords(n) => write!(f, "{n} records exceed {MAX_RECORDS}"),
            EncodeError::Invalid { index, reason } => write!(f, "record {index}: {reason}"),
        }
    }
}

impl core::error::Error for EncodeError {}

/// Encodes a block of records to its one canonical byte string.
pub fn encode(records: &[Observation]) -> Result<Vec<u8>, EncodeError> {
    if records.is_empty() {
        return Err(EncodeError::Empty);
    }
    if records.len() > MAX_RECORDS {
        return Err(EncodeError::TooManyRecords(records.len()));
    }
    for (index, r) in records.iter().enumerate() {
        check(r).map_err(|reason| EncodeError::Invalid { index, reason })?;
    }
    let mut nodes: Intern<[u8; 16]> = Intern::new();
    let mut strings: Intern<String> = Intern::new();

    let mut times = Vec::new();
    let mut enc = SecondOrderEncoder::new();
    for r in records {
        varint::put(&mut times, enc.code(r.time_ns));
    }
    let mut strands = Vec::new();
    let (mut pg, mut ps, mut pi) = (0_u64, 0_u64, 0_u64);
    for r in records {
        varint::put(&mut strands, nodes.id(&r.strand.node_id));
        varint::put(&mut strands, step(r.strand.generation, pg));
        varint::put(&mut strands, step(r.sequence, ps));
        varint::put(&mut strands, step(u64::from(r.index), pi));
        (pg, ps, pi) = (r.strand.generation, r.sequence, u64::from(r.index));
    }
    let mut tags = Vec::new();
    for r in records {
        let mut tag = match r.signal {
            Signal::Log { .. } => 0,
            Signal::Point { .. } => 1,
            Signal::Span { .. } => 2,
        };
        if let Some(l) = &r.locators {
            tag |= TAG_LOCATORS;
            if l.parent_span_id.is_some() {
                tag |= TAG_PARENT;
            }
        }
        tags.push(tag);
    }
    let mut locators = Vec::new();
    for l in records.iter().filter_map(|r| r.locators.as_ref()) {
        locators.extend_from_slice(&l.trace_id);
        locators.extend_from_slice(&l.span_id);
        if let Some(p) = &l.parent_span_id {
            locators.extend_from_slice(p);
        }
    }
    let mut attributes = Vec::new();
    for r in records {
        put_attributes(&mut attributes, &r.attributes, &mut strings);
    }
    let mut payloads = Vec::new();
    for r in records {
        match &r.signal {
            Signal::Log {
                severity,
                event,
                body,
            } => {
                payloads.push(*severity);
                varint::put(&mut payloads, strings.id(event));
                varint::put_bytes(&mut payloads, body.as_bytes());
            }
            Signal::Point {
                name,
                unit,
                kind,
                value,
            } => {
                varint::put(&mut payloads, strings.id(name));
                varint::put(&mut payloads, strings.id(unit));
                match kind {
                    PointKind::Gauge => payloads.push(POINT_GAUGE),
                    PointKind::Sum {
                        monotonic,
                        start_ns,
                    } => {
                        payloads.push(if *monotonic { POINT_COUNTER } else { POINT_SUM });
                        varint::put(&mut payloads, step(*start_ns, r.time_ns));
                    }
                }
                put_number(&mut payloads, *value);
            }
            Signal::Span {
                name,
                end_ns,
                status,
                kind,
            } => {
                varint::put(&mut payloads, strings.id(name));
                varint::put(&mut payloads, step(*end_ns, r.time_ns));
                payloads.push(match status {
                    Status::Unset => 0,
                    Status::Ok => 1,
                    Status::Error => 2,
                });
                payloads.push(match kind {
                    SpanKind::Unspecified => 0,
                    SpanKind::Internal => 1,
                    SpanKind::Server => 2,
                    SpanKind::Client => 3,
                    SpanKind::Producer => 4,
                    SpanKind::Consumer => 5,
                });
            }
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    varint::put(&mut out, records.len() as u64);
    varint::put(&mut out, nodes.table().len() as u64);
    for id in nodes.table() {
        out.extend_from_slice(id);
    }
    varint::put(&mut out, strings.table().len() as u64);
    for s in strings.table() {
        varint::put_bytes(&mut out, s.as_bytes());
    }
    for column in [&times, &strands, &tags, &locators, &attributes, &payloads] {
        out.extend_from_slice(column);
    }
    let crc = crc32::hash(&out);
    put_u32_le(&mut out, crc);
    Ok(out)
}

/// Decodes an FOB1 block, accepting only its canonical form.
pub fn decode(bytes: &[u8]) -> Result<Vec<Observation>, DecodeError> {
    let fail = |offset: usize, reason: &'static str| Err(DecodeError { offset, reason });
    let Some(body_len) = bytes.len().checked_sub(4) else {
        return fail(0, "truncated");
    };
    let (Some(body), Some(tail)) = (bytes.get(..body_len), bytes.get(body_len..)) else {
        return fail(0, "truncated");
    };
    let mut cur = Cursor::new(body);
    if cur.take(4)? != MAGIC {
        return cur.fail_at(0, "not an FOB1 block");
    }
    if u64::from(crc32::hash(body)) != unpack_le(tail) {
        return fail(body_len, "CRC mismatch");
    }
    let n_at = cur.position();
    let n = varint::count(&mut cur, 1)?;
    if n == 0 {
        return cur.fail_at(n_at, "a block holds at least one record");
    }
    if n > MAX_RECORDS {
        return cur.fail_at(n_at, "too many records");
    }
    // Dictionaries.
    let node_count = varint::count(&mut cur, 16)?;
    let mut node_table = Vec::with_capacity(node_count);
    let mut node_starts = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        node_starts.push(cur.position());
        node_table.push(cur.array::<16>()?);
    }
    let mut nodes = Lookup::new(
        &mut cur,
        node_table,
        &node_starts,
        "duplicate node dictionary entry",
    )?;
    let string_count = varint::count(&mut cur, 1)?;
    let mut string_table: Vec<String> = Vec::with_capacity(string_count);
    let mut string_starts = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        string_starts.push(cur.position());
        string_table.push(varint::get_string(&mut cur)?);
    }
    let mut strings = Lookup::new(
        &mut cur,
        string_table,
        &string_starts,
        "duplicate string dictionary entry",
    )?;
    // Column 1: times.
    let mut times = Vec::with_capacity(n);
    let mut dec = SecondOrderDecoder::new();
    for _ in 0..n {
        times.push(dec.value(varint::get(&mut cur)?));
    }
    // Column 2: strands.
    let mut strands = Vec::with_capacity(n);
    let (mut pg, mut ps, mut pi) = (0_u64, 0_u64, 0_u64);
    for _ in 0..n {
        let at = cur.position();
        let id = varint::get(&mut cur)?;
        let node_id = nodes.get(&mut cur, at, id)?;
        let generation = unstep(varint::get(&mut cur)?, pg);
        let sequence = unstep(varint::get(&mut cur)?, ps);
        let index_at = cur.position();
        let index = unstep(varint::get(&mut cur)?, pi);
        let Ok(index32) = u32::try_from(index) else {
            return cur.fail_at(index_at, "index above u32");
        };
        (pg, ps, pi) = (generation, sequence, index);
        strands.push((
            Strand {
                node_id,
                generation,
            },
            sequence,
            index32,
        ));
    }
    if !nodes.all_used() {
        return cur.fail("unused node dictionary entry");
    }
    // Column 3: tags.
    let mut tags = Vec::with_capacity(n);
    for _ in 0..n {
        let at = cur.position();
        let tag = cur.byte()?;
        if tag & TAG_RESERVED != 0 || tag & TAG_KIND_MASK == 3 {
            return cur.fail_at(at, "reserved tag bits");
        }
        if tag & TAG_PARENT != 0 && tag & TAG_LOCATORS == 0 {
            return cur.fail_at(at, "parent span without locators");
        }
        tags.push(tag);
    }
    // Column 4: locators.
    let mut locators = Vec::with_capacity(n);
    for tag in &tags {
        if tag & TAG_LOCATORS == 0 {
            locators.push(None);
            continue;
        }
        let trace_id = cur.array::<16>()?;
        let span_id = cur.array::<8>()?;
        let parent_span_id = if tag & TAG_PARENT != 0 {
            Some(cur.array::<8>()?)
        } else {
            None
        };
        locators.push(Some(Locators {
            trace_id,
            span_id,
            parent_span_id,
        }));
    }
    // Column 5: attributes.
    let mut attributes = Vec::with_capacity(n);
    for _ in 0..n {
        attributes.push(get_attributes(&mut cur, &mut strings)?);
    }
    // Column 6: payloads, assembled with the columns above.
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let (
            Some(tag),
            Some(&time_ns),
            Some(&(strand, sequence, index)),
            Some(locators),
            Some(attributes),
        ) = (
            tags.get(i),
            times.get(i),
            strands.get(i),
            locators.get(i),
            attributes.get(i),
        )
        else {
            return cur.fail("internal column length mismatch");
        };
        let signal = match tag & TAG_KIND_MASK {
            0 => {
                let at = cur.position();
                let severity = cur.byte()?;
                if severity > MAX_SEVERITY {
                    return cur.fail_at(at, "severity above 24");
                }
                let at = cur.position();
                let id = varint::get(&mut cur)?;
                let event = strings.get(&mut cur, at, id)?;
                let body = varint::get_string(&mut cur)?;
                Signal::Log {
                    severity,
                    event,
                    body,
                }
            }
            1 => {
                let at = cur.position();
                let id = varint::get(&mut cur)?;
                let name = strings.get(&mut cur, at, id)?;
                let at = cur.position();
                let id = varint::get(&mut cur)?;
                let unit = strings.get(&mut cur, at, id)?;
                let at = cur.position();
                let kind = match cur.byte()? {
                    POINT_GAUGE => PointKind::Gauge,
                    k @ (POINT_SUM | POINT_COUNTER) => PointKind::Sum {
                        monotonic: k == POINT_COUNTER,
                        start_ns: unstep(varint::get(&mut cur)?, time_ns),
                    },
                    _ => return cur.fail_at(at, "unknown point kind"),
                };
                let value = get_number(&mut cur)?;
                Signal::Point {
                    name,
                    unit,
                    kind,
                    value,
                }
            }
            _ => {
                let at = cur.position();
                let id = varint::get(&mut cur)?;
                let name = strings.get(&mut cur, at, id)?;
                let end_ns = unstep(varint::get(&mut cur)?, time_ns);
                let at = cur.position();
                let status = match cur.byte()? {
                    0 => Status::Unset,
                    1 => Status::Ok,
                    2 => Status::Error,
                    _ => return cur.fail_at(at, "unknown span status"),
                };
                let at = cur.position();
                let kind = match cur.byte()? {
                    0 => SpanKind::Unspecified,
                    1 => SpanKind::Internal,
                    2 => SpanKind::Server,
                    3 => SpanKind::Client,
                    4 => SpanKind::Producer,
                    5 => SpanKind::Consumer,
                    _ => return cur.fail_at(at, "unknown span kind"),
                };
                Signal::Span {
                    name,
                    end_ns,
                    status,
                    kind,
                }
            }
        };
        records.push(Observation {
            strand,
            sequence,
            index,
            time_ns,
            locators: *locators,
            attributes: attributes.clone(),
            signal,
        });
    }
    if !strings.all_used() {
        return cur.fail("unused string dictionary entry");
    }
    if cur.remaining() != 0 {
        return cur.fail("trailing bytes");
    }
    Ok(records)
}
