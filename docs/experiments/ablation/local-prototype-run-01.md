# Local prototype and collection correctness, run 01

Status: the integrated lifecycle and its focused checks pass. The [full E3
corpus](resume-e3-run-01.md) and [registered costs](../benchmarks/research-costs-run-01.md)
complete the separate evidence gates in the [completion contract](end-to-end-prototype.md).

## Collection boundary

The [S5 profile](../../../tools/storage-probe/COLLECT_API.md) accepts a capped offline
OTLP/JSON Logs file and caller-owned identity configuration. It preserves physical
record and attribute order, duplicate attribute keys, supported scalar values and
timestamp-presence information. It rejects unsupported structures, unknown or
duplicate object members, explicit nulls, range overflow and known dropped data.
All validation precedes output creation. This is a restricted profile, not a
conformant network receiver or a source-completeness claim.

Two independent adapter candidates passed the 11 frozen mapping tests and two
clarification tests. Candidate A was selected after equal gates with the smaller
implementation. Its `collect.rs` SHA-256 is
`17299a704eb5612fc55bf84c2101be682c2734b93814fe40dcec58049a04afe0`.
GPT's seven independent tests passed. Four scratch defects failed their intended
assertions with exit 101: dropping a LogRecord, trusting a telemetry tenant value,
dropping duplicate attributes and accepting unknown fields. Clean controls exited 0.
Claude's nine combined adapter/CLI probes also passed, including float bits,
ordering, source-identity overflow, capacity-1 retry and checkpoint mismatches.
Both families approved the unchanged adapter. See the [collection artifacts](data/collection-s5-run-01/).

## CLI selection and repairs

Both lifecycle candidates initially passed three frozen process tests. Broader
capacity/batch combinations found that candidate A drained only one final batch and
could report success with queued events lost. Candidate B fully drained the queue
and was selected. Both candidates initially collapsed duplicate query members via
`serde_json::Value`. B now checks the required field set, then deserializes the
original bytes into the typed query so duplicates are rejected.

Independent review found that B could publish a valid 75,498,948-byte FOL2 log even
though the CLI contract caps every file input at 64 MiB. A new regression failed
before repair (exit 101). B now checks log length before opening/replaying it in
publish and existing-log ingest. This limits the research CLI; the general EventLog
API retains its existing behavior. The repaired regression and six other CLI tests
passed on ext4. GPT repeated the large-log refusal and 48 process probes; Claude
repeated all seven CLI tests. Both approved final `fabric-research.rs` SHA-256
`a75d6e6585649d2c75172085a3e4c7761db4741e22f90198798f7a98628fc2ba`.
The [CLI artifacts](data/lifecycle-cli-run-01/) retain original failures and repairs.

## Reproduce the integrated lifecycle

From the repository root, use a fresh output directory whose parent already exists:

```sh
mkdir -p target
python3 -B tools/bench/run_prototype.py target/prototype-demo
```

The recorded run exited 0 and preserved 21 separate process commands. It adapts
caller-authored Log input, ingests through a capacity-1 buffer, publishes a snapshot
and checks a complete answer. An identical ingest retry changes no log bytes. A new
partial query then resumes through a cold-directory move and a missing block;
verification refuses the still-incomplete answer. Restoring the retained block
closes the same snapshot with positions `[0, 2]`. Removing the manifest makes query
fail; same-root rebuild restores it. A late event is appended and published as a
successor. Reusing the old checkpoint is rejected; the new snapshot verifies as
`[0, 2, 3]`. Earlier source and checkpoint files remain intact.

The [preserved lifecycle and validation artifacts](data/local-prototype-run-01/)
include every command/exit, inputs, publications, checkpoint digests, output hashes,
binary/source hashes and illustrative elapsed times. Its ingest-to-first-complete
query observation includes subprocess launch and report checks on this shared
WSL2/ext4 host. One tiny lifecycle observation is not a latency benchmark.

The integrated research test command exited 0 while explicitly skipping only the
separately completed expensive E3 corpus on the selected B body. The eight layout tests passed. The Linux
partial-read/EIO harness executed its two normally ignored tests with observed
injections and exited 0. `cargo test --offline --locked --all-features` passed all
25 root integration tests (exit 0). Source-pinned review and these finite checks
do not establish physical power-loss safety, network delivery, clustering or
malicious-executor correctness. The [prototype architecture](../../architecture/research-prototype.md)
and [experimental decision](../../decisions/ADR-0009-isolate-durable-research-snapshots.md)
state the retained-source and external-trust assumptions.
