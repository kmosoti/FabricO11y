# Spindle local collection contract, revision 1

Spindle runs as a Fedora user service, accepts bounded length-framed v2 records
on a private Unix socket and validates peer UID. It explicitly configures text or
JSONL file sources, accessible systemd journal sources and host sampling. File
collection tracks stable file identity and byte offsets, partial lines, rename
rotation and truncation. Journal collection tracks native cursors and reports
missing cursors and access gaps. New file and journal sources start at the current
end unless replay is requested. Spindle journal messages are excluded by default.
Host CPU, memory, disk, network and filesystem capacity samples identify resets
and collection gaps.

Custody returns a receipt only after durable atomic publication of the record,
retry identity and source checkpoint. Initial file and directory creation must
survive a crash. Source checkpoints advance only with custody or a durable explicit
gap/rejection record. Lost acknowledgements and timeouts are unknown outcomes;
clients retry unchanged identities. Same identity and bytes succeeds; changed
bytes conflicts. Registry descriptors remain available with stored observations.

Storage, frame sizes, decode nesting, connections, work per poll and buffers are
bounded. Storage quotas include database/log overhead and recovery reserve. Full
storage refuses SDK submissions, pauses replayable sources without cursor advance,
and reports missed sampling explicitly. No automatic eviction or forwarding is
permitted. Self status remains readable at full storage. Health exposes ingestion,
refusals, errors, lag, retries, queue/custody/latency/resource measurements without
feeding its collection into itself. Status, read and export interfaces are local.

Before selection, a transaction engine and framed append-log candidate face the
same frozen correctness, quota, retry and crash-window fixtures. Failed candidates
cannot enter performance ranking. Independent acceptance covers source restart and
readback, commit/ack fault windows, malformed/large/deep frames, peer validation,
identity conflicts, quotas, slow clients, shutdown, missing cursors, file
rotation/truncation/permissions, metric resets and collection feedback. Measurement
reports latency, throughput, memory and recovery separately, with development and
held-out workloads. Unavailable host capabilities remain explicit uncertainty.
