# ADR-0003: Observe agent activity with typed events

## Status

Accepted for the first passive telemetry increment.

## Context

A proposed local dashboard needs machine-readable observations without repeatedly asking Codex to narrate progress or feeding an activity log into its context. Task intent, runtime observations, and durable architectural knowledge have different evidence requirements.

## Decision

Use a versioned [JSON event contract](../../tools/telemetry/event.schema.json), a silent Python hook adapter, a per-session JSONL journal, and an explicit deterministic snapshot command. The runtime emits observations; the reducer does not infer task completion. Project knowledge remains in the existing documentation. Codex's coding responses retain their normal format.

## Alternatives considered

- Parse prose status messages: wording changes and optimistic claims could become false machine state; repeated status writing adds model work.
- Have Codex rewrite one shared status JSON: concurrent sessions can overwrite one another, and a delayed start can replace a completed operation.
- Load all telemetry into startup context: adds irrelevant history and does not establish that claims are true.

## Evidence and correctness argument

Assume cooperating processes use the same local Linux filesystem and file-lock protocol; session/turn/operation IDs are supplied correctly. The store serializes whole record writes and snapshot reads. The reducer partitions by session and turn, then combines per-operation start/finish observations monotonically. By induction, adding a late start never clears a finish or its exit result. Unknown exit codes carry no result evidence: a concrete code refines `null`, while `null` cannot erase a concrete code. Different concrete outcomes produce an error rather than selecting an arbitrary result. This merge is order-independent for compatible observations. These properties are exercised by the [independent contract probes](../../tools/telemetry/test_contract.py), including reordered traces and concurrent subprocesses.

There is no inference rule from `tool.finished`, exit zero, or `turn.stop_requested` to task completion. A snapshot therefore cannot manufacture that conclusion. This is a property of the output vocabulary, not proof that hooks see every operation.

The [official hook documentation](https://learn.chatgpt.com/docs/hooks) describes structured inputs and background execution. Hook output can still reach model context; this observer emits none. Runtime hook activation and any model-quality effect have not been measured by the direct script probes.

The [formal investigation](../experiments/formal/agent-telemetry-merge.md) adds symbolic merge-law checks and bounded implementation correspondence. Its assumptions and limits are explicit; it does not prove the whole hook pipeline.

## Consequences

Consumers can validate and replay observations without a model. Python uses only the standard library; there is no new Rust dependency or service. File locking is Linux-specific. Journals and replay memory grow until old sessions are removed. Crashes can lose observations or leave an invalid journal; reading fails explicitly. Raw content is discarded, so semantic task descriptions and exact test counts are unavailable in this increment.

## Validation

Run `python3 -B tools/telemetry/test_contract.py` and `python3 -B tools/telemetry/test_cli.py`. Invalid versions, malformed variants, conflicting identities, partial records, and unexpected raw payload fields must not turn into plausible success. A future runtime trial must distinguish configured hooks from trusted, observed execution and separately measure latency, injected context, and coding outcomes.

## Related

- [Telemetry architecture](../architecture/agent-telemetry.md)
- [Operational commands and limits](../../tools/telemetry/README.md)
- [Current project state](../CURRENT.md)
