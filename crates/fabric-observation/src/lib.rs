//! The version-one **Observation** record and its canonical **FOB1** block
//! encoding.
//!
//! One record type carries the three signals Fabric may ever store: a log
//! line, a metric point and a span. Every record has the same key
//! `(time_ns, node_id, generation, sequence, index)` that the query kernel
//! orders by, the same optional locators (`trace_id`, `span_id`,
//! `parent_span_id`) that join signals, the same typed attributes, and one
//! signal-specific payload. See
//! [ADR-0023](../../../docs/decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md).
//!
//! The encoding is **canonical**: a valid block of records has exactly one
//! byte string, and an accepted byte string decodes to exactly one block
//! whose re-encoding is the same bytes. `decode(encode(b)) == b` for every
//! valid `b`, and `encode(decode(x)) == x` for every accepted `x`. That is
//! what lets a hash of the bytes stand for the records (custody) without a
//! second, raw copy. The decoder rejects everything that is not canonical:
//! overlong varints, dictionaries with duplicate or unused entries or out of
//! first-use order, unsorted or duplicate attribute keys, NaN, reserved bits,
//! trailing bytes and a wrong CRC.
//!
//! The layout is columnar inside a block (times, strands, tags, locators,
//! attributes, payloads), each column delta- or dictionary-coded, so that a
//! block compresses well under a general compressor and a reader can skip
//! what it does not need. Nothing here performs an effect; the crate is a
//! codec and belongs to adapter support ([layers](../../../docs/architecture/layers.json)).
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(kani)]
mod proofs;

/// Block magic: the format name and version.
pub const MAGIC: &[u8; 4] = b"FOB1";
/// The most records one block may hold.
pub const MAX_RECORDS: usize = 65_536;
/// Severity numbers follow the OpenTelemetry log data model: 0 is unspecified, 1 to 24 are TRACE to FATAL4.
pub const MAX_SEVERITY: u8 = 24;

/// The producer's identity: a node and its generation (a Strand).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Strand {
    pub node_id: [u8; 16],
    pub generation: u64,
}

/// Where a record sits in a trace, when it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locators {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent_span_id: Option<[u8; 8]>,
}

/// A number that compares by its bits, so that a record is a value with one encoding.
#[derive(Clone, Copy, Debug)]
pub enum Number {
    Int(i64),
    Double(f64),
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => a == b,
            (Number::Double(a), Number::Double(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}
impl Eq for Number {}

/// An attribute value. Doubles compare by bits; NaN is not a valid value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Double(Bits),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// An `f64` held by its bits, so that `Value` is `Eq` and the encoding is one-to-one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bits(pub u64);

impl Bits {
    pub fn from_f64(v: f64) -> Self {
        Bits(v.to_bits())
    }
    pub fn to_f64(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// How a metric point accumulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    Gauge,
    /// A sum over `[start_ns, time_ns]`; monotonic sums are counters.
    Sum {
        monotonic: bool,
        start_ns: u64,
    },
}

/// Span status, as OpenTelemetry defines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unset,
    Ok,
    Error,
}

/// Span kind, as OpenTelemetry defines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Unspecified,
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

/// The signal-specific part of a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    Log {
        /// 0 unspecified, 1 to 24 as in OpenTelemetry.
        severity: u8,
        /// The event name, empty when there is none.
        event: String,
        body: String,
    },
    Point {
        name: String,
        unit: String,
        kind: PointKind,
        value: Number,
    },
    Span {
        name: String,
        /// End time; `time_ns` is the start.
        end_ns: u64,
        status: Status,
        kind: SpanKind,
    },
}

/// One observation of any kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub strand: Strand,
    /// The producer's Batch sequence.
    pub sequence: u64,
    /// The record's position inside that Batch.
    pub index: u32,
    /// The node's time: observed time of a line, point time, start of a span.
    pub time_ns: u64,
    pub locators: Option<Locators>,
    /// Sorted strictly by key.
    pub attributes: Vec<(String, Value)>,
    pub signal: Signal,
}

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

impl std::error::Error for EncodeError {}

/// Why bytes are not an FOB1 block. `offset` is where the decoder stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for DecodeError {}

/// Checks the canonical-form rules a record must meet to have an encoding.
pub fn check(record: &Observation) -> Result<(), &'static str> {
    for pair in record.attributes.windows(2) {
        if let [(a, _), (b, _)] = pair
            && a >= b
        {
            return Err("attributes must be strictly sorted by key");
        }
    }
    for (_, value) in &record.attributes {
        if let Value::Double(bits) = value
            && bits.to_f64().is_nan()
        {
            return Err("NaN is not a value");
        }
    }
    match &record.signal {
        Signal::Log { severity, .. } if *severity > MAX_SEVERITY => Err("severity above 24"),
        Signal::Point {
            value: Number::Double(d),
            ..
        } if d.is_nan() => Err("NaN is not a value"),
        _ => Ok(()),
    }
}

// ---- varints and deltas -------------------------------------------------

// Shifts by constants below the width and `wrapping_neg` cannot overflow;
// the two functions are each other's inverse (proofs.rs).
#[allow(clippy::arithmetic_side_effects)]
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[allow(clippy::arithmetic_side_effects)]
fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ ((v & 1) as i64).wrapping_neg()
}

/// The signed difference `a - b`, wrapping, as the bijection `u64 -> u64`.
fn delta(a: u64, b: u64) -> u64 {
    zigzag(a.wrapping_sub(b) as i64)
}

fn undelta(d: u64, b: u64) -> u64 {
    b.wrapping_add(unzigzag(d) as u64)
}

fn put_uvar(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_uvar(out, b.len() as u64);
    out.extend_from_slice(b);
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn fail<T>(&self, reason: &'static str) -> Result<T, DecodeError> {
        Err(DecodeError {
            offset: self.pos,
            reason,
        })
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let Some(b) = self.bytes.get(self.pos) else {
            return self.fail("truncated");
        };
        self.pos = self.pos.saturating_add(1);
        Ok(*b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.saturating_add(n);
        let Some(slice) = self.bytes.get(self.pos..end) else {
            return self.fail("truncated");
        };
        self.pos = end;
        Ok(slice)
    }

    /// A LEB128 unsigned varint in its shortest form: at most ten bytes, no
    /// continuation into a zero byte, and the tenth byte at most 1.
    fn uvar(&mut self) -> Result<u64, DecodeError> {
        let start = self.pos;
        let mut value: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let b = self.byte()?;
            let group = u64::from(b & 0x7f);
            if shift == 63 && group > 1 {
                self.pos = start;
                return self.fail("varint overflows 64 bits");
            }
            value |= group << shift;
            if b & 0x80 == 0 {
                if b == 0 && self.pos.saturating_sub(start) > 1 {
                    self.pos = start;
                    return self.fail("overlong varint");
                }
                return Ok(value);
            }
            shift = shift.saturating_add(7);
            if shift > 63 {
                self.pos = start;
                return self.fail("varint longer than ten bytes");
            }
        }
    }

    /// A count that the remaining bytes could hold at `at_least` bytes each.
    fn count(&mut self, at_least: usize) -> Result<usize, DecodeError> {
        let start = self.pos;
        let n = self.uvar()?;
        let Ok(n) = usize::try_from(n) else {
            self.pos = start;
            return self.fail("count too large");
        };
        if n.checked_mul(at_least)
            .is_none_or(|need| need > self.remaining())
        {
            self.pos = start;
            return self.fail("count exceeds the bytes left");
        }
        Ok(n)
    }

    fn bytes_field(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.count(1)?;
        self.take(n)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let start = self.pos;
        let raw = self.bytes_field()?;
        match core::str::from_utf8(raw) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => {
                self.pos = start;
                self.fail("string is not UTF-8")
            }
        }
    }
}

// ---- dictionaries ----------------------------------------------------------

/// Interns values in first-use order while encoding.
struct Intern<T: Ord + Clone> {
    ids: BTreeMap<T, u64>,
    order: Vec<T>,
}

impl<T: Ord + Clone> Intern<T> {
    fn new() -> Self {
        Self {
            ids: BTreeMap::new(),
            order: Vec::new(),
        }
    }
    fn id(&mut self, value: &T) -> u64 {
        if let Some(id) = self.ids.get(value) {
            return *id;
        }
        let id = self.order.len() as u64;
        self.ids.insert(value.clone(), id);
        self.order.push(value.clone());
        id
    }
}

/// Resolves ids while decoding and checks that they appear in first-use order.
struct Lookup<T> {
    table: Vec<T>,
    seen: usize,
}

impl<T: Clone> Lookup<T> {
    fn new(table: Vec<T>) -> Self {
        Self { table, seen: 0 }
    }
    fn get(&mut self, cur: &Cursor<'_>, id: u64) -> Result<T, DecodeError> {
        let Ok(id) = usize::try_from(id) else {
            return cur.fail("dictionary id out of range");
        };
        if id > self.seen {
            return cur.fail("dictionary id used before its first-use turn");
        }
        if id == self.seen {
            self.seen = self.seen.saturating_add(1);
        }
        match self.table.get(id) {
            Some(v) => Ok(v.clone()),
            None => cur.fail("dictionary id out of range"),
        }
    }
    fn all_used(&self) -> bool {
        self.seen == self.table.len()
    }
}

// ---- tags ------------------------------------------------------------------

const TAG_KIND_MASK: u8 = 0b0000_0011; // 0 log, 1 point, 2 span
const TAG_LOCATORS: u8 = 0b0000_0100;
const TAG_PARENT: u8 = 0b0000_1000;
const TAG_RESERVED: u8 = 0b1111_0000;

const VALUE_STR: u8 = 0;
const VALUE_INT: u8 = 1;
const VALUE_DOUBLE: u8 = 2;
const VALUE_FALSE: u8 = 3;
const VALUE_TRUE: u8 = 4;
const VALUE_BYTES: u8 = 5;

const NUMBER_INT: u8 = 0;
const NUMBER_DOUBLE: u8 = 1;

const POINT_GAUGE: u8 = 0;
const POINT_SUM: u8 = 1;
const POINT_COUNTER: u8 = 2;

// ---- encode ----------------------------------------------------------------

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

    // Column 1: times, delta-of-delta from the first.
    let mut times = Vec::new();
    let mut prev_t = 0_u64;
    let mut prev_d = 0_u64;
    for (i, r) in records.iter().enumerate() {
        if i == 0 {
            put_uvar(&mut times, r.time_ns);
        } else {
            let d = r.time_ns.wrapping_sub(prev_t);
            put_uvar(&mut times, delta(d, prev_d));
            prev_d = d;
        }
        prev_t = r.time_ns;
    }
    // Column 2: strands and positions, each a delta from the previous record.
    let mut strands = Vec::new();
    let (mut pg, mut ps, mut pi) = (0_u64, 0_u64, 0_u64);
    for r in records {
        put_uvar(&mut strands, nodes.id(&r.strand.node_id));
        put_uvar(&mut strands, delta(r.strand.generation, pg));
        put_uvar(&mut strands, delta(r.sequence, ps));
        put_uvar(&mut strands, delta(u64::from(r.index), pi));
        (pg, ps, pi) = (r.strand.generation, r.sequence, u64::from(r.index));
    }
    // Column 3: tags.
    let mut tags = Vec::new();
    for r in records {
        let kind = match r.signal {
            Signal::Log { .. } => 0,
            Signal::Point { .. } => 1,
            Signal::Span { .. } => 2,
        };
        let mut tag = kind;
        if let Some(l) = &r.locators {
            tag |= TAG_LOCATORS;
            if l.parent_span_id.is_some() {
                tag |= TAG_PARENT;
            }
        }
        tags.push(tag);
    }
    // Column 4: locators.
    let mut locators = Vec::new();
    for r in records {
        if let Some(l) = &r.locators {
            locators.extend_from_slice(&l.trace_id);
            locators.extend_from_slice(&l.span_id);
            if let Some(p) = &l.parent_span_id {
                locators.extend_from_slice(p);
            }
        }
    }
    // Column 5: attributes.
    let mut attributes = Vec::new();
    for r in records {
        put_uvar(&mut attributes, r.attributes.len() as u64);
        for (key, value) in &r.attributes {
            put_uvar(&mut attributes, strings.id(key));
            match value {
                Value::Str(s) => {
                    attributes.push(VALUE_STR);
                    put_uvar(&mut attributes, strings.id(s));
                }
                Value::Int(i) => {
                    attributes.push(VALUE_INT);
                    put_uvar(&mut attributes, zigzag(*i));
                }
                Value::Double(b) => {
                    attributes.push(VALUE_DOUBLE);
                    attributes.extend_from_slice(&b.0.to_le_bytes());
                }
                Value::Bool(false) => attributes.push(VALUE_FALSE),
                Value::Bool(true) => attributes.push(VALUE_TRUE),
                Value::Bytes(b) => {
                    attributes.push(VALUE_BYTES);
                    put_bytes(&mut attributes, b);
                }
            }
        }
    }
    // Column 6: payloads.
    let mut payloads = Vec::new();
    for r in records {
        match &r.signal {
            Signal::Log {
                severity,
                event,
                body,
            } => {
                payloads.push(*severity);
                put_uvar(&mut payloads, strings.id(event));
                put_bytes(&mut payloads, body.as_bytes());
            }
            Signal::Point {
                name,
                unit,
                kind,
                value,
            } => {
                put_uvar(&mut payloads, strings.id(name));
                put_uvar(&mut payloads, strings.id(unit));
                match kind {
                    PointKind::Gauge => payloads.push(POINT_GAUGE),
                    PointKind::Sum {
                        monotonic,
                        start_ns,
                    } => {
                        payloads.push(if *monotonic { POINT_COUNTER } else { POINT_SUM });
                        put_uvar(&mut payloads, delta(*start_ns, r.time_ns));
                    }
                }
                match value {
                    Number::Int(i) => {
                        payloads.push(NUMBER_INT);
                        put_uvar(&mut payloads, zigzag(*i));
                    }
                    Number::Double(d) => {
                        payloads.push(NUMBER_DOUBLE);
                        payloads.extend_from_slice(&d.to_bits().to_le_bytes());
                    }
                }
            }
            Signal::Span {
                name,
                end_ns,
                status,
                kind,
            } => {
                put_uvar(&mut payloads, strings.id(name));
                put_uvar(&mut payloads, delta(*end_ns, r.time_ns));
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
    // Assemble: header, dictionaries, columns, CRC.
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_uvar(&mut out, records.len() as u64);
    put_uvar(&mut out, nodes.order.len() as u64);
    for id in &nodes.order {
        out.extend_from_slice(id);
    }
    put_uvar(&mut out, strings.order.len() as u64);
    for s in &strings.order {
        put_bytes(&mut out, s.as_bytes());
    }
    for column in [&times, &strands, &tags, &locators, &attributes, &payloads] {
        out.extend_from_slice(column);
    }
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

// ---- decode ----------------------------------------------------------------

/// Decodes an FOB1 block, accepting only its canonical form.
pub fn decode(bytes: &[u8]) -> Result<Vec<Observation>, DecodeError> {
    let body_len = bytes.len().saturating_sub(4);
    let (Some(body), Some(tail)) = (bytes.get(..body_len), bytes.get(body_len..)) else {
        return Err(DecodeError {
            offset: 0,
            reason: "truncated",
        });
    };
    if bytes.len() < 4 {
        return Err(DecodeError {
            offset: 0,
            reason: "truncated",
        });
    }
    let mut cur = Cursor {
        bytes: body,
        pos: 0,
    };
    if cur.take(4)? != MAGIC {
        cur.pos = 0;
        return cur.fail("not an FOB1 block");
    }
    let mut expected = [0_u8; 4];
    expected.copy_from_slice(tail);
    if crc32fast::hash(body) != u32::from_le_bytes(expected) {
        cur.pos = body_len;
        return cur.fail("CRC mismatch");
    }
    let n = cur.count(1)?;
    if n == 0 {
        return cur.fail("a block holds at least one record");
    }
    if n > MAX_RECORDS {
        return cur.fail("too many records");
    }
    // Dictionaries: every entry distinct (the encoder interns, so a duplicate
    // entry would re-encode to a different table) and, checked as the columns
    // are read, used in first-use order with nothing left over.
    let node_count = cur.count(16)?;
    let mut node_table = Vec::with_capacity(node_count);
    let mut node_seen = BTreeSet::new();
    for _ in 0..node_count {
        let at = cur.pos;
        let mut id = [0_u8; 16];
        id.copy_from_slice(cur.take(16)?);
        if !node_seen.insert(id) {
            cur.pos = at;
            return cur.fail("duplicate node dictionary entry");
        }
        node_table.push(id);
    }
    let mut nodes = Lookup::new(node_table);
    let string_count = cur.count(1)?;
    let mut string_table = Vec::with_capacity(string_count);
    let mut string_seen = BTreeSet::new();
    for _ in 0..string_count {
        let at = cur.pos;
        let s = cur.string()?;
        if !string_seen.insert(s.clone()) {
            cur.pos = at;
            return cur.fail("duplicate string dictionary entry");
        }
        string_table.push(s);
    }
    let mut strings = Lookup::new(string_table);

    // Column 1: times.
    let mut times = Vec::with_capacity(n);
    let mut prev_t = 0_u64;
    let mut prev_d = 0_u64;
    for i in 0..n {
        let t = if i == 0 {
            cur.uvar()?
        } else {
            let d = undelta(cur.uvar()?, prev_d);
            prev_d = d;
            prev_t.wrapping_add(d)
        };
        prev_t = t;
        times.push(t);
    }
    // Column 2: strands.
    let mut strands = Vec::with_capacity(n);
    let (mut pg, mut ps, mut pi) = (0_u64, 0_u64, 0_u64);
    for _ in 0..n {
        let node_id = {
            let id = cur.uvar()?;
            nodes.get(&cur, id)?
        };
        let generation = undelta(cur.uvar()?, pg);
        let sequence = undelta(cur.uvar()?, ps);
        let index = undelta(cur.uvar()?, pi);
        let Ok(index32) = u32::try_from(index) else {
            return cur.fail("index above u32");
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
        let tag = cur.byte()?;
        if tag & TAG_RESERVED != 0 || tag & TAG_KIND_MASK == 3 {
            cur.pos = cur.pos.saturating_sub(1);
            return cur.fail("reserved tag bits");
        }
        if tag & TAG_PARENT != 0 && tag & TAG_LOCATORS == 0 {
            cur.pos = cur.pos.saturating_sub(1);
            return cur.fail("parent span without locators");
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
        let mut trace_id = [0_u8; 16];
        trace_id.copy_from_slice(cur.take(16)?);
        let mut span_id = [0_u8; 8];
        span_id.copy_from_slice(cur.take(8)?);
        let parent_span_id = if tag & TAG_PARENT != 0 {
            let mut p = [0_u8; 8];
            p.copy_from_slice(cur.take(8)?);
            Some(p)
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
        let count = cur.count(2)?;
        let mut list: Vec<(String, Value)> = Vec::with_capacity(count);
        for _ in 0..count {
            let key_at = cur.pos;
            let key = {
                let id = cur.uvar()?;
                strings.get(&cur, id)?
            };
            if list.last().is_some_and(|(k, _)| *k >= key) {
                cur.pos = key_at;
                return cur.fail("attributes must be strictly sorted by key");
            }
            let value = match cur.byte()? {
                VALUE_STR => Value::Str({
                    let id = cur.uvar()?;
                    strings.get(&cur, id)?
                }),
                VALUE_INT => Value::Int(unzigzag(cur.uvar()?)),
                VALUE_DOUBLE => {
                    let mut b = [0_u8; 8];
                    b.copy_from_slice(cur.take(8)?);
                    let bits = Bits(u64::from_le_bytes(b));
                    if bits.to_f64().is_nan() {
                        cur.pos = cur.pos.saturating_sub(8);
                        return cur.fail("NaN is not a value");
                    }
                    Value::Double(bits)
                }
                VALUE_FALSE => Value::Bool(false),
                VALUE_TRUE => Value::Bool(true),
                VALUE_BYTES => Value::Bytes(cur.bytes_field()?.to_vec()),
                _ => {
                    cur.pos = cur.pos.saturating_sub(1);
                    return cur.fail("unknown value tag");
                }
            };
            list.push((key, value));
        }
        attributes.push(list);
    }
    // Column 6: payloads.
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let (Some(tag), Some(&time_ns), Some(&(strand, sequence, index))) =
            (tags.get(i), times.get(i), strands.get(i))
        else {
            return cur.fail("internal column length mismatch");
        };
        let signal = match tag & TAG_KIND_MASK {
            0 => {
                let severity = cur.byte()?;
                if severity > MAX_SEVERITY {
                    cur.pos = cur.pos.saturating_sub(1);
                    return cur.fail("severity above 24");
                }
                let event = {
                    let id = cur.uvar()?;
                    strings.get(&cur, id)?
                };
                let body = cur.string()?;
                Signal::Log {
                    severity,
                    event,
                    body,
                }
            }
            1 => {
                let name = {
                    let id = cur.uvar()?;
                    strings.get(&cur, id)?
                };
                let unit = {
                    let id = cur.uvar()?;
                    strings.get(&cur, id)?
                };
                let kind = match cur.byte()? {
                    POINT_GAUGE => PointKind::Gauge,
                    k @ (POINT_SUM | POINT_COUNTER) => PointKind::Sum {
                        monotonic: k == POINT_COUNTER,
                        start_ns: undelta(cur.uvar()?, time_ns),
                    },
                    _ => {
                        cur.pos = cur.pos.saturating_sub(1);
                        return cur.fail("unknown point kind");
                    }
                };
                let value = match cur.byte()? {
                    NUMBER_INT => Number::Int(unzigzag(cur.uvar()?)),
                    NUMBER_DOUBLE => {
                        let mut b = [0_u8; 8];
                        b.copy_from_slice(cur.take(8)?);
                        let d = f64::from_bits(u64::from_le_bytes(b));
                        if d.is_nan() {
                            cur.pos = cur.pos.saturating_sub(8);
                            return cur.fail("NaN is not a value");
                        }
                        Number::Double(d)
                    }
                    _ => {
                        cur.pos = cur.pos.saturating_sub(1);
                        return cur.fail("unknown number tag");
                    }
                };
                Signal::Point {
                    name,
                    unit,
                    kind,
                    value,
                }
            }
            _ => {
                let name = {
                    let id = cur.uvar()?;
                    strings.get(&cur, id)?
                };
                let end_ns = undelta(cur.uvar()?, time_ns);
                let status = match cur.byte()? {
                    0 => Status::Unset,
                    1 => Status::Ok,
                    2 => Status::Error,
                    _ => {
                        cur.pos = cur.pos.saturating_sub(1);
                        return cur.fail("unknown span status");
                    }
                };
                let kind = match cur.byte()? {
                    0 => SpanKind::Unspecified,
                    1 => SpanKind::Internal,
                    2 => SpanKind::Server,
                    3 => SpanKind::Client,
                    4 => SpanKind::Producer,
                    5 => SpanKind::Consumer,
                    _ => {
                        cur.pos = cur.pos.saturating_sub(1);
                        return cur.fail("unknown span kind");
                    }
                };
                Signal::Span {
                    name,
                    end_ns,
                    status,
                    kind,
                }
            }
        };
        let (Some(locators), Some(attributes)) = (locators.get(i), attributes.get(i)) else {
            return cur.fail("internal column length mismatch");
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

#[cfg(test)]
mod tests;
