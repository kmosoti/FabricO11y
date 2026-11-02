# Concurrent admission contract

The queue accepts multiple producers and consumers through shared references.
Construction is fallible. A configuration specifies SDK-owned payload capacity
and a maximum encoded record size. Construction reserves all payload buffers and
queue metadata before the queue can accept records. Allocation failure and
invalid configurations are explicit errors.

An admission attempt makes a bounded number of synchronization attempts. It
returns Accepted, Invalid, TooLarge, Full, Contended, Closed or Poisoned.
Contended and Full permit callers to retry; neither promises eventual admission.
A capacity reservation precedes encoding. Failed encoding restores the reservation
and does not enqueue a partial record. Invalid observations never enter the queue.
No producer waits for another producer to finish encoding.

Successful enqueue linearization is the insertion into the ready sequence under
synchronization. Consumers receive records in this order. Dequeue linearization
removes its first record. Concurrent consumers may finish processing in another
order. A consumer lease retains its payload charge until release or destruction.
Leases cannot be cloned. Releasing a lease makes its storage reusable exactly once.

Shutdown prevents new admissions and permits draining already accepted records.
Outstanding leases remain valid after shutdown and after queue handles are dropped.
Poisoning is terminal for normal operations and yields an explicit error. Cleanup
may recover poisoned state solely to return owned resources; recovery does not
resume admissions or clear poison.

The memory guarantee covers preallocated SDK payload buffers, free/ready metadata
and queue synchronization state. It excludes caller observations, caller-owned
strings/vectors, the allocator's metadata/fragmentation, thread stacks, runtime
and process memory. Fixed-size slots may refuse a record when all slots are in
use even if unused bytes remain within occupied slots. Report actual reserved
storage and slot count; do not describe this as an exact process-memory cap.

Acceptance requires ownership conservation over free, reserved, ready and leased
states, bounded arithmetic, rollback on every refusal, and FIFO at the adapter's
synchronization boundary. Arithmetic proofs alone do not establish adapter
correctness. Controlled interleavings, MPMC histories, stress and reference-model
sequence tests supplement the native proof paths.

Candidate families to evaluate include a single preallocated mutex pool and
batched publication with a globally ordered commit step. Producer-local queues
without a global successful-enqueue ordering mechanism violate the contract.
Queue selection follows the task-suite pilot; this brief does not select a family.
