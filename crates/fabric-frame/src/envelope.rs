//! Version-one Fabric batch envelope: the exact bytes a Spindle stores in its
//! Spool, sends, and the server commits. Field numbers and caps are part of
//! the wire and persisted format; a type move is not permission to change them.

use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use prost::Message;
use std::io;

pub const MAX_BATCH: usize = 1024 * 1024;
// One coverage-unknown notice, one host failure, and eight bounded gaps from
// each of sixteen log sources.
pub const MAX_GAPS_PER_BATCH: usize = 2 + 16 * 8;
pub const MAX_GAP_BYTES: usize = 256;

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
