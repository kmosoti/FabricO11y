# Making the experiments durable

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). This page says how the experiments were run until now, what that left fragile, what has been moved into the repository, and what remains to make the research loop something anyone can run on this container or the target host, from a clean clone, and get the same numbers.

## How it was run

Every experiment from the sealer study to L-21 ran from a scratch directory outside the repository: a detached git worktree of the server with the prototypes patched into `query.rs` and `segment.rs` and the sealer-study module added by hand; binaries copied into `bin/stock`, `bin/proto`, `bin/budget` after each build; Python harnesses with absolute paths to that scratch directory and to data under a home directory; `pyarrow` and `zstandard` installed with pip for the byte measurements; the real-text corpus fetched by hand; generated journals and sealed states reused across runs by hard links. The records cite the harness sources as `.txt` copies and keep the JSON results, so every number is reproducible in principle, but the chain from a clean clone to a number was in one person's shell history.

## What is now in the repository

[tools/research](../../tools/research/README.md): the prototype patches with a build script that applies them in a throwaway worktree (the product tree is never patched), the generator and benchmark sources, the harness scripts with every path resolved from one environment variable, the corpus fetch with digests, a requirements file for the optional byte measurements, and a smoke loop that runs the chain at a size this container finishes in minutes. `tools/model` holds the cost model with its calibration test. The specification and its theorems are in the core and the properties crate, where the workspace checks run them.

## What is still fragile, and the remedy for each

| Fragility | Why it matters | Remedy | Cost |
| --- | --- | --- | --- |
| The prototypes are a patch. The index, walk and budget exist as a diff against `query.rs` at `58b694d`; a change to the server's query path breaks the patch silently. | The next query-side experiment needs them, and so does promotion. | Promote under Q2 (ledger L-03 and L-04 together, with the file-order fallback) so the mechanisms become product code behind no flag; until then the build script fails loudly when the patch does not apply. | The promotion: a week of product work and the soak. |
| The generator and sealer-study module live in the patch too. | Every workload (steady, outage, adversarial, tiny, needle, skew, corpus) comes from it. | A `fabric-bench` crate in the verification layer, depending on `fabric-server`, holding the generator, the sealer-study strategies and the `fob` and codec benchmarks as binaries; the few server items it needs (`hex`, the batch builders, `row_group_bounds`, the group scans) exposed as `pub` with a doc comment naming the consumer. | Two days. The layer map allows it today. |
| The harnesses share no code. Fourteen scripts each define a server launcher, a client, the shapes and the differential by copy. | A fix to one (the rate page bug, the token-advance check) does not reach the others. | One `harness/common.py` with the launcher, the client, state builders, shape sets, the differential and the receipts; each experiment a short script over it. | Two days; the scripts are already uniform. |
| Results carry no provenance. A JSON file does not say which commit, patch, binary digest, CPU pinning or corpus produced it. | A number cannot be tied to the code that made it, which the evidence rule requires. | Each run writes a receipt beside its JSON: commit, patch digests, binary SHA-256s, host (`nproc`, model, kernel), corpus digest, start and end times; the record's data directory keeps it. | Half a day. |
| No registered check runs any of it. | A change can break the whole chain unnoticed until the next experiment. | A `research-smoke` entry in the `extended` profile of `xtask/checks.json` running `tools/research/smoke.sh` with a time budget, in its own commit (verification policy). | Half a day, plus the policy commit. |
| The byte measurements depend on `pyarrow` and `zstandard` from pip. | They are not pinned to the vendored registry and may not install offline. | Port `segstat` and `reencode` to a `fabric-bench` subcommand over the `parquet` and `zstd` crates already in the lockfile; keep the Python as the diversity check. | One day. |
| The corpus is eight 2,000-line samples drawn at random. | Real streams have bursts and repeats (hypothesis B3); the samples' licence allows research use but the files are fetched, not committed. | Keep the fetch with digests; add a Spindle capture of this container's own logs and `/proc` series as a second corpus with counters (B4, C4); never commit either. | One capture run. |
| Generated states are rebuilt by hand and reused by hard link. | A state's provenance (generator version, size, seed) is in a directory name. | The state builders in `common.py` name states by a digest of their parameters and rebuild on a mismatch. | Part of the `common.py` work. |
| The proofs of the query specification do not finish. | The theorems rest on random properties only. | Restate the definition's data structures in a proof-friendly form (a fixed-capacity array, a loop-written selection sort) without changing its meaning; keep the vector form as the readable one and prove the two equal by property. | Two days; uncertain whether CBMC then finishes. |
| Disk. This container has about 2.4 GiB free after a day of states. | A full-size run (B5 at 20 GiB) cannot happen here. | A `clean.sh` that removes states and keeps results; the product-size runs on the target host. | An hour. |

## Running on this machine

`bash tools/research/smoke.sh` builds the binaries, fetches the corpus, generates a 16 MiB real-text tail, runs the L-21 shapes and a 60-query differential against the walk, the codec micro-benchmark and the cost model's calibration test. The full-size runs are the harness scripts under `tools/research/harness/`, each with its budget in the README; the heaviest, `budget.py`, is three hours because the stock server's page drains dominate. The container's limits are four CPUs, of which the harnesses pin the server to two, and the disk above.

## The order

1. `common.py` with receipts, and the smoke loop registered in the extended profile: the loop becomes something a check can fail.
2. The `fabric-bench` crate, so the generator stops being a patch.
3. Promotion of L-03 and L-04 under Q2, so the query prototypes stop being a patch.
4. The proof-friendly restatement of the specification.
5. The capture corpus with counters.
