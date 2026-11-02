use super::*;
use admission_kernel::Fifo;
use std::{
    collections::VecDeque,
    mem::size_of,
    ops::Deref,
    sync::{Arc, Mutex, TryLockError, atomic::AtomicUsize},
};
struct State {
    ready: VecDeque<(usize, u64)>,
    order: Fifo,
    closed: bool,
    rejected: u64,
    leased: usize,
}
struct Shared {
    slots: Vec<Mutex<Option<Buffer>>>,
    state: Mutex<State>,
    fault: AtomicBool,
    probe: AtomicUsize,
    storage: Storage,
}
#[derive(Clone)]
pub struct Queue {
    shared: Arc<Shared>,
}
pub struct Lease {
    shared: Arc<Shared>,
    buffer: Option<Buffer>,
    ticket: u64,
    index: usize,
}
impl Deref for Lease {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.buffer.as_ref().expect("live lease").bytes()
    }
}
impl Lease {
    pub fn sequence(&self) -> u64 {
        self.ticket
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let Some(mut buffer) = self.buffer.take() else {
            return;
        };
        let mut slot = self.shared.slots[self.index]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() || state.leased == 0 || !buffer.credit.release() {
            self.shared.fault.store(true, Ordering::Release);
            return;
        }
        state.leased -= 1;
        *slot = Some(buffer);
    }
}
impl Queue {
    pub fn new(max_bytes: usize, max_record: usize) -> Result<Self, BuildError> {
        let count = slots(max_bytes, max_record)?;
        let mut cells = Vec::new();
        let mut ready = VecDeque::new();
        cells
            .try_reserve_exact(count)
            .map_err(|_| BuildError::Allocation)?;
        ready
            .try_reserve_exact(count)
            .map_err(|_| BuildError::Allocation)?;
        let mut payload = 0usize;
        for id in 0..count {
            let buffer = Buffer::new(id, max_record)?;
            payload = payload
                .checked_add(buffer.bytes.capacity())
                .ok_or(BuildError::CapacityOverflow)?;
            cells.push(Mutex::new(Some(buffer)));
        }
        if payload > max_bytes {
            return Err(BuildError::Allocation);
        }
        let metadata = checked_bytes(cells.capacity(), size_of::<Mutex<Option<Buffer>>>())?
            .checked_add(checked_bytes(ready.capacity(), size_of::<(usize, u64)>())?)
            .ok_or(BuildError::CapacityOverflow)?;
        let storage = storage(count, max_record, payload, metadata, size_of::<Shared>())?;
        Ok(Self {
            shared: Arc::new(Shared {
                slots: cells,
                state: Mutex::new(State {
                    ready,
                    order: Fifo::empty(),
                    closed: false,
                    rejected: 0,
                    leased: 0,
                }),
                fault: AtomicBool::new(false),
                probe: AtomicUsize::new(0),
                storage,
            }),
        })
    }
    pub fn storage(&self) -> Storage {
        self.shared.storage
    }
    pub fn rejected(&self) -> Result<u64, QueueError> {
        let state = self.shared.state.lock().map_err(|_| QueueError::Poisoned)?;
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(QueueError::Poisoned);
        }
        Ok(state.rejected)
    }
    fn reject(&self, reason: Admission) -> Admission {
        let mut state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Admission::Contended,
            Err(TryLockError::Poisoned(_)) => return Admission::Poisoned,
        };
        if self.shared.fault.load(Ordering::Acquire) {
            return Admission::Poisoned;
        }
        if state.closed {
            return Admission::Closed;
        }
        if reason == Admission::Full {
            let Some(charged) = state.ready.len().checked_add(state.leased) else {
                self.shared.fault.store(true, Ordering::Release);
                return Admission::Poisoned;
            };
            if charged > self.shared.slots.len() {
                self.shared.fault.store(true, Ordering::Release);
                return Admission::Poisoned;
            }
            // A scan is not a simultaneous observation. Ready entries and held
            // leases are counted together under this guard before reporting Full.
            if charged < self.shared.slots.len() {
                return Admission::Contended;
            }
        }
        state.rejected = state.rejected.saturating_add(1);
        reason
    }
    pub fn try_record(&self, observation: &Observation) -> Admission {
        self.record(observation, || {})
    }
    fn record(&self, observation: &Observation, after_reserve: impl FnOnce()) -> Admission {
        self.record_with_probe(observation, after_reserve, |_| {})
    }
    fn record_with_probe(
        &self,
        observation: &Observation,
        after_reserve: impl FnOnce(),
        mut before_probe: impl FnMut(usize),
    ) -> Admission {
        if self.shared.fault.load(Ordering::Acquire) {
            return Admission::Poisoned;
        }
        if observation.validate().is_err() {
            return self.reject(Admission::Invalid);
        }
        let count = self.shared.slots.len();
        let start = self.shared.probe.fetch_add(1, Ordering::Relaxed) % count;
        let mut after_reserve = Some(after_reserve);
        let mut contended = false;
        for offset in 0..count {
            // Avoid start+offset overflow even for a hypothetical huge slot array.
            let index = if offset < count - start {
                start + offset
            } else {
                offset - (count - start)
            };
            before_probe(index);
            let mut slot = match self.shared.slots[index].try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => {
                    contended = true;
                    continue;
                }
                Err(TryLockError::Poisoned(_)) => {
                    self.shared.fault.store(true, Ordering::Release);
                    return Admission::Poisoned;
                }
            };
            let Some(buffer) = slot.as_mut() else {
                continue;
            };
            if !buffer.credit.reserve() {
                continue;
            }
            let _panic = FaultOnPanic(&self.shared.fault);
            let buffer = slot.take().expect("reserved slot has a buffer");
            let mut reservation =
                Reservation::new(buffer, |buffer| *slot = Some(buffer), &self.shared.fault);
            after_reserve.take().expect("one reservation")();
            let len = match encode(reservation.buffer(), observation) {
                Ok(len) => len,
                Err(reason) => {
                    return self.reject(reason);
                }
            };
            let mut state = match self.shared.state.try_lock() {
                Ok(state) => state,
                Err(error) => {
                    return match error {
                        TryLockError::WouldBlock => Admission::Contended,
                        TryLockError::Poisoned(_) => Admission::Poisoned,
                    };
                }
            };
            if self.shared.fault.load(Ordering::Acquire) || state.closed {
                return if self.shared.fault.load(Ordering::Acquire) {
                    Admission::Poisoned
                } else {
                    Admission::Closed
                };
            }
            let Some(ticket) = state.order.enqueue() else {
                return Admission::Exhausted;
            };
            if !reservation.buffer().credit.publish(len) {
                self.shared.fault.store(true, Ordering::Release);
                return Admission::Poisoned;
            }
            let buffer = reservation.take();
            drop(reservation);
            *slot = Some(buffer);
            // Global commit order, rather than reservation order, defines FIFO.
            state.ready.push_back((index, ticket));
            return Admission::Accepted;
        }
        if contended {
            Admission::Contended
        } else {
            self.reject(Admission::Full)
        }
    }
    pub fn try_dequeue(&self) -> Result<Option<Lease>, QueueError> {
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(QueueError::Poisoned);
        }
        let mut state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(QueueError::Contended),
            Err(TryLockError::Poisoned(_)) => return Err(QueueError::Poisoned),
        };
        let Some(&(index, ticket)) = state.ready.front() else {
            return Ok(None);
        };
        let mut slot = match self.shared.slots[index].try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Err(QueueError::Contended),
            Err(TryLockError::Poisoned(_)) => {
                self.shared.fault.store(true, Ordering::Release);
                return Err(QueueError::Poisoned);
            }
        };
        let _panic = FaultOnPanic(&self.shared.fault);
        if state.leased >= self.shared.slots.len() {
            self.shared.fault.store(true, Ordering::Release);
            return Err(QueueError::Poisoned);
        }
        let Some(mut buffer) = slot.take() else {
            self.shared.fault.store(true, Ordering::Release);
            return Err(QueueError::Poisoned);
        };
        if !state.order.dequeue(ticket) || buffer.credit.lease().is_none() {
            self.shared.fault.store(true, Ordering::Release);
            return Err(QueueError::Poisoned);
        }
        state.ready.pop_front();
        state.leased += 1;
        Ok(Some(Lease {
            shared: Arc::clone(&self.shared),
            buffer: Some(buffer),
            ticket,
            index,
        }))
    }
    pub fn shutdown(&self) -> Result<(), QueueError> {
        let mut state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(QueueError::Contended),
            Err(TryLockError::Poisoned(_)) => return Err(QueueError::Poisoned),
        };
        if self.shared.fault.load(Ordering::Acquire) {
            return Err(QueueError::Poisoned);
        }
        state.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod interleavings {
    use super::*;
    use crate::admission::tests::sample;
    use std::{sync::mpsc, time::Duration};
    #[test]
    fn moving_free_storage_must_not_be_reported_as_full() {
        let q = Queue::new(2048, 1024).unwrap();
        assert_eq!(q.try_record(&sample()), Admission::Accepted);
        assert_eq!(q.try_record(&sample()), Admission::Accepted);
        let mut held = Some(q.try_dequeue().unwrap().unwrap());
        drop(q.try_dequeue().unwrap().unwrap());
        // One slot is always free during this attempt. Its location changes
        // after the first probe, a valid interleaving of public operations.
        let result = q.record_with_probe(
            &sample(),
            || {},
            |index| {
                if index == 1 {
                    drop(held.take());
                    assert_eq!(q.try_record(&sample()), Admission::Accepted);
                }
            },
        );
        assert_eq!(result, Admission::Contended);
        assert_eq!(q.rejected().unwrap(), 0);
        assert_eq!(q.try_dequeue().unwrap().unwrap().sequence(), 2);
    }
    #[test]
    fn encoding_is_parallel_and_fifo_follows_commit_not_reservation() {
        let q = Queue::new(2048, 1024).unwrap();
        let (entered, observed) = mpsc::sync_channel(0);
        let (release, resume) = mpsc::sync_channel(0);
        let worker = q.clone();
        let producer = std::thread::spawn(move || {
            let mut a = sample();
            a.id = "first-reservation".into();
            worker.record(&a, || {
                entered.send(()).unwrap();
                resume.recv().unwrap();
            })
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut b = sample();
        b.id = "first-commit".into();
        assert_eq!(q.try_record(&b), Admission::Accepted);
        let first = q.try_dequeue().unwrap().unwrap();
        assert_eq!(first.sequence(), 0);
        assert_eq!(
            serde_json::from_slice::<Observation>(&first).unwrap().id,
            "first-commit"
        );
        assert_eq!(q.try_record(&sample()), Admission::Contended);
        release.send(()).unwrap();
        assert_eq!(producer.join().unwrap(), Admission::Accepted);
        let second = q.try_dequeue().unwrap().unwrap();
        assert_eq!(second.sequence(), 1);
        assert_eq!(
            serde_json::from_slice::<Observation>(&second).unwrap().id,
            "first-reservation"
        );
        assert_eq!(q.try_record(&sample()), Admission::Full);
        drop(first);
        assert_eq!(q.try_record(&sample()), Admission::Accepted);
    }
    #[test]
    fn shutdown_rolls_back_encoding_that_has_not_committed() {
        let q = Queue::new(2048, 1024).unwrap();
        let (entered, observed) = mpsc::sync_channel(0);
        let (release, resume) = mpsc::sync_channel(0);
        let worker = q.clone();
        let producer = std::thread::spawn(move || {
            worker.record(&sample(), || {
                entered.send(()).unwrap();
                resume.recv().unwrap();
            })
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        q.shutdown().unwrap();
        release.send(()).unwrap();
        assert_eq!(producer.join().unwrap(), Admission::Closed);
        assert!(q.try_dequeue().unwrap().is_none());
    }
    #[test]
    fn panic_is_terminal_but_outstanding_lease_stays_valid() {
        let q = Queue::new(2048, 1024).unwrap();
        let reference = serde_json::to_vec(&sample()).unwrap();
        assert_eq!(q.try_record(&sample()), Admission::Accepted);
        let lease = q.try_dequeue().unwrap().unwrap();
        let worker = q.clone();
        assert!(
            std::thread::spawn(
                move || worker.record(&sample(), || panic!("injected encoding panic"))
            )
            .join()
            .is_err()
        );
        assert_eq!(q.try_record(&sample()), Admission::Poisoned);
        assert!(matches!(q.try_dequeue(), Err(QueueError::Poisoned)));
        assert_eq!(q.shutdown(), Err(QueueError::Poisoned));
        assert_eq!(q.rejected(), Err(QueueError::Poisoned));
        for slot in &q.shared.slots {
            let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(buffer) = slot.as_mut() {
                assert!(
                    buffer.credit.reserve(),
                    "encoder unwind must restore free credit"
                );
                assert!(buffer.credit.cancel());
            }
        }
        assert_eq!(&*lease, reference);
        drop(lease);
        assert_eq!(q.try_record(&sample()), Admission::Poisoned);
    }
    #[test]
    fn retained_collection_and_payload_capacities_do_not_grow() {
        let q = Queue::new(4096, 1024).unwrap();
        let capacities = (
            q.shared.slots.capacity(),
            q.shared.state.lock().unwrap().ready.capacity(),
        );
        for _ in 0..1000 {
            assert_eq!(q.try_record(&sample()), Admission::Accepted);
            drop(q.try_dequeue().unwrap().unwrap());
        }
        assert_eq!(
            (
                q.shared.slots.capacity(),
                q.shared.state.lock().unwrap().ready.capacity()
            ),
            capacities
        );
        for (id, slot) in q.shared.slots.iter().enumerate() {
            let slot = slot.lock().unwrap();
            let b = slot.as_ref().unwrap();
            assert_eq!(b.credit.id(), id);
            assert_eq!(b.bytes.capacity(), 1024);
        }
        assert!(q.shared.state.lock().unwrap().ready.is_empty());
    }
}
