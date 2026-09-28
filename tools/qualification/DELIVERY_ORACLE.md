# Phase-2 delivery oracle

`delivery_oracle.py` is an implementation-independent checker for the D4
delivery/dedup rule and the safety contract in the [product contract](../../docs/PRODUCT-CONTRACT.md).
It knows nothing about the Rust wire format or any server code; it only
compares exact bytes the transcript itself asserts, using SHA-256-grade
byte equality (Python `bytes.__eq__`), never a parser from the
implementation under test.

The transcript is a small test protocol, explicitly **not OTLP**. It is a
sequence of independently observed facts a test harness must gather from
real processes (a node spool, the wire, a fresh server process reading its
durable state) — never from each other.

## Format

One JSON object per line (JSONL), UTF-8, no line over 2 MiB. Duplicate JSON
keys, unknown fields, missing fields, an unknown `type`, `NaN`/`Infinity`,
invalid base64, a `node_id` that is not 32 lowercase hex characters, a
non-integer/negative/>2^64-1 `sequence` or `generation`, empty `bytes`, any
record after `end`, or a transcript missing `end` are all malformed input:
the CLI exits 2. A stream is identified by `(node_id, generation)`, matching
the `Batch` envelope identity in [envelope.rs](../../crates/fabric-frame/src/envelope.rs)
(`node_id`: 16 bytes / 32 hex chars; `generation`, `sequence`: `u64`).

| type | fields | meaning |
| --- | --- | --- |
| `source` | `node_id, generation, sequence, bytes` | the node's own spool durably holds this batch, observed independently of the server. One record per identity. |
| `attempt` | `node_id, generation, sequence, bytes, injected_conflict` | a send attempt with the bytes actually placed on the wire. `injected_conflict: true` marks a harness-injected mismatch test. |
| `response` | `node_id, generation, sequence, kind[, committed_through]` | the server's answer to an attempt. `kind` is one of `ack, conflict, gap, unauthorized, bad_request, too_large, unavailable, no_response`. `committed_through` (`u64`) is required iff `kind == "ack"` and forbidden otherwise. |
| `fault` | `label` | an injected fault (`server_kill`, `node_kill`, `drop_ack`, `disconnect`, `io_error`, `torn_tail`, ...), free text, 1..=256 UTF-8 bytes. Informational: it establishes nothing on its own; its effect must show up in a later `node_state` or `recovered` record. |
| `node_state` | `node_id, generation, ack_cursor, retained_sequences` | a point-in-time observation of the node spool (after a restart, or at the end): the highest sequence the node believes acknowledged, and the exact set of sequences still retained. |
| `recovered` | `node_id, generation, sequence, bytes` | one logical record read back from a **fresh** server process after its final restart. |
| `end` | (none) | must be the last record; exactly one per transcript. |

Blank lines are harmless separators, never records. `retained_sequences`
must not repeat a value (a set, written as a JSON array of `u64`).

## Rules checked

All rules assume successful fsync semantics, only declared process-crash and
retry faults, and fail-closed known I/O errors; none of this proves
behavior under physical power loss.

| ID | Statement |
| --- | --- |
| `ACKED-DURABLE` | Every identity whose sequence is `<=` the highest `committed_through` ever ack'd for its stream appears at least once in `recovered` (bytes checked by `NO-FABRICATION`). |
| `NO-FABRICATION` | Every `recovered` entry has a matching `source` identity with identical bytes. |
| `NO-DUPLICATE` | At most one `recovered` entry per identity. |
| `CONFLICT-NOT-REPLACED` | An `injected_conflict` attempt whose bytes differ from `source` must never be covered by an `ack` (checked against the stream's highest `committed_through`). |
| `EXACT-RETRY` | A non-conflict attempt's bytes equal the `source` bytes for that identity; an attempt with no matching `source` is also flagged here (nothing to retry against). |
| `NODE-RETAINS-UNACKED` | At each `node_state`: (a) `ack_cursor` never exceeds the highest `committed_through` actually observed (in transcript order) for that stream by that point — an early/over-claim is a violation; (b) every sequence already sourced (as of that point) but not yet observed as ack'd must be in `retained_sequences` — an early forget is a violation. |
| `IN-ORDER-ACK` | Per stream, `committed_through` never decreases across `ack` responses in transcript order, and never exceeds the highest sequence ever sourced for that stream. |
| `REJECTED-NOT-COMMITTED` | Documented policy, not a separate check: an identity whose only responses are `unauthorized`/`bad_request`/`too_large` (never covered by an `ack`) may be absent from `recovered`; if present, `NO-FABRICATION` still applies. Chosen because the transcript alone cannot distinguish "never accepted" from "accepted then never reported"; presence-or-absence is left open, exactness is not. |
| `GENERATION-INDEPENDENT` | Not a runtime check: every rule above keys on `(node_id, generation)`, so two generations for the same node never interact. Exercised by a dedicated test, not a violation path. |

Unacked identities may or may not appear in `recovered` depending on crash
point; if they do appear, `NO-FABRICATION` still requires exact bytes.

## Open questions the adapter must resolve

- **Zero `generation`/`sequence`.** `Batch::validate` in `journal.rs` rejects
  `generation == 0` and `sequence == 0`; this transcript format only enforces
  the `u64` bounds, not that stronger rule, because the transcript is
  wire-agnostic. **The adapter must never emit `generation` or `sequence`
  0**; the oracle will not catch it if it does.
- **Attempt/response adjacency.** The oracle does not require a `response`
  to immediately follow its `attempt` in the transcript, only that both
  reference the same identity and that transcript order is preserved. If a
  future rule needs strict pairing (e.g. to bound in-flight concurrency to
  one batch per D4), **the adapter must add an explicit correlation field**
  (for example an attempt id echoed on the response).
- **Fault correlation.** `fault` records are informational only; the oracle
  draws no conclusion from a fault's presence or label. If a claim like
  "recovery happened within N seconds of this fault" is ever needed, **the
  adapter must add a timestamp field to `fault`, `node_state`, and
  `recovered`**, since this format carries no wall-clock time.
- **Reclaimed-but-acked retention.** Nothing requires a `node_state` to
  retain sequences at or below the observed `committed_through` (D5 spool
  reclaim may delete them); nothing forbids it either. This is left
  unconstrained deliberately.
- **Extra retained sequences never sourced.** A `node_state` may list a
  retained sequence with no matching `source` record; the oracle does not
  flag this (it would indicate a harness or spool bug, not a delivery
  violation) and no ID is assigned to it.

## Adapter contract (summary)

A real harness must, per fault scenario: (1) read the node spool before any
send and emit one `source` per durably-committed batch with its exact
bytes; (2) log every wire attempt as `attempt`, flagging harness-injected
byte mismatches with `injected_conflict: true`; (3) log the server's literal
answer as `response`; (4) after every restart or fault injection relevant to
the property under test, re-open the node spool and emit `node_state` with
the true `ack_cursor` and retained-sequence set; (5) after the **final**
restart, start a **fresh** server process, read only its durable state, and
emit one `recovered` per logical record it holds; (6) terminate with `end`.
No field may be inferred, rounded, or copied from the implementation; every
field is a direct observation.
