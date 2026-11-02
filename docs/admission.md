# Concurrent admission

`fabric::admission::Queue` is a cloneable, in-memory MPMC queue. Construction
returns `Result<Queue, BuildError>` and reserves `floor(max_bytes / max_record)`
fixed buffers. Zero record limits and configurations with no slot are rejected.
Each slot reserves `max_record` bytes even for short records; unused remainder
bytes do not create another slot.

```rust
use fabric::admission::{Admission, Queue};
# use fabric::collection::Observation;
# fn submit(observation: &Observation) -> Result<(), Box<dyn std::error::Error>> {
let queue = Queue::new(8192, 2048)?;
match queue.try_record(observation) {
    Admission::Accepted => {
        if let Some(frame) = queue.try_dequeue()? {
            // Process &frame while its buffer remains charged to this queue.
            drop(frame); // Returns capacity to the queue.
        }
    }
    _ => { /* Apply the caller's refusal or retry policy. */ }
}
# Ok(())
# }
```

Admission reserves one slot before writing JSON into its fixed slice. Independent
producers encode concurrently, then publish under a global FIFO commit lock.
Successful commit order determines sequence numbers and dequeue order; producer
start order and overlapping return order need not match it. Consumers can finish
processing leases in a different order. A lease can outlive all queue handles.

`try_record` makes at most one try-lock attempt per slot and one on global state;
it never waits for a mutex. This bounds synchronization attempts, not elapsed
time or serialization work for caller-owned input. `try_dequeue` also uses
try-locks. `Contended` permits caller-controlled retry and can occur even when
capacity exists. `Full` requires a simultaneous accounting snapshot with every
slot ready or leased. Reserved work may instead produce `Contended`.

`Invalid`, `TooLarge` and `Full` increment the saturating rejection count.
`Contended`, `Closed`, `Poisoned` and `Exhausted` are separate outcomes and do not
increment it. Errors restore reservations. Sequence exhaustion refuses further
publication rather than wrapping. `shutdown()` closes publication while allowing
ready records to drain; contention is returned explicitly. `rejected()` and
lease destruction may wait for a mutex. A detected internal panic or poisoned mutex is terminal; operations
overlapping the fault can finish before observing it. Caller panic while holding
a lease still returns its buffer during unwind.

`storage()` reports actual reserved payload and collection capacities, fixed
control bytes and control allocation count. Buffers and collections never grow
after construction. Allocator bookkeeping, Arc's private header, thread stacks,
transient error objects and caller-owned observations are excluded. Standard
Arc allocation retains Rust's allocation-failure behavior. The limit therefore
does not bound total process memory. Admission is not a durability receipt;
the local file writer has its own synchronization and failure contract.

The [kernel proof scope](../proofs/README.md) and
[admission contract](../verification/admission-contract.md) distinguish checked
state transitions from tested synchronization behavior.
