//! Bounded payload ownership and the synchronization adapter.
mod queue;
use crate::collection::Observation;
use admission_kernel::Slot;
pub use queue::{Lease, Queue};
use std::{
    fmt,
    io::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Accepted,
    Invalid,
    TooLarge,
    Full,
    Contended,
    Closed,
    Poisoned,
    Exhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    InvalidLimits,
    CapacityOverflow,
    Allocation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    Contended,
    Poisoned,
}
impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BuildError {}
impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for QueueError {}

/// Reserved payload and collection storage; fixed control state is reported separately.
/// Allocator bookkeeping, Arc's private header, stacks and transient error objects
/// are excluded from these byte counts. No buffer or collection grows after construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Storage {
    pub slots: usize,
    pub max_record: usize,
    pub payload_bytes: usize,
    pub collection_bytes: usize,
    pub control_bytes: usize,
    pub control_blocks: usize,
}
fn slots(max_bytes: usize, max_record: usize) -> Result<usize, BuildError> {
    if max_record == 0 || max_bytes / max_record == 0 {
        return Err(BuildError::InvalidLimits);
    }
    Ok(max_bytes / max_record)
}
struct Buffer {
    bytes: Vec<u8>,
    credit: Slot,
}
/// Restores the reservation before its synchronization guard can be poisoned.
struct Reservation<'a, F: FnMut(Buffer)> {
    buffer: Option<Buffer>,
    restore: F,
    fault: &'a AtomicBool,
}
impl<'a, F: FnMut(Buffer)> Reservation<'a, F> {
    fn new(buffer: Buffer, restore: F, fault: &'a AtomicBool) -> Self {
        Self {
            buffer: Some(buffer),
            restore,
            fault,
        }
    }
    fn buffer(&mut self) -> &mut Buffer {
        self.buffer.as_mut().expect("live reservation")
    }
    fn take(&mut self) -> Buffer {
        self.buffer.take().expect("live reservation")
    }
}
impl<F: FnMut(Buffer)> Drop for Reservation<'_, F> {
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            if !buffer.credit.cancel() {
                self.fault.store(true, Ordering::Release);
            }
            (self.restore)(buffer);
        }
    }
}
impl Buffer {
    fn new(id: usize, limit: usize) -> Result<Self, BuildError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(limit)
            .map_err(|_| BuildError::Allocation)?;
        bytes.resize(limit, 0);
        Ok(Self {
            bytes,
            credit: Slot::new(id, limit),
        })
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.credit.encoded_len()]
    }
}
fn checked_bytes(count: usize, element: usize) -> Result<usize, BuildError> {
    count
        .checked_mul(element)
        .ok_or(BuildError::CapacityOverflow)
}
fn storage(
    slots: usize,
    max_record: usize,
    payload_bytes: usize,
    collection_bytes: usize,
    control_bytes: usize,
) -> Result<Storage, BuildError> {
    payload_bytes
        .checked_add(collection_bytes)
        .and_then(|n| n.checked_add(control_bytes))
        .ok_or(BuildError::CapacityOverflow)?;
    Ok(Storage {
        slots,
        max_record,
        payload_bytes,
        collection_bytes,
        control_bytes,
        control_blocks: 1,
    })
}
struct Writer<'a> {
    bytes: &'a mut [u8],
    used: usize,
    full: bool,
}
impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(end) = admission_kernel::reserve(self.bytes.len(), self.used, bytes.len()) else {
            self.full = true;
            return Err(io::ErrorKind::WriteZero.into());
        };
        self.bytes[self.used..end].copy_from_slice(bytes);
        self.used = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(buffer: &mut Buffer, observation: &Observation) -> Result<usize, Admission> {
    let mut writer = Writer {
        bytes: &mut buffer.bytes,
        used: 0,
        full: false,
    };
    match serde_json::to_writer(&mut writer, observation) {
        Ok(()) => Ok(writer.used),
        Err(_) if writer.full => Err(Admission::TooLarge),
        Err(_) => Err(Admission::Invalid),
    }
}
struct FaultOnPanic<'a>(&'a AtomicBool);
impl Drop for FaultOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
