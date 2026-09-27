# Alpha completion plan

Status: proposed on 2026-09-27 from checkpoint commit `af5b7bc`. The [alpha contract](ALPHA.md) owns every gate, workload and numeric target. This page orders the remaining work, names the design choices the contract leaves open, and says which check decides each step. Nothing below is implemented unless the [phase ledger](ALPHA.md#phase-ledger) says so. Items marked **owner** need the owner's answer before the affected step starts. The owner's standing constraint applies throughout: no over-engineering, and every feature or configuration value must have a current consumer and a test.

## 1. Starting point

- Phase 0 is approved. Phase 1 has `fabric-node`, `fabricctl inspect`, the `FAB1` journal, the host sampler and the log reader, with 21 alpha tests and the unchanged 25 legacy tests passing. Both reviewers approved the four [review repairs](experiments/formal/alpha-phase1-review-checkpoint.md) on the recorded hashes.
- Phase 1 is not promoted. Open: the [Unicode gap-text counterexample](experiments/formal/alpha-phase1-review-checkpoint.md#re-review-outcome-and-open-counterexample), a post-repair native run, and decisions D1 and D2 below.
- Phases 2 to 5 have no code. A [phase-2 oracle brief](experiments/benchmarks/data/alpha-review/parent-notes/phase2-oracle-brief.md.txt) was issued, but that session produced no files.
- The GPT reviewer's quota is exhausted until 2026-10-03. The Claude verifier and the Gemini verifier are available for the independent half of each review.

Building blocks that exist: the `Batch` envelope and journal in [journal.rs](../src/alpha/journal.rs); the node cycle in [node.rs](../src/alpha/node.rs); the phase-0 [runner](../tools/alpha/runner.py), [workload](../tools/alpha/workload.py) and [rate oracle](../tools/alpha/rate_oracle.py); the checked [delivery ownership model](../formal/delivery/README.md); and research Parquet code in the [layout probe](../tools/layout-probe/README.md), which is evidence for format choices rather than application code.

## 2. Design decisions the contract leaves open

### D1. Crash during an append (accepted; implemented in P1.2, [ADR-0011](decisions/ADR-0011-separate-interrupted-append-from-known-failure.md))

Today the `recovery-required` sidecar is written before any frame bytes, and reopen refuses while it exists ([journal.rs](../src/alpha/journal.rs)). A SIGKILL, an OOM kill, power loss or a plain SIGTERM inside the few-millisecond append window therefore makes the spool unopenable, although the node never advanced its cursor for that batch and nothing was acknowledged. Recommendation: split the sidecar in two. `append-in-progress` is resolved on reopen by verifying the tail exactly as reopen already does without a sidecar: a frame with a valid marker is kept, an unmarked tail is truncated. `recovery-required` is written only when a write or sync call has reported an error, which is the case the contract calls a known failure. Add a directory sync after removing the in-progress marker, and handle SIGTERM in `fabric-node` so a stop finishes the in-flight cycle. This revises one sentence of the [safety contract](ALPHA.md#safety-contract): readable bytes cannot clear a *reported* error. The journal oracle's assertions stay. Alternative: keep the current rule and add a manual `fabricctl recover` doing the same tail check. Same code, worse availability, and the phase-5 restart gate becomes timing dependent.

### D2. Clearing coverage-unknown (accepted; implemented in P1.2)

Any append failure writes `coverage-unknown`, and every later cycle refuses with no operator path back ([node.rs](../src/alpha/node.rs)). Recommendation: a cycle that finds the marker proceeds and adds one gap entry, "coverage unknown since <marker time>", to its batch; a successful commit removes the marker with a directory sync. A full spool keeps refusing until phase-2 reclaim frees space, then the next commit records the gap. Missed data is still never called delivered, and no command or option is added.

### D3. Package layout and dependencies (recommended; record as ADR-0012 when phase 2 starts)

Use a Cargo workspace. The root package stays as it is: domain types, FOL2, the demo, the `alpha` module and the `fabric-node` and `fabricctl` binaries. Add `crates/fabric-server`. Server dependencies: tokio 1.53.1, axum 0.8.9, axum-server 0.7.3 with rustls 0.23.45, prost 0.14.4, and in phase 4 parquet 60.0.0 with zstd. Node and CLI HTTP client: ureq 3.3.0 with rustls, synchronous, so the node gains no async runtime and keeps its 64 MiB RSS margin. Every version is pinned exactly and is already in the local cargo cache. No SQLite: control state for at most 1,000 nodes is one atomically replaced, synced protobuf file, and dedup state is rebuilt from journal replay. TLS material is operator-provided PEM; qualification generates it with `openssl`. No `rcgen`, no watcher framework, no plugin boundary.

### D4. Delivery and dedup rule (recommended; record as ADR-0013)

One batch in flight per node, sent in sequence order as the exact stored bytes. Per (node identity, generation) the server keeps the last committed sequence and the SHA-256 of its bytes, durable because both are replayed from the server journal. Rules:

| Node sends | Server answer |
| --- | --- |
| `seq = last + 1` | commit, then ACK `committed_through = seq` |
| `seq = last`, equal hash | ACK again (lost-ACK retry) |
| `seq = last`, different hash | 409 conflict; the node quarantines that generation |
| `seq < last` | ACK `committed_through = last`; the node advances its ACK cursor |
| `seq > last + 1` | 409 gap; the node resends from `last + 1` |
| bad or revoked credential, malformed, oversize, commit queue full | 401, 400, 413, 503 with bounded retry |

A new generation starts at sequence 1 and is independent. This meets the contract's three identity rules with bounded server state. A prover extends the [TLA+ delivery model](../formal/delivery/README.md) with sequences and this rule before the server commit path is written; the property is that every acknowledged batch is durable exactly once under lost ACK, retry and restart.

### D5. Spool reclaim after ACK (recommended)

Rotate the node journal into fixed-size files (`batches.000001.faj`, closed at 8 MiB). Persist the ACK cursor `(generation, sequence)` in a small synced file after each ACK. Delete a closed file when every batch in it is at or below the cursor. The frame format, identity file and inspect semantics stay; inspect adds acknowledged and unacknowledged counts. The server journal uses the same rotation so phase 4 can seal and delete whole files.

## 3. Phase 1 close-out

| Step | Work | Deciding check | Role |
| --- | --- | --- | --- |
| P1.1 (done) | Truncate gap text to at most 256 bytes on a character boundary in `bounded_gap`; regression built from the frozen counterexample | `cargo test`; the copied checkpoint probe exits 0 with one gap | worker |
| P1.2 (done) | D1 and D2: two sidecars, tail verification on reopen, directory sync after unlink, SIGTERM handling, marker-to-gap conversion, regressions for each; rerun the adapted journal oracle and the fault tests | oracle and fault tests exit 0; `kill -TERM` during `run` leaves a reopenable spool | implementer; prover for the reopen property |
| P1.3 | Register the log budget in the [node view](architecture/node.md): 64 KiB of bodies per cycle and 128 lines per file per pass in sorted order; report per-file unread bytes in the cycle line and inspect so lag is visible | docs check; a test with a busy first file shows the second still progresses | worker |
| P1.4 | Post-repair native run 02: same protocol, three seeds, final binaries | runner exit 0; VmHWM at most 64 MiB; exact replay | runner |
| P1.5 | Cross-family re-review on the final hashes: Claude verifier now, GPT after 2026-10-03 or Gemini; then mark phase 1 promoted in the ledger | both verdicts recorded with hashes | verifier |

## 4. Phase 2: network delivery

Order inside the phase: oracle first, then implementation, then verifier probes, then the cross-family gate review.

| Step | Work | Deciding check | Role |
| --- | --- | --- | --- |
| 2.0 | Delivery oracle from the [phase-2 oracle brief](experiments/benchmarks/data/alpha-review/parent-notes/phase2-oracle-brief.md.txt): a dependency-free Python checker over a JSONL transcript (source batches with exact bytes, attempts, ACKs, rejections, recovered records), with mutation controls that must fail | its own tests, including dropped-ACKed-record, duplicate, replaced-bytes and early-forget controls | oracle-author, before any server code |
| 2.1 | Workspace and `crates/fabric-server` skeleton: config file (listen address, certificate and key paths, state directory, admin credential file, journal cap), TLS listener, `/v1/health` | handshake with the pinned CA succeeds; plain HTTP and an unknown CA are refused | worker |
| 2.2 | Server batch journal: the `FAB1` frame reader and writer generalized to hold batches from any node, with identity checks moved to the server layer; a single commit thread with group commit (50 ms or 1 MiB) and an individual mode reachable only from the benchmark example | fault tests as in phase 1; both modes have the same two-sync boundary per group | implementer |
| 2.3 | Dedup and ACK per D4; state rebuilt by replay | restart tests over 10 identities; the TLA+ extension passes | implementer, prover |
| 2.4 | Node sender: after each cycle send the oldest unacknowledged batch, one in flight, bounded backoff; ACK cursor and reclaim per D5; credential file; keep collecting through an outage | 30 minutes buffered at the registered rate drains within 10 minutes after reconnect | implementer |
| 2.5 | Fault seams, test-only like the journal's: server drops the ACK after commit; node dies between commit and cursor write; torn server tail; injected I/O errors; harness SIGKILLs | the 2.0 oracle reports zero mismatches on recovered records | verifier probes |
| 2.6 | Ten real node processes under the runner with the registered source shape; ACK p50 and p99, CPU, RSS, all live bytes; grouped and individual commit measured with identical durability | ACK p99 at most 1 s; no growing backlog; queues byte-bounded | runner |
| 2.7 | Docs: delivery view gains the real ACK path, data-flow diagram, ADR-0012 and ADR-0013, ledger and current state | docs check | worker |
| 2.8 | Cross-family gate review on recorded hashes | both verdicts | verifier plus GPT or Gemini |

## 5. Phase 3: central control

| Step | Work | Deciding check | Role |
| --- | --- | --- | --- |
| 3.1 | Server state file (protobuf, atomic replace with sync): node name, credential hash, status (active, paused, revoked), desired and applied configuration revisions, last seen. Admin API behind the admin token: enroll (returns the node token once), list, set configuration, pause, resume, revoke | restart preserves state; a revoked token gets 401 on the next send | implementer |
| 3.2 | Node configuration poll on every cycle with the revision as ETag; validate with the existing `Config::validate`; write `applied.conf` atomically; keep the last valid configuration on any error; report the applied revision with the next batch. Pause stops collection and sending and records a gap on resume | invalid, stale, offline and server-restart tests; live reconfiguration | implementer |
| 3.3 | `fabricctl` node add, list, config set, pause, resume, revoke over HTTPS; inspect stays local | CLI tests against a live server | worker |
| 3.4 | `node-sim` example: one process speaking the real protocol for N identities with the registered open-loop workload, reused for every fleet tier later | 10, 100 and 1,000 identities enroll and apply a change | implementer |
| 3.5 | Gate: healthy apply at most 30 s at each tier; docs and ADR if the state-file choice needs one | measured under the runner | runner, verifier |

## 6. Phase 4: retained history

| Step | Work | Deciding check | Role |
| --- | --- | --- | --- |
| 4.0 | Exact-scan oracle: an independent reader of the raw batches table that answers the registered queries by full scan | its own mutation controls | oracle-author |
| 4.1 | Segment format: one Zstd Parquet `batches` table (identity, sequence, received time, hash, exact envelope bytes) for reconstruction, plus projected `logs` and `metrics` tables; a manifest with per-file SHA-256 written last with directory sync. A spike measures server RSS with parquet before committing to the layout | manifest-less directories are removed at startup; the spike stays under the 2 GiB gate | implementer |
| 4.2 | Sealing: a background task seals a contiguous journal range every 60 s or at 64 MiB; journal files are deleted only after the manifest is durable | crash-during-seal test | implementer |
| 4.3 | Retention: keep the newer of 24 h and 20 GiB; delete whole segments; report the actual retained boundary in every query answer | retention test within the live-data budget | worker |
| 4.4 | Query API and CLI: log search by host, time and substring; metric history; counter-aware rate using start-time resets; sealed segments plus the unsealed journal; answers carry scan completeness, node freshness, collection gaps and retained window; pagination bound to a snapshot (segment set and journal end offset in the page token) | 4.0 oracle matches on every registered query; missing or corrupt segment marks the answer incomplete | implementer |
| 4.5 | Indexes: Parquet row-group statistics only. Add a token index only if the 1,000,000-record p99 gate fails, with exact-scan fallback when it is missing or corrupt | registered queries p99 at most 2 s over the fixture | runner |
| 4.6 | Journal-only versus segment-backed reads with all live bytes; docs, storage and query views, ADR for the segment format | measured; docs check; gate review | runner, verifier |

## 7. Phase 5: qualification and packaging

| Step | Work | Deciding check | Role |
| --- | --- | --- | --- |
| 5.1 | Reproducible build: `rust-toolchain.toml` pinned to 1.98.0, `cargo build --release --locked`, fixed source date, checksum file | two clean builds produce equal hashes | worker |
| 5.2 | `packaging/`: the sysusers file, two units, the slice and `fabricctl`, built into a `.deb` with `dpkg-deb` and a postinst that runs `systemd-sysusers` and refuses an unexpected existing `fabricolly` account; purge semantics in postrm. Exactly the directives in the [installation contract](ALPHA.md#linux-installation-contract-phase-5-not-yet-implemented) | `systemd-analyze verify`; the installation acceptance list run on a disposable systemd host with throwaway state paths | worker, verifier |
| 5.3 | Fleet tiers 10, 100 and 1,000 with `node-sim` and one real server; outage and recovery; one real node buffering 30 minutes; 5x bursts; overload, auth rejection and concurrent management plus query; a soak within the 2 h invocation limit | every gate in the [decision rule](ALPHA.md#frozen-qualification-workload-and-decision-rule) | runner |
| 5.4 | CLI and API reference, recovery guide, examples, checksums; ledger and current state; independent executable review across families; tag `v0.1.0-alpha.1` only after every gate has a recorded command, exit and evidence | ready check | verifier plus GPT or Gemini |

## 8. Cross-cutting rules

- Every measurement runs under the existing [runner](../tools/alpha/runner.py); it gains a multi-process manifest (one server, N nodes) and nothing else.
- Each increment is one commit with its tests, diagrams and docs. Never rewrite history. Push after each gate.
- Roles: oracle-author writes graders before the implementation of each phase; implementer takes journal, dedup, sealing and query code; worker takes CLI, configuration and docs; verifier probes every gate; prover settles D1 and D4. At most four agents at once, no fast mode, no workflow fan-out unless the owner asks.
- Evidence stays compact: manifests, hashes, exits, histograms and minimized counterexamples under `docs/experiments/benchmarks/data/alpha-*`; bulk data under ignored `target/alpha-*`. Copy reviewer verdicts into the tracked directory at each gate so `cargo clean` cannot erase them.
- Non-goals for the alpha: a general OTLP receiver, traces, a UI, SQLite, `rcgen`, an async runtime in the node, inotify watchers, plugin boundaries, dynamic users, polkit helpers, and any index beyond row-group statistics unless a gate fails.

## 9. Risks

- WSL2 and ext4 sync behavior is the only tested environment; power loss stays untested. The I/O controller may be ineffective on WSL; report it, never count it as enforcement.
- One host runs the 1,000-identity tier, the server and the harness; CPU contention distorts p99. Record host state and keep tiers sequential.
- parquet 60 raises server build time and memory. The 4.1 spike decides before the format is fixed.
- GPT review is unavailable until 2026-10-03. Gemini is the fallback second family; a review with only one family is recorded as such.
- Scope creep: any new configuration key without a gate consumer is a defect.

## 10. Sequence and checkpoints

1. Owner answers D1 and D2. P1.1 to P1.3 land in one commit each, then P1.4 and P1.5 close phase 1.
2. Phase 2 in the order 2.0 to 2.8; the workspace change and ADR-0012 are the first commit.
3. Phase 3, then phase 4 with the 4.1 spike first, then phase 5.
4. Each phase ends with a checkpoint like the current one: ledger row, evidence copied, cross-family verdicts recorded, before the next phase's oracle is written.
