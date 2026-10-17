# Engineering evidence preflight protocol

## Rust implementation amendment

Owner requested incremental Rust migration on 2026-10-17. The existing version-1
data and refusal contracts below remain authoritative. Implement the aid inside
the existing tooling-layer `xtask` crate as `cargo xtask evidence-preflight`;
remove the newly introduced `tools/engineering` Python implementation, tests and
measurement runner after Rust equivalents are verified. The existing independent
Python delivery/query/rate oracles and required resource launcher remain unchanged.
Use existing locked `serde`, `serde_json`, `sha2` and `libc` versions; no product
or core dependency changes. Keep descriptor ownership and Linux no-follow lookup,
bounded JSON with duplicate rejection, numeric-indexed adjacency/indegrees and
deterministic ready selection as separable Rust primitives before composition.

The Rust CLI checks actual cgroup-v2 memory.max (positive and at most20GiB) and
memory.swap.max=0, and requires --root to match explicit FABRIC_STORAGE_ROOT.
No environment/storage fallback is allowed. Existing launcher job admission,
actual whole-service stop and storage enforcement remain required.

Repeat the original negative/positive fixture families and exhaustive4,165-graph
permutation reference against Rust; retain meaningful discrepancies. For measured
Rust costs, use the same seed17/sizes/shapes/32hash-joins/seven alternating pairs
after one warmup, exact outputs and byte counts, with elapsed monotonic walltime.
Report whole-command cgroup memory. The earlier Python tracemalloc metric does
not apply to Rust; make no cross-language speed or allocation claim. Register
Rust source/build settings and commands before running, preserve earlier Python
raw outcomes as historical tool evidence only. No product acceptance is gained.


## Boundary and authority

This is a prospective, read-only consistency aid for registered work. It neither
executes jobs nor grants acceptance. A receipt can attest structure and byte
identity, not prove execution, semantics, truthful provenance or qualification
([ADR-0018](../../decisions/ADR-0018-accept-work-on-executable-evidence.md)).
Existing package, custody, query, browser and soak graders remain authoritative.
No product contract, oracle, registered acceptance outcome or release gate changes.

The caller pins a version-1 plan by its SHA-256 on the command line. The plan
records integer `version=1`, `candidate` (40 lowercase hex characters),
`artifacts`, and `jobs`.
The caller separately supplies `--candidate`, `--root`, `--plan` (relative to root)
and `--plan-sha256`; candidate disagreement is an error. Root must already exist,
be absolute and contain no symlink components. Plans may not choose another root.
The CLI runs through the required resource launcher and prints JSON only.
Exit 0 means structurally consistent and every listed job has matched evidence;
exit 3 means consistent but pending/blocked jobs remain; exit 2 means invalid input
or evidence. None means release-ready. An incomplete list cannot establish that
all registered cases were supplied: callers must reconcile required inventories
with their original protocols before freezing a plan.

## Version-1 data contract

Unknown keys in plan objects are refused to catch stale metadata and misspellings.
`artifacts` is a list of `{id, path, sha256}` objects. IDs use lowercase ASCII
letters/digits/hyphens (1–80 characters, first a letter). Paths are nonempty
relative POSIX paths with no empty, dot or parent components. SHA-256 values use
64 lowercase hex characters. Duplicate IDs, paths or conflicting hashes fail.
Every listed artifact is hashed, including unreferenced prerequisite pins.

`jobs` is a nonempty list of `{id, needs, command, harness_revision, expected_exit, evidence}`:

- `needs`: unique existing job IDs, with no self edge or cycle.
- `command`: nonempty list of nonempty strings, compared literally, never executed.
- `harness_revision`: exact 40-hex source revision expected for this job.
- `expected_exit`: integer 0–255 (booleans refused). Expected nonzero exits only
  describe structural joins; named fault/refusal graders still establish reason.
- `evidence`: null for not run, otherwise `{wrapper, resource}` artifact IDs.
  Both must resolve to JSON objects; sharing either receipt between jobs fails.
  Resource units must also be unique across jobs, so copying receipt bytes to
  another path cannot count the same stopped service twice.

The wrapper must contain `state=completed`, exact `command`, `harness_revision`
(40 hex characters), integer `exit` and `launcher_exit` equal to expected exit,
`source_status_before` and `source_status_after` both empty strings, and exactly
one `resource_receipts` entry whose `path` resolves to the declared resource path
under root, whose `sha256` matches its pin, whose integer `exit` matches and whose
`stop_confirmed` is literally true. The resource must have exact `command`,
integer `exit` and `wrapper_exit`, `stop_confirmed=true`, `stop_error=null`, a
`storage_root` exactly root, a `unit` matching `fabric-work-[0-9a-f]+`, and a
completed `command_receipt` matching command, unit, exit and cgroup.
Its cgroup must be an absolute canonical path under `/sys/fs/cgroup`, ending in
that unit plus `.service`, or that exact service's `/workload` child only when
resource `delegated` is literally true. This follows the launcher's two actual
accounting topologies; arbitrary descendants are refused. Wrapper harness
revision must equal the job pin and child `revision`
only if such a child field exists; otherwise no source execution claim follows.
Resource limits are not re-certified by this join; the launcher and original
job-specific admission retain that responsibility.

Jobs without evidence are `pending` if all dependencies are structurally matched,
otherwise `blocked`; jobs with evidence but an unmet dependency are invalid.
Output is deterministically ordered by a lexicographic topological traversal.

## Primitives and composition

Use bounded strict UTF-8 JSON: duplicate keys, non-finite values, excessive
nesting and oversized documents fail. Limits: 4 MiB per JSON document and 64 MiB
aggregate retained JSON payload before parsing, depth 32, 4,096
artifacts/jobs and 65,536 edges; 1 MiB string per command argument, 256 arguments.
Resolve files relative to held no-follow directory descriptors; refuse symlinks,
non-regular files, noncanonical paths and path escape. Hash with 1 MiB streaming
chunks; at most 8 GiB per artifact and 32 GiB total admitted artifact bytes.
Read JSON from the very descriptor being hashed, never reopen it for parsing.
Capture descriptor and pathname identity before/after, including device/inode,
size, nanosecond mtime/ctime. Recheck all artifact identities at composition end.
These checks detect tested replacements/writes; a hostile privileged concurrent
writer or dishonest artifacts are outside the trust model. No persistent digest
cache or pathname-only cache is allowed. One read/hash per unique artifact per
invocation avoids repeated joins; retain only bounded JSON payloads, not packages.

Build ID indexes, adjacency lists and indegrees once. A min-heap gives stable
topological output in O((V+E) log V) time and O(V+E) memory. Compare against a
simple repeated-scan reference rather than inventing engine-performance claims.

## Registered controls and experiment

Before claiming the tool works, run literal positive fixtures and negative
controls for stale candidate/hash, missing evidence, wrong argv/exit/unit/cgroup,
false/boolean stop or exit fields, copied/shared receipt, pending dependency,
duplicate JSON keys/IDs, unknown fields/edges, cycles, path traversal/symlink,
non-regular file, oversized/deep JSON, file replacement and in-place mutation.
Preserve any surprising failure as a minimized deterministic fixture.
An independently written finite graph reference must agree on ordered output
or cycle rejection across all directed graphs up to four vertices.

The original Python measurement (historical after the Rust amendment) measures
graph composition at 64, 512 and 2,048 vertices, chain and fan-out graphs,
seed 17, seven paired samples after one warmup. Compare indexed heap traversal
with repeated sorted scans on identical inputs/output. Report wall time medians,
peak Python allocations and exact-output equality; no minimum speed gate.
For artifact composition, reference repeated streaming hashes versus one hash
shared by 32 joins of the same 8 MiB fixture, seven pairs, same digest. Charge
fixture creation outside measurement; record bytes actually hashed and timings.
Reject optimization if equality or any negative control fails; describe measured
costs even if timing regresses. No universal speed or security claim follows.

Run source checks, experiments and documentation checks through
`python3 tools/resource_group.py -- COMMAND ...`, with data-drive scratch and
the existing serialized source driver, clean source SHA, explicit reservation,
actual exits, logs and outer stopped receipts. Raw results stay on the data drive;
publish bounded results in the research wiki and link them from the evidence index.
