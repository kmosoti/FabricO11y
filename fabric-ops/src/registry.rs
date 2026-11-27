//! Versioned metadata and typed bindings, independent of transport and codecs.

use crate::Shape;
use std::marker::PhantomData;

/// Maximum number of operation contracts in one in-memory registry.
pub const MAX_OPERATIONS: usize = 256;
/// Maximum bytes in a protocol or operation name.
pub const MAX_NAME_BYTES: usize = 128;

/// Version numbers describe an adapter's operation contract, not authority.
/// A protocol version and an operation version are independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub protocol: &'static str,
    pub protocol_version: u32,
    pub operation: &'static str,
    pub operation_version: u32,
    pub shape: Shape,
}

/// A concrete adapter implements this for each operation it supports. The
/// associated types are retained after negotiation; no values are erased.
pub trait Operation {
    type Request;
    type Response;
    const DESCRIPTOR: Descriptor;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    InvalidDescriptor,
    TooManyOperations,
    DuplicateDescriptor,
    UnsupportedOperation,
    UnsupportedProtocolVersion,
    UnsupportedOperationVersion,
    BindingMismatch,
}

/// Bounded static inventory. Construct once at adapter setup and retain it.
#[derive(Clone, Copy, Debug)]
pub struct Registry {
    entries: &'static [Descriptor],
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Registry {
    pub fn new(entries: &'static [Descriptor]) -> Result<Self, RegistryError> {
        if entries.len() > MAX_OPERATIONS {
            return Err(RegistryError::TooManyOperations);
        }
        for (index, entry) in entries.iter().enumerate() {
            if !valid_name(entry.protocol)
                || !valid_name(entry.operation)
                || entry.protocol_version == 0
                || entry.operation_version == 0
            {
                return Err(RegistryError::InvalidDescriptor);
            }
            if entries[..index].iter().any(|prior| {
                prior.protocol == entry.protocol
                    && prior.protocol_version == entry.protocol_version
                    && prior.operation == entry.operation
                    && prior.operation_version == entry.operation_version
            }) {
                return Err(RegistryError::DuplicateDescriptor);
            }
        }
        Ok(Self { entries })
    }

    /// Validate independently supplied metadata before invoking the typed handler.
    /// The returned token binds a registered descriptor to the concrete types.
    pub fn negotiate<O: Operation>(
        &self,
        protocol: &str,
        protocol_version: u32,
        operation: &str,
        operation_version: u32,
    ) -> Result<Negotiated<O>, RegistryError> {
        let candidates = self
            .entries
            .iter()
            .filter(|entry| entry.protocol == protocol && entry.operation == operation);
        let mut found_name = false;
        let mut found_protocol_version = false;
        for entry in candidates {
            found_name = true;
            if entry.protocol_version != protocol_version {
                continue;
            }
            found_protocol_version = true;
            if entry.operation_version != operation_version {
                continue;
            }
            if *entry != O::DESCRIPTOR {
                return Err(RegistryError::BindingMismatch);
            }
            return Ok(Negotiated(PhantomData));
        }
        if !found_name {
            Err(RegistryError::UnsupportedOperation)
        } else if !found_protocol_version {
            Err(RegistryError::UnsupportedProtocolVersion)
        } else {
            Err(RegistryError::UnsupportedOperationVersion)
        }
    }
}

/// Evidence of metadata matching only. It confers no identity or authority.
#[derive(Debug)]
pub struct Negotiated<O: Operation>(PhantomData<fn() -> O>);
impl<O: Operation> Negotiated<O> {
    /// Calls only the handler for the bound request and response types.
    pub fn call(
        self,
        request: O::Request,
        handler: impl FnOnce(O::Request) -> O::Response,
    ) -> O::Response {
        handler(request)
    }
}

/// Bounded, payload-free outcomes. Interpretation is supplied by the adapter;
/// none of these variants itself authorizes replay or implies a durable result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationError {
    /// A retry may be considered only under the operation's replay contract.
    Retry,
    /// Reconcile against authoritative state before deciding what to do next.
    Reconcile,
    /// The applicable local deadline or lease has expired. Expiry alone says
    /// nothing about whether an external effect occurred.
    Expired,
    /// Completion is unknown; do not assume either success or failure.
    UnknownOutcome,
}
