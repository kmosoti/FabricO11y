//! Level 7: the Observation record.
//!
//! One record for a log line, a metric point or a span. Every record has the
//! key the query kernel orders by (`time_ns`, `node_id`, `generation`,
//! `sequence`, `index`), the same optional trace locators, the same typed
//! attributes, and one signal payload. [`check`] states the canonical-form
//! rules a record must meet to have an encoding at all: sorted attribute
//! keys, finite doubles, a severity in range. Everything else about a record
//! is free, so the encoding stays total over the type.

use crate::cells::{Number, Value, check_attributes, check_double};
use alloc::string::String;
use alloc::vec::Vec;

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

/// Checks the canonical-form rules a record must meet to have an encoding.
pub fn check(record: &Observation) -> Result<(), &'static str> {
    check_attributes(&record.attributes)?;
    match &record.signal {
        Signal::Log { severity, .. } if *severity > MAX_SEVERITY => Err("severity above 24"),
        Signal::Point {
            value: Number::Double(d),
            ..
        } => check_double(crate::cells::Bits::from_f64(*d)),
        _ => Ok(()),
    }
}
