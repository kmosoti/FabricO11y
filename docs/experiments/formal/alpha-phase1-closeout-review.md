# Phase-1 close-out review and repairs

Status: one executable review returned REQUEST_CHANGES on 2026-09-27 for head `8703847`. All six findings were reproduced in the current code, repaired with regressions, and each regression fails on a mutant that restores the defect. A re-review of the repairs is pending. Phase 1 is not promoted.

## Reviewers

- **Claude verifier (Opus 5.5).** The author of these changes is also Opus 5.5, so this is same-model review, not the independent half. It built an LD_PRELOAD shim that SIGKILLs a process before or after the Nth `write`, `fsync`, `unlink`, `rename`, `ftruncate` or `open` on a matching path, or makes the call fail with a real EIO. Probes and the shim source are kept under [claude-shim-probes](../benchmarks/data/alpha-review/phase1-closeout/claude-shim-probes/). Across 48 kill points in a second append, 64 in first open, EIO at every append stage, 600 double kills, lock contention, starvation and multibyte gap boundaries, it found the six defects below and no others.
- **Gemini (`agy`, gemini-3.1-pro-high).** The run produced no verdict. Headless mode auto-denied every shell command and the result has no structured output; the [raw output](../benchmarks/data/alpha-review/phase1-closeout/gemini-denied.json) is kept. Enabling it needs an allow-rule in its settings, which is the owner's decision.
- **GPT.** Unavailable until 2026-10-03 (usage limit).

The [review contract](../benchmarks/data/alpha-review/phase1-closeout/contract.md.txt) lists the claims that were tested.

## Findings and repairs

| Finding | Severity | Repair | Regression and mutant |
| --- | --- | --- | --- |
| A kill or write error while creating `coverage-unknown` left an empty file; every later cycle failed with "invalid coverage-unknown marker" | major | The marker is written by synced rename; any existing but unreadable marker is reported as coverage unknown "since an unrecorded time" | `empty_coverage_marker_is_reported_as_unknown_time_and_collection_continues`; fails when an unreadable marker is fatal |
| A reported fsync error while reopen settles an interrupted append was not recorded, so the next reopen kept the frame | major | While `append-in-progress` exists, any error from open's directory sync, truncation, file sync or sidecar removal writes `recovery-required` | `reported_error_while_settling_an_interrupted_append_is_recorded` through a test-only seam; fails when the error is not recorded |
| `fabricctl inspect` reported zero backlog for a truncated or rewritten same-inode file | minor | Inspect and the reader share one cursor-validity check; an invalid cursor counts the whole file | `inspect_backlog_counts_a_truncated_same_inode_file_in_full` |
| `collect_once` returned an error after committing when the marker could not be removed, with stale in-memory cursors, so a retrying library caller collected lines twice | minor | In-memory state is updated right after the commit; a failed removal is retried next cycle without repeating the notice | `unremovable_marker_is_reported_once_and_committed_lines_are_not_collected_twice`; fails when removal failure is fatal |
| A kill between committing the notice and removing the marker repeated the notice in the next batch | minor | At open, a timestamped marker whose notice is already in the newest batch is removed without reporting again | `marker_left_after_its_notice_committed_is_not_reported_again`; fails without the open-time check |
| SIGTERM during startup replay ended the process by signal | minor | The stop handler is installed before the spool is opened, and a stop during startup exits 0 | covered by the existing SIGTERM child-process test; no separate startup-timing regression |

A failure that follows an already-reported notice now starts a new interval, so the second failure is not hidden behind the first notice.

## Remaining limits

The double fault in [ADR-0011](../../decisions/ADR-0011-separate-interrupted-append-from-known-failure.md) remains: if a reported error occurs and recording it also fails, only the process's error output carries it. An unrecorded-time notice is always reported and never deduplicated at open, because it is not unique. Physical power loss is untested.
