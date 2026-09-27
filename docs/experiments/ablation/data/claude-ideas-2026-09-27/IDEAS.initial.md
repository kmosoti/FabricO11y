# Ideas: evidence-first observability with exact completeness at lower cost

Status: steps 1-4 of /mad-scientist (orthodoxy, hunches, divergence, novelty proxy). **Independent
critique (step 5) and promotion with pre-registered kill tests (step 6) are pending**; the parent
supplies a cross-family critic. Nothing below is a result. Every idea is a hypothesis.

Ledger: `IDEAS.jsonl` (events written by `ideas.py`). Literature cache: `lit/papers.jsonl` (180 records).

## Evidence read (Fabric, branch research/storage-query-ablation)
Revision: `TODO: git -C /home/kmosoti/projects/fabric_o11y rev-parse HEAD` (git was denied in this
session's permission mode; the loose ref file `.git/refs/heads/research/storage-query-ablation` read with
the Read tool matched the revision stated in FABRIC_CONTEXT.md, but that is not a git-computed value).
Working-tree cleanliness was **not** checked.

Read: `docs/CURRENT.md`, `docs/architecture/system.md`, `src/lib.rs`, `src/log.rs`,
`tools/storage-probe/src/lib.rs`, `docs/experiments/ablation/observability-storage-research.md`,
`docs/experiments/ablation/storage-query-s1-run-01.md`. Facts used below come from those files:
two syncs per `append` (`src/log.rs:104-111`); Stage 6: 340.0-394.8 durable events/s with
99.965-99.969% of pipeline time inside `append`; S1: 15,360 query executions all equal to the reference,
93.945% rows avoided for clustered narrow-time queries, 1.172-1.367% for shuffled time, 100% for
absent tokens, 0% for common tokens in Log/mixed rows, summary build 39.3-2,206.5 µs per snapshot
(RAM logical work, not disk I/O). The agenda's proof (§ Contract before acceleration) assumes a
complete immutable snapshot and names stale-summary reuse as unsafe.

## Baseline (recorded, never offered)
Immutable time-partitioned blocks, optional min/max and Bloom summaries, selective Parquet projection,
postings or materialized views, adaptive indexes, OTel sidecars, later offload; accelerators are
disposable caches rebuilt from raw evidence, missing summaries mean full scan. Prior art for its pieces:
zone maps / column stores [doi:10.1561/1900000024], Bloom filters [doi:10.1080/15427951.2004.10129096],
range filters [doi:10.1145/3183713.3196931], adaptive indexing / cracking [doi:10.14778/2168651.2168652].

## Assumptions and genealogy
| id | assumption | core | origin | still holds? |
|---|---|---|---|---|
| A1 | Completeness is the reader's scan; the iterated block list is trusted to be the whole snapshot | yes | single-node warehouses owned the heap file; S1 owns all rows in RAM | **no** once blocks live on disk/remote |
| A2 | Durable commit needs ordering by two syncs (frame, then marker) | yes | WAL practice without ordering primitives; ADR-0005/0006 per-event lesson | unknown |
| A3 | Indexing is a phase after ingest, published separately, so it can be missing or stale | yes | batch ETL, Lucene refresh: index build was expensive relative to the write | **no** for a single writer that already touches every byte and waits on sync |
| A4 | Identity is position in one log; auditing ACKed-vs-durable requires full sequence comparison | yes | FOL2 single writer; `EventId` not globally unique | unknown |
| A5 | A result is a row list; coverage is implicit; a query succeeds fully or fails | yes | SQL closed world on a local DB | **no** for partial availability |
| A6 | Absence is proven by exhaustive scan or a probabilistic positive summary; raw is stored verbatim | no | Bloom filters were cheap to bolt on | unknown |

## Tensions
- **T1** S1's no-false-negative proof needs a complete block set, but nothing attests the block list; a lost block and an empty region look the same.
- **T2** 99.965-99.969% of time is in the two-sync append, yet the research effort optimizes RAM pruning worth microseconds.
- **T3** Torn-tail handling is exact only through sync ordering, while corruption detection already rests on CRC32 (not authentication); two kinds of guarantee are mixed and only one is called exact.
- **T4** After a failed sync the file cannot reveal lost ACKed records: the no-ACK-before-durable-owner contract is preventable but not observable.

## Hunches and where they went
- **H1** Indexes never record their own omissions (standpoint: a historian in 2126) → I1, I3.
- **H2** The sync shadow is free compute (standpoint: the phenomenon; writer reads every byte then idles) → I2, I5.
- **H3** Double-entry bookkeeping for ACKs (standpoint: an accountant) → I4.

## Ideas
Novelty column: `ideas.py novelty --all` cosine maxima (paper / other idea / baseline). All six were
labelled `novel-by-proxy`. **That label is a weak proxy**: the cache was harvested on narrow angles and
the embedder is MiniLM. Each idea's *Known prior art* line is the real novelty judgment, and several
ideas are engineering combinations, said so explicitly.

### I1 — Coverage receipts (operator: invert; breaks A1, A5; resolves T1; core-breaking)
**Mechanism.** The writer owns completeness. Each block seal extends a hash chain
`seal_k = H(seal_{k-1} ‖ block_id ‖ row_count ‖ H(block bytes) ‖ H(summary bytes))`. A query returns rows
plus a receipt: chain head, and for every block one of `scanned`, `rejected by summary digest d`,
`unavailable`. A verifier replays the receipt against the chain using seals and summary digests only.
A dropped block, reordered block or summary from another snapshot fails verification; an empty answer
counts as complete only if the receipt covers every sealed block.
**Thought experiment.** 32 blocks, block 17 holds the only rare-token match. (a) block file deleted →
chain head not reproducible → empty answer refused; (b) block 17's Bloom swapped for the previous
snapshot's → the rejection cites `d_old ≠` the digest in `seal_17` → refused; (c) clean → verifies.
**Known prior art / novelty limit.** Hash chains and Merkle logs for append-only proofs
[doi:10.1145/2668152.2668154] [doi:10.1145/3319535.3345652]; authenticated range-query completeness
[doi:10.1007/11535706_7] [arxiv:1812.02386]. Those target a *malicious* server and prove membership or
range completeness over data. The claimed difference is narrow: the receipt binds *which accelerator
justified each skip* to the durable commit, so "missing acceleration" and "missing data" become
mechanically distinct. Aimed at accidental omission (lost/stale files), not an adversary. Probably an
engineering application of ADS ideas, not a new primitive. Relies on hash collision resistance; this is
a stated assumption, like the current CRC32.
**Kill test.** 3 injected defects (deleted block, stale summary, swapped order) × 12 S1 snapshots plus
12 clean controls. Kill if any defect verifies, any control fails, verification reads any row byte, or
sealing adds a sync per block (count with strace).
Novelty proxy: 0.448 / 0.538 / 0.442.

### I2 — Remove the ordering sync with content-bound group seals (subtract; breaks A2; resolves T3; core-breaking)
**Mechanism.** Write a group of frames plus one seal (byte length + SHA-256 over the group), then **one**
sync. Recovery accepts a group only when its seal is present and matches; anything else is an unsealed
tail and is truncated. Content binding replaces write ordering. ACK is still sent only after the sync
returns, so the durable-owner contract is unchanged.
**Thought experiment.** A device that persists any subset of sectors in any order before a crash. For
every one of the 2^n subsets, a partial group cannot match its seal, so recovery yields a group-aligned
prefix, and nothing whose sync returned is lost.
**Known prior art / novelty limit.** OptFS decouples ordering from durability with checksums and
osync/dsync [doi:10.1145/2517349.2522726]; group commit and external synchrony [doi:10.5555/1298455.1298457].
ext4 journal checksums are the same trick (general knowledge, not in cache). **This is an engineering
application, not a research novelty.** Its value is T2: it attacks the phase that dominates cost. It also
exposes a contract question: Fabric's docs describe recovery as deterministic, and I2 makes the hash
assumption explicit instead of hiding it in CRC32.
**Kill test.** Exhaustive sector-subset crash enumeration for groups of 1-6 frames (pure sector model,
or dm-log-writes replay). Kill on any non-prefix recovery, exposed partial group, or loss after sync
returned. Kill the cost motive if group size 1 is not ≥1.5× the two-sync baseline in durable events/s on
the Stage 6 host.
Novelty proxy: 0.531 / 0.449 / 0.278.

### I3 — Residual answers (reframe; breaks A5; core-breaking)
**Mechanism.** An answer is `(R_c, q_r)`: exact rows over covered sealed blocks, plus the same predicate
restricted to the named uncovered blocks. Answers compose by block position into exactly `R`, keeping
order and duplicates. Unavailable, cold or corrupt blocks become residual work with an identity, not an
error or a silent empty. Completeness is monotone and client-driven; cold reads are paid only when the
client needs closure.
**Thought experiment.** 4 blocks, block 2 offline → return blocks 0, 1 and 3 plus residual `{q, block 2}`;
later the residual's rows merge by position into the full-scan list. No empty-residual answer can
omit a sealed block.
**Known prior art / novelty limit.** Online aggregation and partial-shard results in search engines are
close. Online aggregation (Hellerstein et al., 1997) is **not in the cache and is uncited: citation gap**;
shard-failure counts in Elasticsearch are vendor behaviour. Those report statistical confidence or a
failure count, not an executable exact residual. **This is likely a twin of I1**: I1's `unavailable`
entries are the residual. The critic should decide whether to merge them.
**Kill test.** 1,000 random availability masks and re-availability orders per S1 snapshot. Kill if any
composed answer ≠ the full-scan position list, any empty-residual answer omits a sealed block, or
merging fails on duplicate `EventId`s.
Novelty proxy: 0.365 / 0.538 / 0.389.

### I4 — Reconcile the ACK ledgers with an IBLT difference (transplant from networking; breaks A4; resolves T4; core-breaking)
**Mechanism.** The sender keeps an invertible Bloom lookup table over `(producer, sequence, record-hash)`
for events ACKed in an epoch; the durable owner keeps one over what it sealed. Subtract the tables and
peel. The outcome has three values: empty difference; an exact listed difference (ACKed-not-durable is a
named contract violation; durable-not-ACKed is a safe duplicate candidate); or **decode failure**, which
forces a full comparison and is never taken as a match. This gives T4 a runtime detector, including
after an fsync failure the log itself cannot see, at a cost proportional to the loss, not the epoch.
**Thought experiment.** 10^6 ACKed events, and a writeback failure drops 3. A table of about 60 cells
lists exactly those 3. With 10^4 lost, peeling fails, which is reported as undecodable and triggers the
full compare.
**Known prior art / novelty limit.** IBLT and difference digests [doi:10.1109/allerton.2011.6120248]
[doi:10.1145/2018436.2018462]; rateless reconciliation [arxiv:2402.02668]. The primitive is known. The
claimed contribution is the transplant: auditing a delivery-ownership contract (sender retention vs.
durable owner) with it, keyed by sequence so that duplicates survive. A false "empty" requires a
checksum/hash collision in the cells; this is an explicit probabilistic trust assumption and must be
stated, not hidden. Whether that is acceptable under "no hidden relaxation via probability" is a **scope
question for the owner**. The mitigation is a wide (≥128-bit) cell hash plus periodic full comparison.
**Kill test.** Epochs of 10^4-10^6 tuples, injected loss d ∈ {0, 1, 3, 30, 300}, with duplicate retries.
Kill if any d>0 run reports empty, any decode lists a wrong tuple, or audit bytes exceed 1% of a full
tuple exchange at d ≤ 30.
Novelty proxy: 0.444 / 0.496 / 0.424.

### I5 — The commit marker is the block summary (unify; breaks A3; resolves T2; core-breaking)
**Mechanism.** A block's commit record carries its time min/max, exact tenant set and token summary,
computed in the sync shadow. The durable commit *is* that record's sync. A committed block without its
summary, or a summary for other rows, cannot exist, so staleness is impossible by construction. Rebuild
from raw becomes a verification step (recompute and compare), not a recovery path.
**Thought experiment.** 2 ms sync, 0.9 ms summary build: no extra wall time and no publication window
between rows and summary.
**Known prior art / novelty limit.** Writing per-part min/max and checksums atomically with a part
(ClickHouse MergeTree parts; Parquet footers) is established practice; see the vendor docs cited in the
Fabric agenda. **This is an engineering combination.** The only non-obvious part is making the summary
the *durability* record in a two-phase commit log, so the agenda's stale-summary counterexample becomes
unrepresentable. It composes with I1 (the seal is the summary digest) and I2 (the seal is the group commit).
**Kill test.** Crash-inject at every write boundary over 1,000 sealed blocks. Kill on any recovered
summary that differs from a fresh recomputation. Kill the cost claim if median durable block commit
latency rises >3% versus the same writer without summaries (20 paired trials).
Novelty proxy: 0.400 / 0.519 / 0.440.

### I6 — Exact absence by lossless decomposition, gated at the ACK boundary (exact; breaks A6; not core)
**Mechanism.** Treat "logs are templates plus variables" as exact: store `(template id, typed variables)`
with per-block exact template and value dictionaries. A token absent from both is *exactly* absent: a
deterministic certificate with no Bloom false positives. The contract twist: before ACK, the writer
decodes and byte-compares with the original (as `same_record_contents` does). A non-round-tripping event
is stored verbatim instead, so lossless reconstruction is a durability-boundary invariant.
**Known prior art / novelty limit.** CLP [t:clp-efficient-and-scalable-search-on-compressed-text-logs]
and LogGrep [doi:10.1145/3552326.3567484] already search template-decomposed compressed logs; LogShrink
[doi:10.1145/3597503.3608129]. **Close to known.** Only the ACK-gated round-trip invariant is
Fabric-specific, and that is engineering. Kept as the portfolio's `exact` operator. The critic may kill it
as `known`.
**Kill test.** Pinned Loghub subset [arxiv:2008.06448] (hash recorded) plus the S1 corpus. Kill on any
unflagged round-trip failure, any dictionary-excluded block that contains the token, or dictionaries
larger than raw for more than half the blocks.
Novelty proxy: 0.503 / 0.496 / 0.422.

## Proposed top 3 (for the critic; not promoted)
Ranked by change-if-true divided by kill-test cost:
1. **I1 Coverage receipts.** Closes T1, the premise the S1 proof currently leaves unstated. The kill test is cheap (hash replay over the existing S1 snapshots). It subsumes I3's residual as `unavailable` entries.
2. **I4 IBLT ACK-ledger audit.** Gives the delivery contract its first runtime violation detector (T4). A pure simulation kill test is cheap. It carries the explicit probability-scope question.
3. **I2 One-sync content seal.** Targets the phase holding 99.97% of cost (T2). The kill test is an exhaustive sector-subset model and is cheap. Labelled engineering (OptFS lineage), not research novelty.

A composed architecture, **I1 + I2 + I5**, is worth naming: one sealed group commit record carries the
chain link, the group hash and the summary. It is one sync per group, cannot be stale, and verifies
coverage. That is a combination claim and must be tested as one only after each part survives alone.

## Checks run in this session (exit codes)
- `ideas.py baseline/assume ×6/tension ×4/hunch ×3/add ×6`: all exit 0.
- `ideas.py novelty --all`: exit 0; all six `novel-by-proxy` (scores above).
- `ideas.py check`: exit 0; "6 ideas (6 live, 0 parked), 3 hunches, 0 rule failures".
- `lit.py search` ×14 (first session): exit 0 each; Semantic Scholar returned HTTP 429 on most runs and OpenAlex 429 on two, so coverage came mainly from OpenAlex/Crossref/arXiv.
- `lit.py get` ×4: exit 0.
- `lit.py cites IDEAS.md`: exit 0; "17 cited, 0 unknown".

## Open limitations
- The critique (step 5), promotion and pre-registration (step 6) have not been done.
- Novelty scores do not name the nearest paper, and the harvest missed Crosby-Wallach history trees, online aggregation and fsync-failure studies (PostgreSQL fsyncgate; Rebello et al. ATC 2020). These are **citation gaps, not evidence of novelty**.
- Scope question for the owner: do hash-collision-based guarantees (I1, I2, I4) count as a "hidden relaxation via probability"? The current log already depends on CRC32 for corruption detection.
- No Gemini/GPT generator ran (bounded inline pass), so diversity across model families is absent.
