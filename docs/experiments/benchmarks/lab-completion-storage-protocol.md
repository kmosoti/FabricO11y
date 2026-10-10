Registered by the inline coordinator before execution under [campaign scope](lab-completion-protocol.md). Preparation wording below records the PI handoff; commands have not yet run.

# Storage correctness fixture proposal

Prepared before execution; no test result is claimed. Run the new integration
target serially through the resource launcher after registration/review:
`cargo test --offline --locked -p fabric-server --test completion_storage -- --test-threads=1`.
Every scratch directory is fresh beneath `FABRIC_SCRATCH_ROOT`; failed tests
retain the directory, input ledger, query/pages, oracle output and fault origin.

Q2b fixtures construct their expected Batch/receipt ledger **before** building
Segments and never obtain expected rows from Segment enumeration or replay.
Full chains are graded by the unchanged Python query oracle, for Scan and Walk,
including fresh History instances to exercise rebuilding derived state. Age
eviction uses deliberately ancient and current receive timestamps, a huge byte
ceiling and the real sealer retention pass; expect only ancient Segment removal,
an independently partitioned retained ledger and Gone on the old page token.
No age boundary timing or default 24-hour envelope is inferred.

Deleted and unauthentic optional log/span filters must preserve exact projected
answers. A missing `batches.parquet` is missing raw custody data: replay must
error, while unaffected logs/spans remain exactly queryable. This is distinct
from a deleted `logs.parquet`/`spans.parquet` or whole Segment, whose unavailable
semantics need their own declared ledger and fixture. Existing corrupt-log and
byte-retention tests remain unchanged. Negative controls deliberately remove a
returned row and require the unchanged oracle to reject it.

R1 owned-child kill fixture runs the actual bounded builder in a subprocess,
waits for a marker **after publication and before any reclaim**, kills only that
owned child, reopens Store, invokes the actual sealer pass, then requires exact
pre-kill custody and independently graded answers and journal reclaim. This is
one real process cut with synced journal assumptions; not arbitrary stage,
physical power-loss or live ACK timing evidence.

The public builder has no injected read/write/sync ports. A missing input and a
blocked output-directory path are genuine scoped filesystem failures; inputs
must survive, temporary builds must not leak, and retry after removing the
obstruction must produce exact answers. They do not cover partial merge reads,
partial writes, ENOSPC, filter/manifest fsync, publication-rename failure,
checkpoint/reclaim sync or kill during an unfinished spill/merge. Those remain
explicit missing R1 evidence, requiring private syscall injection or an owned
limited filesystem and deterministic cut-point barriers before admission.

Feasible next private fixture: adapt the existing
`tools/storage-probe/tests/io_fault_support/partial_read*.c` LD_PRELOAD pattern.
Match canonical `/proc/self/fd/N` paths strictly beneath the owned scratch root;
freeze operation, suffix, occurrence and errno, then inject only in the builder
or sealer subprocess. Targets include `.run-` reads/writes, logs/metrics/spans
Parquet, raw Batches, optional filters, manifest and stream checkpoint writes,
and `fsync`/`fdatasync`. An injected ENOSPC is preferable to filling shared disk.
Interpose `write`/`writev` and `read`/`pread64` as needed, plus sync calls; validate
actual libc symbols with a tiny owned control and require a unique injection
receipt (path, operation, occurrence, errno). No matching syscall is **not run**,
not a successful fault result. First return a genuine short write/read, then
EIO for the next same-target call to expose partial-progress handling. Check
input digest/identity custody, exact restart answers, no unsafe temporary leaks
and a clean retry with the interposer disabled. Avoid intercepting graders,
ledger/receipt IO or unrelated system files. Root builds the private `.so` under
containment using `gcc -shared -fPIC ... -ldl`; no compiler/interposer ran here.
