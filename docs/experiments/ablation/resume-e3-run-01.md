# E3R: snapshot-bound residual answers, run 01

Status: the selected candidate B passed the fixed finite corpus and independent review. This is a research-library correctness result, not an application query service or a cost result. The [registered protocol](resume-e3-protocol.md) and [fixed API](../../../tools/storage-probe/RESUME_API.md) define the claim.

## Claim and setup

Given an immutable snapshot, valid authenticated summaries, an independently retained anchor, collision-resistant hashes and a trusted exact scan executor, compatible pages can merge in any order into the own-snapshot positional full-scan answer. A repeated page is idempotent. A changed binding or conflicting resolved block fails without changing the accumulator; unresolved blocks remain explicit. The implementation uses a private per-block state, validates a whole page before mutation, and derives residual work from the authenticated full layout.

An independent oracle and tests were [frozen before implementation](data/resume-e3-run-01/oracle-freeze.json). Candidates A and B each passed the fast six-test `resume_contract` suite and three-test `resume_selection` suite (exit 0). The original full-package sessions on both candidates ended without recoverable final output or exit codes; they are **not** counted as passes. Candidate B was selected for a fresh, unchanged full-corpus run. Candidate A has no qualified full-corpus result. The selected B `resume.rs` source hash was `6621d9c44dde31cce5c4f85bc01cb7265c55c7dd0fe6768a4590054bf5f509a0`.

## Executed gate

`cargo test --release --offline --locked --manifest-path tools/storage-probe/Cargo.toml --test resume_e3 -- --nocapture --test-threads=1` ran in the candidate B worktree and exited **0**. Output: `1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. The [command receipt](data/resume-e3-run-01/candidate-b-full.json) and [six corpus rows](data/resume-e3-run-01/candidate-b-full.stdout) preserve the actual exit and counters:

| Shape | Seeds | Cases | Retries | Permanent-unavailable cases |
| --- | --- | ---: | ---: | ---: |
| Mixed | 201, 202, 203 | 384,000 | 767,997 | 12,000 |
| Shuffled Logs | 201, 202, 203 | 384,000 | 767,997 | 12,450 |
| **Total** | Six snapshots | **768,000** | **1,535,994** | **24,450** |

Each snapshot had 2,048 rows, 32 blocks, 128 queries and 1,000 masks per query. The corpus also recorded 480 conflict cases, 300 successor checks and 768 adapter cases, with **zero contract violations** in each of six rows. Permanent unavailability was kept unresolved; successor snapshots with reused positions were rejected. The tests cover finite inputs and fixtures, not all possible snapshots.

The [mutation record](data/resume-e3-run-01/mutations.json) shows a clean exit **0** and exit **101** for each injected defect: omitting expected binding, ignoring a conflict, and deduplicating equal row digests. Independent GPT probes passed six tests (exit **0**) and Claude probes passed nine tests (exit **0**), covering binding changes, residual subsets, atomic conflict rejection, duplicate physical rows, replay order and unresolved versus resolved-empty blocks. Both reviews approved candidate B; see the [GPT record](data/resume-e3-run-01/gpt-review.json) and [Claude record](data/resume-e3-run-01/claude-stdout.json).

## Integration and limits

The main research package integrated candidate B with Serde import, derives and attributes for the later S2 checkpoint wire types. The [integration record](data/resume-e3-run-01/serde-adaptation.json) confirms the Rust body is byte-equivalent to the B source after removing only those Serde additions; its integrated source hash differs for that reason. The full E3 corpus above was run against the candidate B body, not independently repeated against the integrated Serde form.

The accumulator authenticates layout and page structure, but cannot prove that a trusted executor actually scanned every row in a `Scanned` block. Completion does not establish raw retention, permanent availability, disk recovery, or performance. Receipt/resume cost has [measured results](../benchmarks/research-costs-run-01.md) under its [separate registered protocol](../benchmarks/receipt-resume-cost-protocol.md). The local disk lifecycle has its own [S2 result](durable-snapshot-s2-run-01.md).
