#![forbid(unsafe_code)]
//! Checked arithmetic shared by the executable adapter and proof checkers.
use vstd::prelude::*;

verus! {
pub fn reserve(capacity: usize, used: usize, charge: usize) -> (result: Option<usize>)
    ensures
        match result {
            Some(next) => used <= capacity && charge <= capacity - used
                && next as int == used as int + charge as int && next <= capacity,
            None => used > capacity || charge > capacity - used,
        }
{
    if used <= capacity && charge <= capacity - used {
        Some(used + charge)
    } else {
        None
    }
}
}

#[cfg(test)]
mod tests {
    #[test]
    fn reservation_matches_wider_independent_arithmetic() {
        let values = [0, 1, 2, 7, usize::MAX / 2, usize::MAX - 1, usize::MAX];
        for capacity in values {
            for used in values {
                for charge in values {
                    let sum = used as u128 + charge as u128;
                    let expected = if used <= capacity && sum <= capacity as u128 {
                        Some(sum as usize)
                    } else {
                        None
                    };
                    assert_eq!(super::reserve(capacity, used, charge), expected);
                }
            }
        }
    }
}

verus! {
#[derive(Clone, Copy)]
pub enum Phase { Free, Reserved, Ready, Leased }

/// Admission state for one payload buffer.
///
/// The synchronization adapter binds each credit to its payload storage.
/// ```
/// let mut slot = admission_kernel::Slot::new(0, 16);
/// assert!(slot.reserve());
/// assert!(slot.publish(8));
/// assert_eq!(slot.lease(), Some(8));
/// assert!(slot.release());
/// ```
/// ```compile_fail
/// let slot = admission_kernel::Slot::new(0, 16);
/// let duplicate = slot.clone();
/// ```
pub struct Slot {
    id: usize,
    capacity: usize,
    len: usize,
    phase: Phase,
}

impl Slot {
    pub closed spec fn identity(&self) -> usize { self.id }
    pub closed spec fn limit(&self) -> usize { self.capacity }
    pub closed spec fn length(&self) -> usize { self.len }
    pub closed spec fn phase(&self) -> Phase { self.phase }
    pub closed spec fn valid(&self) -> bool {
        self.len <= self.capacity
        && ((self.phase is Free || self.phase is Reserved) ==> self.len == 0)
    }

    pub closed spec fn ownership(&self) -> (int, int, int, int) {
        match self.phase {
            Phase::Free => (1, 0, 0, 0),
            Phase::Reserved => (0, 1, 0, 0),
            Phase::Ready => (0, 0, 1, 0),
            Phase::Leased => (0, 0, 0, 1),
        }
    }

    pub proof fn ownership_conservation(&self)
        ensures self.ownership().0 + self.ownership().1 + self.ownership().2 + self.ownership().3 == 1,
    {}

    pub fn new(id: usize, capacity: usize) -> (result: Self)
        ensures result.valid(), result.identity() == id, result.limit() == capacity,
            result.phase() is Free, result.length() == 0,
    {
        Self { id, capacity, len: 0, phase: Phase::Free }
    }

    pub fn id(&self) -> (result: usize) ensures result == self.identity() { self.id }
    pub fn capacity(&self) -> (result: usize) ensures result == self.limit() { self.capacity }
    pub fn encoded_len(&self) -> (result: usize) ensures result == self.length() { self.len }

    pub fn reserve(&mut self) -> (result: bool)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).identity() == old(self).identity(), final(self).limit() == old(self).limit(),
            result == (old(self).phase() is Free),
            result ==> final(self).phase() is Reserved,
            !result ==> *final(self) == *old(self),
    {
        if matches!(self.phase, Phase::Free) {
            self.phase = Phase::Reserved;
            true
        } else { false }
    }

    pub fn publish(&mut self, len: usize) -> (result: bool)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).identity() == old(self).identity(), final(self).limit() == old(self).limit(),
            result == ((old(self).phase() is Reserved) && len <= old(self).limit()),
            result ==> final(self).phase() is Ready && final(self).length() == len,
            !result ==> *final(self) == *old(self),
    {
        if matches!(self.phase, Phase::Reserved) && len <= self.capacity {
            self.len = len;
            self.phase = Phase::Ready;
            true
        } else { false }
    }

    pub fn lease(&mut self) -> (result: Option<usize>)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).identity() == old(self).identity(), final(self).limit() == old(self).limit(),
            result.is_some() == (old(self).phase() is Ready),
            result.is_some() ==> final(self).phase() is Leased && result.unwrap() == old(self).length(),
            final(self).length() == old(self).length(),
            result.is_none() ==> *final(self) == *old(self),
    {
        if matches!(self.phase, Phase::Ready) {
            self.phase = Phase::Leased;
            Some(self.len)
        } else { None }
    }

    pub fn cancel(&mut self) -> (result: bool)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).identity() == old(self).identity(), final(self).limit() == old(self).limit(),
            result == (old(self).phase() is Reserved),
            result ==> final(self).phase() is Free && final(self).length() == 0,
            !result ==> *final(self) == *old(self),
    {
        if matches!(self.phase, Phase::Reserved) {
            self.phase = Phase::Free;
            self.len = 0;
            true
        } else { false }
    }

    pub fn release(&mut self) -> (result: bool)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).identity() == old(self).identity(), final(self).limit() == old(self).limit(),
            result == (old(self).phase() is Leased),
            result ==> final(self).phase() is Free && final(self).length() == 0,
            !result ==> *final(self) == *old(self),
    {
        if matches!(self.phase, Phase::Leased) {
            self.phase = Phase::Free;
            self.len = 0;
            true
        } else { false }
    }
}

/// Successful enqueue tickets and the next permitted dequeue ticket.
/// Exhaustion refuses admission instead of wrapping an ordering identity.
pub struct Fifo {
    head: u64,
    next: u64,
}
impl Fifo {
    pub closed spec fn first(&self) -> u64 { self.head }
    pub closed spec fn end(&self) -> u64 { self.next }
    pub closed spec fn valid(&self) -> bool { self.head <= self.next }

    pub fn empty() -> (result: Self)
        ensures result.valid(), result.first() == 0, result.end() == 0,
    { Self { head: 0, next: 0 } }

    pub fn enqueue(&mut self) -> (result: Option<u64>)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).first() == old(self).first(),
            result.is_some() == (old(self).end() < u64::MAX),
            result.is_some() ==> result.unwrap() == old(self).end() && final(self).end() as int == old(self).end() as int + 1,
            result.is_none() ==> *final(self) == *old(self),
    {
        if self.next == u64::MAX { None }
        else {
            let ticket = self.next;
            self.next += 1;
            Some(ticket)
        }
    }

    pub fn dequeue(&mut self, ticket: u64) -> (result: bool)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).end() == old(self).end(),
            result == (old(self).first() < old(self).end() && ticket == old(self).first()),
            result ==> final(self).first() as int == old(self).first() as int + 1,
            !result ==> *final(self) == *old(self),
    {
        if self.head < self.next && ticket == self.head {
            self.head += 1;
            true
        } else { false }
    }
}
}

#[cfg(test)]
mod protocol_tests {
    use super::{Fifo, Slot};
    #[test]
    fn one_credit_survives_rejected_transitions_and_reuse() {
        for limit in [0, 8, usize::MAX] {
            let mut slot = Slot::new(7, limit);
            assert!(slot.reserve());
            assert!(!slot.reserve());
            assert!(!slot.release());
            if limit < usize::MAX {
                assert!(!slot.publish(limit + 1));
            }
            assert!(slot.publish(limit));
            assert!(!slot.cancel());
            assert_eq!(slot.lease(), Some(limit));
            assert_eq!(slot.lease(), None);
            assert!(!slot.reserve());
            assert!(slot.release());
            assert!(!slot.release());
            assert_eq!(slot.encoded_len(), 0);
            assert_eq!(slot.id(), 7);
            assert_eq!(slot.capacity(), limit);
            assert!(slot.reserve());
            assert!(slot.cancel());
            assert!(slot.reserve());
        }
    }

    #[test]
    fn fifo_refuses_reordering_replay_and_integer_exhaustion() {
        let mut fifo = Fifo::empty();
        assert_eq!(fifo.enqueue(), Some(0));
        assert_eq!(fifo.enqueue(), Some(1));
        assert!(!fifo.dequeue(1));
        assert!(fifo.dequeue(0));
        assert!(!fifo.dequeue(0));
        assert!(fifo.dequeue(1));
        assert!(!fifo.dequeue(2));
        let mut exhausted = Fifo {
            head: u64::MAX,
            next: u64::MAX,
        };
        assert_eq!(exhausted.enqueue(), None);
        assert!(!exhausted.dequeue(u64::MAX));
        assert_eq!(exhausted.head, u64::MAX);
        assert_eq!(exhausted.next, u64::MAX);
    }

    #[test]
    fn exhaustion_preserves_ready_records_and_outstanding_credits() {
        let mut order = Fifo {
            head: u64::MAX - 2,
            next: u64::MAX - 1,
        };
        let mut ready = Slot::new(0, 8);
        assert!(ready.reserve());
        assert!(ready.publish(5));
        let mut held = Slot::new(1, 8);
        assert!(held.reserve());
        assert!(held.publish(3));
        assert_eq!(held.lease(), Some(3));
        let mut last = Slot::new(2, 8);
        assert!(last.reserve());
        assert_eq!(order.enqueue(), Some(u64::MAX - 1));
        assert!(last.publish(6));
        let mut refused = Slot::new(3, 8);
        assert!(refused.reserve());
        assert_eq!(order.enqueue(), None);
        assert!(refused.cancel());
        assert!(order.dequeue(u64::MAX - 2));
        assert_eq!(ready.lease(), Some(5));
        assert!(order.dequeue(u64::MAX - 1));
        assert_eq!(last.lease(), Some(6));
        assert!(held.release());
        assert!(ready.release());
        assert!(last.release());
        assert_eq!(order.enqueue(), None);
    }
}
