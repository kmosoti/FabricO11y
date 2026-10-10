# Server-owned Spindle correctness investigation

Owner scope: every production server launch owns a dedicated Spindle; both
components observe their operational data and deliver locally. This is a new
capability investigation, not an extension or reset of the completed pressure
experiment's allocation and not installation/performance qualification.

Before measuring, implement CLI lifecycle ownership, bounded diagnostic files,
durable local credential provisioning and normal authenticated HTTPS delivery.
Keep wire encoding, Batch identity, sync order, independent oracles and numeric
installed cgroup limits unchanged. The lower-level library serve primitive is
an explicit composition boundary. Use no recursive collector creation.

Correctness checks: exact diagnostic log bodies reach a real local server through
its actual companion; process samples are queryable; own diagnostic source remains
selected across remote configuration changes; duplicate process launch is refused;
restart reuses token/Spool identity; wrong/revoked credentials are not overwritten;
missing executable/invalid TLS leaves no child; unexpected child exit terminates
the server; normal stop and abrupt parent exit leave no live orphan. File tests
cover fixed storage bounds, rotation, forbidden file types and unsafe paths.
Include absent/nonmatching logs as query controls; exact answers matter more than
timing. No claim of a p99 or overhead improvement from these tiny fixtures.

All builds/tests/validators run serially inside tools/resource_group.py (20 GiB,
no swap, at most 30 minutes per invocation); build/temp paths stay on the mounted
data drive. The native service smoke has a 4 decimal GB cgroup covering server
and child, CPU/task limits and a finite five-minute workload deadline. Keep
fixtures below 128 MiB and total storage below the owner's 100 GB ceiling.
Retain commands, exact exits, source identity, assertions, resource/cleanup
receipts and any failures. Clean owned scratch only after preserving failures.
