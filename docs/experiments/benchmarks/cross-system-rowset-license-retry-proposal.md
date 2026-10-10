# RowSet known-license-alias retrieval retry

Prospective, separately registered checker correction after `query/rowset-01`
failed before compilation. The [original screen](cross-system-rowset-sweep-proposal.md)
and its failed receipt remain unchanged. The complete failed state was archived,
read back and owned scratch removed. This retry uses fresh `query/rowset-02`;
it neither retroactively passes that failure nor changes the finite grid.

The pinned and SHA-256-authenticated Roaring archive contains two source-license
aliases: `roaring/LICENSE-APACHE -> ../LICENSE-APACHE` and
`roaring/LICENSE-MIT -> ../LICENSE-MIT`. The blanket source link rejection
encountered these benign archive members before compilation. Prospective rule:
only for the registered Roaring package, recognize exactly those two stripped
paths, exactly those relative targets, and the tar symbolic-link type. Record
and **skip** them; never extract a filesystem link. Require the corresponding
root `LICENSE-APACHE`/`LICENSE-MIT` regular member to exist, and record its bytes
and SHA-256 as the skipped alias's target evidence. Those regular files remain
part of the full source-member inventory and exact archive checksum.

Every other link, hardlink, special member, path traversal, duplicate or
noncanonical path still rejects. Do not allow arbitrary parent-relative links.
Representative controls accept the known alias with a regular target and reject
wrong target, wrong path, absolute target, wrong package and missing regular
target. Source pin/checksums, actual manifest-edition checks, native code, feature
closure, Python membership oracle, 48-cell grid, resource bounds and cleanup
contract remain unchanged. This protected extraction-check correction gets its
own reasoned commit before retry.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id rowset-02 --lab query --seconds 600 --reserve-mib 64 -- python3 -B tools/bench/labs/cross_system/rowset_sweep.py --destination docs/experiments/benchmarks/data/cross-system-sweep-01/query/rowset-02 --protocol docs/experiments/benchmarks/cross-system-rowset-license-retry-proposal.md
```

The retry's protocol receipt captures this supplement; it references the complete
original procedure above. All actual command times still count against the round.
No expectation of a performance win or deployment qualification is added.
