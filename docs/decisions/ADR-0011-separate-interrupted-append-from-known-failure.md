# ADR-0011: Separate an interrupted append from a known journal failure

## Status

Accepted on 2026-09-27 for the alpha `FAB1` journal (plan decision D1). This revises one sentence of the [alpha safety contract](../ALPHA.md#safety-contract). FOL2 is unchanged.

## Context

The phase-1 journal synced one `recovery-required` sidecar before writing any frame byte and removed it after commit; reopen refused while it existed. Any process death inside the append window therefore made the spool unopenable, although the node had not advanced its source cursor and nothing had been sent. A [kill probe](../../tools/alpha/kill_probe.py) reproduced this on the checkpoint binary at its second killed iteration. The same probe found that a kill between creating the identity file and the journal file on first open left a permanently refused spool. Phase 5 requires graceful restart and retained spool, and systemd can deliver SIGKILL after a stop timeout.

## Decision

Use two sidecars. `append-in-progress` is synced before any frame byte and removed, followed by a directory sync, after the commit marker and directory syncs. Reopen with only this sidecar verifies the tail as it already did for an unmarked tail: frames with a valid commit marker are kept, a plausible incomplete suffix is truncated, corruption is refused, and the sidecar is then removed. `recovery-required` is written only after a write, sync or sidecar call reports an error. Reopen still refuses it until a rebuild from a retained source. Inspection reports `interrupted_append` for a dead writer's in-progress sidecar and `recovery_required` for a recorded failure. First open creates the journal file before publishing the identity by synced write-to-temp and rename, so an empty journal without an identity is a fresh spool. `fabric-node run` also stops between cycles on SIGTERM or SIGINT.

The ordering argument: the data sync completes before the commit marker is written, so a persisted marker implies persisted data unless a sync reported an error, and a reported error takes the known-failure path.

## Alternatives considered

- Keep one pre-written sidecar and add a manual `fabricctl recover` doing the same tail check. It runs the same code with worse availability, and a restart gate would depend on kill timing.
- Rely on SIGTERM handling alone. It does not cover SIGKILL, OOM kills or power loss.

## Evidence

The tracked unit test `process_death_at_each_append_stage_reopens_to_a_verified_prefix` simulates death after each sync stage without running the error path; `reported_sync_errors_quarantine_and_record_known_failure` injects a reported error at each stage. The kill probe on the pre-change binary (`fabric-node` SHA-256 `d7f9ff1de103b0488deb2e41e2c0ca8eeb2c859fe87cf01328e746b6bdc07599`) exited `1` with `recovery_required=true` at iteration 2. On the changed binary (`f0293a2d9d1733e4ad8b041e5fd0eea7da02984e16a541feb10d31061b1b6d41`) seeds 7, 11, 12 and 13 each killed at least 291 of 300 collects, observed between 73 and 150 interrupted appends, and replayed all 300 lines exactly once. After two small follow-up edits (preserving both errors in the double-fault path and a signal-handler cast), the committed `fabric-node` SHA-256 `1610d5e008247c730edc7602a2f7a302661aff1cbfe77db0c41d20cde99ff251` reran seed 7: 298 kills, 132 interrupted appends, 300 of 300 lines exact. The independent adapted journal oracle still passes unmodified: six API cases, the corruption matrix and all 72 length mutations.

## Consequences

A killed node restarts without operator action, and graceful stops never interrupt an append. The residual double fault is new and explicit: if a write or sync reports an error and recording `recovery-required` also fails, the knowledge survives only in the returned error; `fabric-node` then exits nonzero with both errors on stderr. If the process is restarted before an operator reads that, reopen treats the state as an interruption. Physical power loss remains untested.

## Validation

Falsify by a reopen that keeps a frame whose data sync reported an error, a reopen that refuses after a clean process death, or a kill probe run that loses or duplicates a line. The kill probe and both unit tests are the executable checks.

## Related

[Storage view](../architecture/storage.md), [node view](../architecture/node.md), [ADR-0006](ADR-0006-use-framed-local-log.md), [completion plan](../ALPHA-PLAN.md).
