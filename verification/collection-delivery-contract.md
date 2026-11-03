# Verified local collection delivery

The delivered release SDK and Spindle must support a reproducible user-service
installation and real executable smoke test. Installation resolves absolute user
paths, protects private configuration/runtime/state, preserves existing custody and
configuration, and rejects incompatible existing files. Repeating the same install
is safe. Default collection accepts SDK events only, with 64 MiB local custody.
Readiness requires a successful status request, not an elapsed sleep.

Rust and persistent Python adapter harnesses must submit identifiable typed records,
obtain successful flush, and export those exact records. Restart must preserve the
acknowledged records and permit new submissions. Destructive fault and quota tests
use owned temporary custody only. Children and waits are bounded and cleaned up.

Every SDK payload buffer has one stable capacity and exactly one ownership phase:
free, encoding, queued, or in-flight. Verified executable transitions must preserve
capacity, reject invalid transitions without mutation, return rejected encoding
capacity without advancing admission, retain charged payload through retries, and
release only after matching confirmation. Independent tests connect the kernel to
actual adapter ownership, identity checks and concurrent use. Model assumptions,
implementation tests, and machine-checked executable properties remain distinct.

An experimental representation comparison must retain exact v2 observations while
bounding batches to 32 records/256 KiB canonical JSON and frames to 1 MiB. Compare
direct JSON, direct binary and directly constructed Arrow IPC with equivalent
batching, without intermediate full-batch dynamic value trees. Reject malformed
metadata, excessive lengths, unsupported compression/schema/dictionaries before
unbounded allocation. Correctness gates precede performance comparisons. Freeze a
new evaluator revision and fresh held-out workloads before selection. Report memory,
CPU, latency, throughput and size independently; no global optimum is claimed.

Native acceptance binds concrete source, checker results and delivered binaries.
The actual user service must be running after successful installation and smoke.
Arrow remains experimental; production transport, remote custody, Parquet and SDK
package separation are outside this delivery contract.
