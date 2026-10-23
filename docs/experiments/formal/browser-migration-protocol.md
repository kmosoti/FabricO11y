# Packaged legacy administration migration supplement

## Console-window fixture correction

Registered before correction, 2026-10-23. Fresh exact-package Chrome row67 at
`4dc0022` preserved its legacy canary and stopped state, then received HTTP400
for the owner query. The harness reused its legacy discovery range
`[0, 2**64-1)` for the console. The existing console implementation and
operations guide limit those requests to24 hours. Preserve this failed row and
its original stopped receipts; it is not migration acceptance.

Keep the legacy discovery query and original canary unchanged. Once the single
original row is observed, derive the transition query from that row's integer
`observed_ns`, with3600 seconds on either side, clamped to the u64 domain.
Require the exact canary body and node, a representable timestamp strictly
below u64::MAX, a nonempty window containing the timestamp, the same node/body
filters and row limit2. Use that same valid query for the owner, reader and
packaged access CLI. The independent migration grader remains byte unchanged
and still requires the entire original row and no next page for every result.
No record may be dropped, rounded, reissued, edited or replaced.

Register `test_browser_migration_window.py` before the producer correction:
the old all-time query must fail the console-window assertion. The successor
must include the original timestamp in a window no wider than24 hours and
reject wrong canary/node, missing, noninteger, negative or unrepresentable
timestamps. Run the unchanged migration mutation controls and affected source
checks, then fresh complete Chrome and Firefox migration rows on the merged
harness. No API, authorization, workload or product/package change is allowed.

Prospective #45 supplement; no actual package result yet. Reason: native access
store tests do not establish migration of actual immutable package state.

Use one isolated ordinary-clock browser cell per existing admitted Chrome and
Firefox engine. Retain existing 240-second deadline, 4GB browser/fixture cap,
512MiB fixture, DATA-only paths, 1GiB storage reservation,95GB aggregate stop,
no swap, exact package/helper admission and independently observed child cleanup.
No package installation or owner service. Never combine migration with other
lifecycle, expiry, security, role, UI-response, defect or sustained-poll modes.

The caller snapshots exact packaged server, Spindle and fabricctl bytes. Run
serve-legacy with access/console settings absent on fresh owned state, use
protected fabricctl admin config to enroll migration-edge, and run that packaged
Spindle against a literal independently written one-line canary. The literal
query must yield exactly one matching row and no next page. Stop all legacy
processes/descendants using the existing kernel ownership observer, then retain
original stopped control/telemetry bytes and hash/path/device/inode inventory.
Keep the same state directory. Immediately before production startup verify
unchanged complete stopped file inventory. Original selected control/telemetry
files must still exist with their original prefixes after production owner
bootstrap; no filtering of full query responses. Refuse any unexpected reclaim
or conflict rather than calling it conservation.

Start ordinary serve using the same native server bytes and same state with the
configured console; actual resident-owner bootstrap follows the existing browser
flow. Old master bearer to console query must return401/sign in required;
legacy admin route must return404 (empty route fallback). Issue a real reader,
then owner, reader and packaged fabricctl access queries must return the identical
single original canary row (including source and observed timestamp). Preserve
unfiltered answers, original raw selected state and public command/executable
hashes; never archive token values/CLI enrollment stdout. Configs/tokens remain
protected scratch and are cleaned by the original caller.

Separate fresh owned state contains only the literal development access fixture
{"version":1,"principals":{}} plus independent telemetry sentinel. Run normal
serve, require exit1 and exact stderr line:
`fabric-server: access state predates the three-role format; stop the service, remove state_dir/access and re-bootstrap the owner`.
Require unchanged access fixture and telemetry sentinel, with no format fallback.
Unrelated startup/TLS/permission/nonzero failures never qualify this observation.

Independent source checker controls register: valid literal case; changed or
dropped/duplicated canary row; master401 with wrong error/429/accepted200;
format1 unrelated stderr/wrong exit; changed original prefix. Source checks are
finite synthetic checker evidence, never package migration. Implement producer
and two caller hooks separately from this protocol/oracle registration.
