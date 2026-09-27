//! Fabric O11y's small event model, synthetic workload source, local buffer, and log.
//!
//! The event types describe what an observation means. They do not choose how
//! it is encoded, buffered, transported, or stored. The generator and buffer
//! modules use those types without changing their meaning.

pub mod alpha;
pub mod buffer;
pub mod generator;
pub mod log;

/// Identifies one event. The newtype keeps it distinct from other numeric IDs.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct EventId(pub u64);

/// Identifies the component that emitted or observed an event.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub u64);

/// Identifies the thing being described, such as a process or service.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ResourceId(pub u64);

/// Identifies the customer or logical owner of the event.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct TenantId(pub u64);

/// Wall-clock time when the described event happened, in Unix nanoseconds.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventTime(pub i64);

/// Wall-clock time when the source observed the event, in Unix nanoseconds.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObservedTime(pub i64);

/// A small first version of the values we allow on an event.
#[derive(Debug, PartialEq)]
pub enum Scalar {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
}

/// Extra typed context attached to an event.
#[derive(Debug, PartialEq)]
pub struct Attribute {
    pub key: String,
    pub value: Scalar,
}

/// The signal-specific data carried by an event.
#[derive(Debug, PartialEq)]
pub enum Payload {
    Log {
        body: String,
    },
    Gauge {
        name: String,
        value: f64,
        unit: String,
    },
}

/// A claim about a resource, made by a source at a particular time.
#[derive(Debug, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub tenant: TenantId,
    pub source: SourceId,
    pub resource: ResourceId,
    pub event_time: EventTime,
    pub observed_time: ObservedTime,
    pub attributes: Vec<Attribute>,
    pub payload: Payload,
}
