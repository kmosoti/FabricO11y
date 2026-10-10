# Deployment pressure and recovery collection

Registered before pressure load. Requires successful mixed-signal correctness
and nested-cgroup preflight from the [signal addendum](lab-completion-signals-protocol.md).
Repeat its development/small180second mixed workloads under the same application
cgroups. This separate cell intentionally reduces server journal capacity to
1MiB/file64KiB, keeping retention1GiB, and Spool capacity to64KiB development or
1MiB per small node. It does not represent natural64MiB lifecycle behavior.

At60seconds stop the server for45seconds while producers and Spindles continue,
then restart and allow the remaining75seconds plus20seconds drain. SDK queue,
batching and timeout stay frozen. Preserve failed local responses, offered SDK
spans without a successful local HTTP response, collection errors/retries,
backlogs, journal/Segment observations and application cgroup charges. Every
successfully admitted trace byte must survive, and every server-ACKed Batch hash
must recover. Full file-log source hashes must recover because the fixture keeps
the source files intact. SDK spans lacking a successful local response are
reported separately, never silently treated as acknowledged loss or erased.
Ambiguous lost responses remain a limitation of this finite fault model.

Require no normal application crash/OOM, exact accepted custody and projected
answers after drain. Latency and rejection rates during pressure are descriptive;
these runs do not publish a steady three-signal capacity envelope. Data-drive
budgets, process cleanup and unchanged independent query oracle still apply.
