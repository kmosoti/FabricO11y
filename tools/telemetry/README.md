# Passive Codex telemetry

This optional development tool records typed runtime observations for a future dashboard. It runs outside the Fabric O11y Rust application. It does not ask a model to generate status or read the event journal.

## Commands

Requires Python 3 on Linux (`fcntl.flock`), with no third-party Python packages. From the repository root:

```sh
python3 -B tools/telemetry/cli.py observe < hook-input.json
python3 -B tools/telemetry/cli.py snapshot --session SESSION_ID
python3 -B tools/telemetry/cli.py schema
python3 -B tools/telemetry/test_contract.py
python3 -B tools/telemetry/test_cli.py
```

`observe` reads one Codex hook JSON object on stdin and exits successfully with empty stdout and stderr, including on handled recording failures. It records a sanitized diagnostic in `.local/codex-telemetry/observer-errors.log` when possible. If the store itself is unavailable, even that diagnostic can be lost. The `snapshot` command prints JSON and leaves journal contents unchanged, but can create a lock sidecar; missing, invalid, or truncated journals produce an explicit nonzero exit and no snapshot. `--root DIRECTORY` selects an isolated store for either command.

The default store is anchored to this script's repository, not a hook payload's `cwd`. Each session uses a filename derived from SHA-256 of its ID. No session ID is interpreted as a path. These files are gitignored.

## Event contract

[The event JSON Schema](event.schema.json) describes version 1. [The implementation](telemetry.py) validates envelope and event-specific fields before recording. Unknown versions/types, extra fields, invalid IDs, naive/invalid timestamps, and Boolean exit codes are rejected. Fields are deliberately small. JSON Schema consumers should enable date-time format validation. Runtime validation additionally requires Python integers (so `1.0` is rejected even though JSON Schema treats it as mathematically integral).

| Hook | Event | Meaning |
| --- | --- | --- |
| `SessionStart` | `session.started` | Session start/resume observation |
| `SessionEnd` | `session.ended` | Session end observation |
| `UserPromptSubmit` | `turn.started` | Prompt submission observation |
| `Stop` | `turn.stop_requested` | A stop was requested; another hook may continue work |
| `Interrupt` | `turn.interrupted` | Interruption observed |
| `PreToolUse` | `tool.started` | Tool invocation requested; another policy hook may still block execution |
| `PostToolUse` | `tool.finished` | Tool response observed; underlying work may still be running |

The adapter retains only session/turn/tool-call IDs, tool name, its observation timestamp, and a recognized exit code. It drops prompts, arguments, output, transcripts, and assistant messages. `source: runtime_observer` records provenance by convention, not authenticated identity. Arbitrary clients with filesystem access can forge telemetry.

Only an exact integer `tool_response.exit_code` for canonical `Bash` is recognized. Every other response shape has `exit_code: null`, including textual success claims. A zero exit establishes command success only. It does not count tests, complete the task, or establish an architectural fact. Hook coverage is incomplete and runtime response formats can vary; see [official hook coverage](https://learn.chatgpt.com/docs/hooks#tool-coverage).

## Snapshot semantics

An empty journal returns an empty snapshot with `session_id: null`: no session evidence is available. Snapshots retain independent turns and operations. `started`, `finished`, `stop_requested`, `interrupted`, and `session_ended` are historical observations, not mutually exclusive live states. In particular, `session_ended: true` is not reset by a later resume. There is no inferred active/idle/complete state.

Start and finish flags are monotonic, so a delayed start cannot erase a known result. Identical event IDs are deduplicated. A known exit code refines an unknown (`null`) observation; a later unknown cannot erase it. Conflicting reused IDs, different concrete exit codes, or changed tool names for one operation fail replay explicitly. `last_observed_at` is the newest timestamp by instant, while `recent_activity` contains the last 50 unique events in journal receipt order. Observation timestamps describe hook execution, not precise tool duration or causal order. Their age does not prove liveness or failure.

An advisory file lock serializes appenders and readers, with a bounded acquisition wait. A reader sees whole records from cooperating writers or reports an invalid journal. Journals are append-only; writes are not a durable delivery protocol. Process or host failure can lose events or leave an incomplete line. Replay never silently discards a broken record.

## Runtime integration and limits

[Project hook configuration](../../.codex/hooks.json) keeps telemetry handlers separate from the existing context loader and documentation checker. Telemetry handlers return no `additionalContext`, decisions, or status messages. Background scheduling can reorder or cancel hooks; the journal is a partial observation, not a complete audit. `SessionEnd` is synchronous because Codex requires it.

After reviewing changed hook definitions, use `/hooks` in Codex CLI to trust them. File configuration and fixture tests do not establish that a live session has loaded or trusted the observer. We do not modify runtime trust state. See [official hook trust](https://learn.chatgpt.com/docs/hooks#review-and-trust-hooks).

Each raw hook input is capped at 1 MiB; oversized input is dropped with a best-effort diagnostic. Journals and snapshot replay memory grow with recorded activity; remove old session journals only after their writers have stopped. Automatic retention, a server, dashboard UI, semantic annotation tool, structured project-context migration, and architecture highlighting are future increments.

Tests establish event/replay behavior and silent handled failures. They do not establish zero runtime latency, live hook activation, or unchanged model coding quality. The current `CURRENT.md` loader still intentionally supplies project context; this telemetry tool does not change it.

See [the architecture view](../../docs/architecture/agent-telemetry.md) and [the decision](../../docs/decisions/ADR-0003-observe-agent-activity-with-typed-events.md).

## Independent schema check

The runtime and standard tests use only the Python standard library. The independent [schema probe](test_schema.py) additionally uses the pinned [test dependencies](requirements-test.txt). It checks the schema itself and compares accepted/rejected cases with the runtime validator. CI installs these in a temporary virtual environment. Locally, with `uv` available:

```sh
uv run --no-project --with jsonschema==4.23.0 --with rfc3339-validator==0.1.4 python -B tools/telemetry/test_schema.py
```

Two independent implementation candidates were sampled (N=2): candidate A passed all 15 original oracle tests; candidate B also passed all 15. An extended 88-case schema probe rejected both for accepting an invalid timezone offset (`+01:60`). Both passed after correction (2/2 passing the combined checks); candidate A was selected because it avoids rereading the whole journal during every append. Further review added regression probes for fractional timestamp precision, requested-session identity, and refinement of unknown exit codes. The independent schema check now has 90 cases, including the documented JSON Schema/Python integer distinction. This is correctness evidence, not a comparison of model coding quality or runtime performance.

The [formal evidence check](../../formal/agent-telemetry/README.md) separately verifies symbolic merge laws and a bounded reference comparison. It runs only on explicit invocation or in CI, never inside the observer.
