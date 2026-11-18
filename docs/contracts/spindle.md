# Spindle collection and handoff contract

Spindle runs as a Fedora user service, accepts bounded length-framed v2 records
or context-bearing v3 records on a private Unix socket and validates peer UID. It explicitly configures text or
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
and reports missed sampling explicitly. Explicitly configured canonical forwarding
confirms canonical custody after a matching durable contiguous acknowledgement.
Payload reclamation additionally requires explicit opt-in after backfill and shadow
comparison. Self status remains readable at full storage. Health exposes ingestion,
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


An optional `forwarding` configuration names a loopback TCP endpoint, a private
owned credential file and a fixed canonical authority. `reclaim_confirmed` defaults
to `false`: shadow forwarding retains every local payload while recording the
canonical confirmed prefix. Operators enable it only after verifying backfill and
shadow comparison. Each custody directory
has a persistent random origin and monotonic record IDs; deleting acknowledged
payloads never resets allocation. Forwarding runs outside the collection poll
with at most 512 records and 256 KiB of decoded raw payload per batch. A receipt
must match the configured authority, origin and offered prefix before atomic
payload reclamation when opt-in is enabled. Confirmation and reclamation have
separate persistent monotonic watermarks. Enabling reclamation after restart also
reclaims previously confirmed shadow data in batches of at most 32 records, without
requiring new observations. Reclamation of the confirmed prefix continues while a
later forwarding request waits; unconfirmed payloads remain local. Lost, stale or
wrong receipts cannot discharge custody.

Reclaimed reads and retries resolve original bytes through the canonical receiver.
Unavailable canonical data produces an explicit error; it never becomes an empty
page. Remote resolution runs in bounded connection workers so a stalled authority
does not block independent local custody. Export captures the highest logical
record ID and emits original payload bytes through that frontier.

At most 128 generation metadata entries are retained. V3 sequences are ordered;
compact generation frontiers survive until expiry and complete forwarding.
Expiry verification does not depend on that metadata. V2 retry metadata is never
expired and remains subject to the storage quota. Database free pages are reused;
full admission resumes after successful reclamation. The local quota includes
rollback reserve and metadata as well as payloads. Canonical retention is not
automatically expired.


Shadow reads and exports continue to use local bytes. After reclamation, disabling
forwarding or changing its bound authority is refused because canonical access is
required to resolve deleted payloads. Setting `reclaim_confirmed` back to `false`
stops further reclamation while preserving access to already reclaimed records.
Health distinguishes `canonical_through` from `reclaimed_through`.

### Service telemetry custody

Install a separate Spindle instance for service telemetry with a distinct
private socket, storage directory, and local quota. The existing config accepts
an instance with no host, file, or journal sources. Give its persistent origin
a dedicated credential if it forwards to Fabric Server. Keep reclamation in
shadow mode until verified, following the same process as workload origins.
Count the telemetry origin against the receiver's origin limit. A dedicated
local spool isolates its local quota and source frontier. Configure a
`node_quota_bytes` entry for each telemetry origin on Fabric Server to bound
its retained canonical payload bytes. The combined caps may use at most a
quarter of the receiver's canonical control quota. New scoped admissions also
observe a separate control-storage headroom threshold. Keep capacity for
SQLite metadata, projection output and any independent engine storage. All
accepted canonical records retain their normal durability when the telemetry
quota is exhausted. Monitor local retry metadata, retained bytes and forwarding
lag, and manage capacity explicitly because canonical records do not expire
automatically.

Point each service exporter at the dedicated socket with
`FABRIC_SELF_TELEMETRY_SOCKET`. Leave the variable unset on the dedicated
Spindle instance. An exporter rejects its own serving socket. Configure the
socket in a private owned directory and confine instances under
`fabrico11y.slice`. Keep request and export telemetry for the telemetry origin
out of per-record instrumentation on that lane.

Each service instance uses one SDK delivery worker and reserves 32 slots of
4 KiB. Admission reports overflow or contention locally. Delivery uses a
250 ms socket deadline and retries unchanged bytes and identity after an
uncertain reply. Queued data lives in process memory until Spindle confirms
durable custody. After custody, forwarding and canonical projection retain
their existing separate progress boundaries.

Spindle groups synchronous requests already accepted in one bounded event-loop
iteration into a durable SQLite commit. Per-request savepoints isolate invalid
submissions. Receipts and lease replies are published after the outer commit
succeeds; a failed commit closes queued replies so clients retain unknown outcomes.
The commit-latency metric measures that durable batch. Idle and waiting iterations
sleep; successful admission can immediately progress committed replies.
