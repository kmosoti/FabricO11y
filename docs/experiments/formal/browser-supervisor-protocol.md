# Dedicated browser ownership within an R3 campaign

Prospective registration; no source/native result or acceptance is claimed.
This extends [the controlled session protocol](console-session-clock-protocol.md)
and retains its tested browser ownership functions and ECHILD admission.

The R3 controller has other live campaign children. It cannot itself use the
browser controller's initially empty wait domain. A dedicated Python supervisor
starts before its browser child, establishes the existing isolated subreaper
domain, and invokes the existing `browser_group_launch`,
`stop_browser_groups`, `browser_reap_domain` and `end_browser_domain` functions.
It never accepts arbitrary PID/PGID signal authority.

The parent derives its actual original `fabric-work-*.service` ancestor from
kernel cgroup placement and retains that ancestor's no-follow directory FD and
identity. Its actual aggregate memory limit must be at most 6 GiB with no swap;
high, CPU and task limits remain those of the registered R3 envelope. It creates
one new descendant exclusively and retains its descriptor. This descendant is
bounded to 4,000,000,000 maximum/3,500,000,000 high bytes (kernel page rounding),
CPU 200%, 512 tasks and no swap. Before controller delegation its only process
must be the fresh supervisor. Its `fabric-work-browser-controller-<random>` name
preserves existing helper syntax; authority comes from actual ancestor identity,
exclusive creation, retained descriptors and verified placement, not that name.
The existing `enter_group.py` exec wrapper performs placement; no threaded
parent uses `preexec_fn`. Nested delegation must use the same inherited
controllers and leave the original aggregate cap intact.

One private inherited SOCK_SEQPACKET pair carries bounded JSON ready/stop/closed
messages, each at most 8 KiB. Ready means the supervisor has actually acquired
and bound the driver subgroup/identity. It does not mean WebDriver is ready;
the bridge still probes its actual `/status`. Startup, EOF, malformed messages,
driver exit and the run's original absolute monotonic deadline enter the same
bounded cleanup. The supervisor's ready and cleanup receipts contain no tokens,
cookies or response bodies. The parent bounds startup at 10 seconds and normal
shutdown at 20 seconds and the caller's shutdown-phase deadline, whichever is
earlier, without extending the R3 deadline. The stop packet may only tighten the
original absolute deadline; both retained parent records use that tighter bound.
After expiry, owned-domain kill/reap attempts are failed cleanup, never credited
as completion of the expired phase.

Only a clean supervisor exit plus its separate populated-zero, exact child
absence and own-child ECHILD observations can establish browser cleanup.
The parent independently rechecks the retained domain identity and population.
If the supervisor dies or cannot publish complete cleanup, the parent kills
only its held owned descendant through cgroup.kill, reaps its exact supervisor,
retains scratch and fails the campaign. This fallback is not a browser-only
cleanup success. R3 must stop all other campaign children and close its root
owned wait domain before checkpoint/removal; populated zero alone cannot prove
that adopted zombies were reaped.

Register separate source and delegated native controls before execution. Native
controls cover normal driver/descendant cleanup, startup refusal, supervisor
death before ready, and death during cleanup. An independent fixture guardian
owns the complete test wait domain and verifies exact descendant absence and
ECHILD even when the supervisor cannot complete. Retain failures and resource
receipts. Existing thirteen source controls and four native cleanup cases must
retain their exact outcomes at the resulting source revision; their historical
receipts remain bound to their original revisions.

Harness-only deterministic `before-ready` and `during-cleanup` fault points
publish a bounded PID/start marker and SIGSTOP the supervisor itself. The
independent fixture guardian authenticates that identity with its own pidfd and
kills it. The second point follows subgroup stop and precedes late-child
adoption draining, exercising the distinction between kernel population and
reaping. Ordinary bridge callers select no fault point.

This registration does not change product authentication, package bytes,
session expiry rules, polling continuity, R3 deadlines or acceptance gates.
