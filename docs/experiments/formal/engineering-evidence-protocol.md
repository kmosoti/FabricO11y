# Engineering evidence preflight contract

This prospective Rust implementation supersedes the unpublished Python aid;
its historical commands/results remain on the data drive and in Git. The owner
chose incremental migration while registered alpha acceptance continues.
Existing package, custody, query, browser and soak graders remain authoritative.
No product oracle, expected acceptance outcome or release gate changes.

## Interface and trust boundary

`cargo xtask evidence-preflight --root ROOT --plan RELATIVE_PATH
--plan-sha256 SHA256 --candidate COMMIT` is read-only and prints JSON.
The caller pins the complete version-1 plan and separately expected candidate.
Exit0 means all *listed* evidence structurally matches; exit3 means pending work;
exit2 refuses invalid input/evidence. None grants acceptance or authenticates a
producer ([ADR-0018](../../decisions/ADR-0018-accept-work-on-executable-evidence.md)).
An omitted required case cannot be detected from an incomplete caller inventory.

Root is existing, absolute, canonical and has no symlink components. It must
match explicit `FABRIC_STORAGE_ROOT`; no storage fallback. The CLI checks actual
cgroup-v2 memory.max positive and at most20GiB, memory.swap.max=0. Every command
still uses the required resource launcher, serialized source driver, disk-backed
DATA scratch, full95GB admission census/reservation and actual stopped receipts.

## Version-1 schema

Plan keys are exactly `{version:1,candidate,artifacts,jobs}`. Commits are40lowercase
hex characters. IDs begin with a lowercase letter and contain1–80ASCII lowercase
letters/digits/hyphens; SHA-256 values are64lowercase hex characters.

Artifacts are `{id,path,sha256}`. Paths are nonempty relative POSIX paths without
empty, dot or parent components. Duplicate IDs/paths and conflicting pins fail.
Hash every listed artifact, including unreferenced inputs.

Jobs are `{id,needs,command,harness_revision,expected_exit,evidence}`:

- `needs`: unique existing IDs; no self edges or cycles.
- `command`:1–256nonempty string arguments, at most1MiB UTF-8 per argument;
  compare literally, never execute. `harness_revision` pins the exact job source.
- `expected_exit`: integer0–255, never boolean. A nonzero join cannot establish
  the reason for a refusal; the original named grader remains required.
- `evidence`: null or `{wrapper,resource}` referencing two distinct artifacts.
  Receipt references and resource units must be unique across jobs, including
  copies stored under different paths. Unknown plan/schema keys fail.

Wrapper JSON requires completed state, exact command/harness revision, empty
`source_status_before`/`source_status_after`, and integer exit/launcher_exit equal
to expected_exit. Its sole resource_receipts entry must pin the declared resource
path (relative or absolute under root), digest, integer exit and literal true stop.
Resource JSON requires exact command/storage_root, integer exit/wrapper_exit,
literal true stop_confirmed, present null stop_error and fabric-work-[0-9a-f]+unit.
Its completed command_receipt must match command/unit/integer exit; optional
child revision must match the source pin. Its canonical cgroup is under
/sys/fs/cgroup and ends in that unit.service, or that service's direct /workload
child only when resource.delegated is literally true. Extra receipt metadata
has no implied semantics. Resource limits/actual execution are not recertified.

Without evidence a job is pending when all prerequisites match, otherwise blocked.
Evidence with an unmet prerequisite is invalid. Output follows lexicographic
order among currently ready nodes.

## Primitives and bounds

Use the existing tooling-layer xtask crate and locked serde/serde_json/sha2/libc;
no product/core dependency change. Separate strict JSON, owned file descriptors,
indexed graph traversal and receipt composition. Reject duplicate JSON keys,
non-finite numbers and depth above32; max4MiB/document,64MiB aggregate retained
JSON payload,4,096artifacts/jobs and65,536edges. Stream in1MiB chunks with8GiB/file
and32GiB total read bytes (including the plan). Keep bounded JSON payloads only.

Resolve each component relative to held no-follow directory descriptors; refuse
symlinks/nonregular files. Parse the bytes actually hashed. Compare device/inode,
size and nanosecond mtime/ctime before/after reads and recheck paths at composition
end. One hash per path per invocation; no persistent/mtime-only digest cache.
These checks detect exercised races, not privileged hostile writers or dishonest
producers. Indexed adjacency/indegrees and ordered ready selection require
O((V+E)logV) time/O(V+E) state; no speculative generic orchestration framework.

Before changing the file-reader mechanism, inject a real append after its first
descriptor read of an eight-byte fixture. It must refuse the changed file and
consume no bytes beyond the extent admitted by the initial metadata. Record the
old implementation's actual counter before introducing a bounded-read loop;
repeat the same fixture afterward. This distinguishes final identity rejection
from bounded I/O while a file grows. Whole-composition tests remain required.

## Verification and finite experiment

Retain literal positives and repinned negative fixtures for candidate/hash/source,
missing/copied receipts, argv/exit/unit/cgroup/stop joins, boolean/type faults,
unknown/duplicate schema keys/IDs/edges, unmet dependencies/cycles, traversal,
symlinks/nonregular files, oversized/deep JSON and replacement/in-place races.
Compare ordering/cycle rejection with independent permutation enumeration of all
4,165directed graphs on1–4vertices. Test byte-budget arithmetic without reading
32GiB; actual sparse8GiB+1 metadata refusal needs a full10GiB reservation.

For native measurements use seed17,64/512/2,048vertices, chain/fan-out graphs,
seven alternating pairs after one warmup. Compare indexed traversal against
repeated scans with competent hash-set membership and identical outputs. Compare
32streaming hashes of one8MiB fixture with one hash shared across32joins. Charge
fixture creation outside timing; report monotonic walltime medians, exact byte
counts/output equality and whole-command cgroup memory. Use Rust1.99.0 release
builds, two build jobs, original launcher caps. No Python/Rust speed ranking or
allocation comparison follows. Reject optimization if equality/negative controls
fail, and report regressions rather than weakening expectations.

Register exact source/commands before running. Preserve failures/raw records on
DATA, publish bounded results in the research wiki, and keep Rust workspace
controls in the existing required fast test gate. No duplicate Python runner or
new MCP server is needed.
