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

R3 additionally supplies its already registered, empty 1 GiB/850 MiB browser
budget with its retained no-follow descriptor, identity and actual exclusive
creation observation. CPU 200%, 512 tasks and no swap remain enforced there.
The new 4 GB controller domain is nested beneath that budget and inherits its
stronger 1 GiB effective aggregate bound; the inner 4 GB value is never described
as the effective limit. Both ancestors, actual limits and placement are retained.
Changing or substituting the supplied budget must refuse before launch. No
browser process may move to the old sibling budget outside its owned domain.

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
controls cover normal driver/descendant cleanup in both the direct and preserved
1 GiB budget shapes, startup refusal, supervisor death before ready and death
during cleanup in the nested shape, plus wrong/changed ancestor refusals with
no launch or descriptor leak: seven controls total. An independent fixture guardian
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

## Production health-wait scope correction

Prospective harness regression, 2026-10-18. The retained Chrome #45 baseline
`results/replacement-tls-01-browser-chrome-baseline/browser.json` has SHA-256
`ab6439c87bb0cb35d8c4792d7e768d656c5642254c262604e26cf863441820c1`, exit 1,
154 recorded checks, and `NameError: name 'healthy' is not defined` after the
offline owner-recovery command. Its outer resource record is
`evidence/fabric-work-e82276d87ea2492e8ad596f8c6a0d9f0/.resource-command.json`
and reports exit 1. Preserve this failed baseline as partial evidence, not
acceptance. The production source at the failure remains unchanged.

The smallest correction moves the existing production TLS health predicate to
`main` scope and gives it an explicit process argument. Keep its dead-process
refusal, CA-backed opener, one-second request timeout, HTTP-200 predicate and
`OSError`/`URLError` refusal behavior. The initial server wait and the offline
recovery and shell-lifecycle restart waits must each close over the current
`server` process in `wait(lambda: healthy(server), label)` while retaining their
existing labels and 25-second default wait deadline. Do not capture the initial
process for a later restart.

Before changing the source, a Rust integration test must parse the actual Python
AST, extract the actual main-scoped helper and the three actual wait expressions,
and invoke those expressions using a stable fake TLS opener, fake process and
immediate fake wait. It uses no network, browser, server or fixture. Against the
current source, evaluating the actual restart wait must produce the observed
`NameError`; after the correction, require HTTP 200 true, non-200 and
`URLError` false, a dead process raising before any request, and both restart
waits probing the replacement process. This regression does not change product
behavior, acceptance criteria or independent browser/security oracles. A passing
source control does not repair or relabel the retained Chrome #45 result.

## Shared production fixture consumer mapping

Prospective integration correction, 2026-10-15: CandidateFixture must select the
supervised browser lifecycle for ordinary release-main/query consumers as well
as R3. When no explicit browser deadline is supplied, derive a finite monotonic
deadline at fixture entry using the actual launcher runtime environment (1800
seconds for the default launcher). An explicit R3 deadline remains the earlier
activation-based global deadline. Import the supervisor before the loaded-source
census and always pass the held freshly-created browser-budget descriptor. The
outer launcher remains the hard whole-tree backstop; this grants no additional
service runtime. Source controls cover default1800, explicit deadline preservation
and invalid/missing runtime refusal. The actual #42 consumer smoke remains a
separate prerequisite; a direct supervisor control cannot substitute for it.


## Hosted defect-receipt cleanup binding correction

Prospective trust-boundary correction, 2026-10-16: hosted run 38843945807 at
`d537086` reached the unchanged API-cache defect assertion with exit 1, then
failed its receipt checker because that checker still required the removed
cleanup prose. Preserve that failed run. The minimized observed receipt lives
at `tools/ci/fixtures/browser-api-cache-d537.json` with its original digest.

Both browser defect graders must retain their existing exact defect, status,
flag and prerequisite checks. Replace only the obsolete cleanup substring with
the current exact cleanup label plus nonempty unique positive PID/start-tick
bindings, all kernel-empty and children-reaped observations, no stop error,
matching complete integer process exits and own-child ECHILD confirmation.
Reject absent/false observations, duplicate bindings and the historical prose
alone. Do not change the injected defect or its expected assertion. Test the
observed hosted receipt and representative corruptions before rerunning CI.
A corrected offline grade does not relabel the original failed workflow green.
