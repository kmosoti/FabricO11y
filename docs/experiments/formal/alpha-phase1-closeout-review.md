# Phase-1 close-out review and repairs

Status: the first executable review returned REQUEST_CHANGES for head `8703847` with six findings. The re-review of head `b140bd2` confirmed all six repaired and found three more, all repaired below. Each regression fails on a mutant that restores its defect. The final re-review of `71fef99` returned APPROVE with no findings. That approval is same-model review; the independent-family review is still pending, so phase 1 is not promoted.

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

## Re-review of `b140bd2`

The same reviewer reran its reproductions against binaries built from `b140bd2` (`fabric-node` SHA-256 `169ca99e3cc6ca122793b037d4924f36b7a81691ff00d00b9f00b14361c32b84`). All six repairs held, including a new 44-case sweep of EIO at every reopen step after kills at six append points, 1,200 double kills and 48 append kill points. It found three new defects, one a regression from the spool rotation added in the same range. Its new probes are under [claude-shim-probes-rereview](../benchmarks/data/alpha-review/phase1-closeout/claude-shim-probes-rereview/).

| Finding | Severity | Repair | Regression and mutant |
| --- | --- | --- | --- |
| During rotation the writer's lock stayed on the renamed file, so a second writer that opened in the window created and locked a new active file; the first writer then quarantined the spool | major | The exclusive writer lock is held on `writer.lock`, which is never renamed; inspection takes its shared lock on the same file | `second_writer_is_refused_while_the_active_file_is_renamed_away`; fails when open does not lock `writer.lock` |
| After a kill between the rotation rename and creation of the new active file, inspection failed with "No such file or directory" | minor | Inspection treats sealed files without an active file as an interrupted rotation when no writer holds the lock, and retries when one does | same test, inspection half |
| An I/O error before the coverage marker's rename left only the staged file, and the next batch carried no notice | minor | A leftover `coverage-unknown.tmp` also means coverage is unknown, reported as "since an unrecorded time"; clearing removes both files | `staged_marker_left_by_a_failed_write_still_reports_unknown_coverage`; fails when the staged file is ignored |

Not probed separately: a failure that follows an already-reported notice replacing the marker, which needs an in-process append failure that does not quarantine the spool.

## Final re-review of `71fef99`

Verdict APPROVE, no findings, on `fabric-node` SHA-256 `dac2603c9c68560664c1c08bd408eb9909bcd4a4311f8773525fd9abd5030911` (the reviewer's own build). Writer B was refused at every one of rotation events 11 to 17 while writer A was delayed, and A never quarantined; all 2,240 lines replayed exactly. Kills that left only sealed files gave `interrupted_append=true` and exact replay at all 12 points. The staged-marker rows now carry an unrecorded-time notice. For the one-second log poll, a quiet run committed only its metrics batch, a partial line was committed only after its newline, and 50 random SIGKILLs of `run` left inspection clean with an exact 978-line replay. `cargo test --offline --locked --workspace --all-features` exited 0. The added probes `rotrace5.py` and `runkill.py` are kept with the others.

## Remaining limits

The double fault in [ADR-0011](../../decisions/ADR-0011-separate-interrupted-append-from-known-failure.md) remains: if a reported error occurs and recording it also fails, only the process's error output carries it. An unrecorded-time notice is always reported and never deduplicated at open, because it is not unique. Physical power loss is untested.
