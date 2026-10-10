# Production access adapter for the registered soak and outage campaigns

Prospective registration, 2026-10-10. This adds an explicit production access
adapter to [soak revision 2](soak-protocol-r2.md) and the
[30-minute outage protocol](alpha-phase5-outage-protocol.md). Historical legacy
runs keep their original meaning. No campaign result is claimed here.

The adapter uses the exact linked Debian/RPM candidate receipts and matching
three uninstalled qualification helpers, verifies their bytes, and extracts an
owned immutable payload. It starts normal `fabric-server serve` with the exact
packaged console assets. A real Chrome virtual CTAP2 authenticator registers the
owner through production WebAuthn. Owner cookie controls enroll sources and
perform management; a separate workload credential permits only scoped
inventory and telemetry reads. There is no shared-master production fallback.
This proves neither physical authenticator behavior nor deployment qualification.

Keep the original workload, seeds, durations, warmup, measurement windows,
cadences, journal sizes, default retention, CPU allocation and every delivery,
latency, RSS, backlog and drain threshold. Soak retains all original gates and
its strict independent companion custody check. Outage adds that same companion
custody prerequisite because normal serving includes a dedicated Spindle;
source evidence comes from its stopped Spool, never from server recovery.
The independent delivery oracle is unchanged. No console rate endpoint is used.

During the soak the visible production UI tails logs, and production owner
management refreshes user verification through a real passkey ceremony when
needed. During the outage the UI is paused before shutdown and remains paused;
after restart the same retained authenticator performs explicit real login,
then visible polling resumes. The original drain clock starts immediately after
server restart and includes this login time. Short runs remain harness smokes.

Use an explicitly admitted finite launcher deadline suitable for the original
campaign (the default 30 minutes cannot contain the full soak). The combined
fixture has a 6 GiB maximum, zero swap; server plus companion retain the 4 GB,
two-CPU subgroup, and the browser has a 1 GiB subgroup. Keep the original runner
raw-data/evidence bounds. Candidate, browser and telemetry scratch together
have an additional 5 GiB live ceiling; admission stops at 95 GB aggregate DATA
usage. Preserve failed receipts and original oracle ledgers before removing
owned credentials, browser profiles and candidate state. Record actual subgroup
limits, peaks, memory events, empty groups and verified immutable input hashes.

Both harnesses accept `--production-access --deb-receipt PATH --rpm-receipt
PATH --production-out DATA/results/NAME`; soak also requires `--companion`.
Legacy mode must be explicitly labeled and uses `serve-legacy`; its results are
not evidence for production access. Before full trials run boundary controls,
a genuine finite bridge smoke and a finite adapter smoke. Recheck exact package
and helper identities after cleanup. All checks and trials use the resource
launcher. A skipped, interrupted or failed prerequisite is not a passing cell.
