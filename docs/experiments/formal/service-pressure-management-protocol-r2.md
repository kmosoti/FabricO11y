# Service pressure management schedule, revision 2

Prospective registration, 2026-10-12, before implementation or measurements.
This protocol-only amendment awaits independent pre-flight review. It addresses
[the admission finding on #43](https://github.com/kmosoti/FabricO11y/issues/43#issuecomment-6130785328)
in the [production service pressure protocol](service-pressure-protocol.md).
Earlier runs retain their original schedule, revision and outcome.

## Question and boundary

Can the registered burst cell complete every management round without
overloading its own console admission budget? Inventory GET and configuration
PUT each add a bridge session GET; the reader query is another request. Two
rounds per second therefore require at least ten protected requests/s before
visible PWA polling. The shipped private-route bucket admits eight/s with burst
eight, shared by this client's IP; its separate authentication bucket is two/s
with burst four. These are admission boundaries, not authorization rules.

Select **one round per second**, preserving both bridge session reads. Do not
share session snapshots, change CSRF/Origin/user-verification checks, manufacture
another peer, alter quotas or oracles, retry refusals or omit failed requests.

After setup and immediately before launching the simulator, perform one explicit
recorded owner login. Record its start, completion and returned freshness bound.
Any in-cell refresh still counts, and a missed slot still fails. This keeps setup
time outside the owner's 300-second freshness window for the measured cell;
it does not extend that window or suppress an automatic refresh.

## Schedule and coverage

Use absolute monotonic deadlines `anchor + 0.25 + k`, for nonnegative integer
`k`, with the existing local simulator-launch anchor and quarter-second offset.
Record scheduled deadline, actual start, scheduling lag, completion/exception,
inventory/configuration statuses and every query page. Keep both owner commands
and the reader query inside the round's original elapsed timer. Seeded node and
interval choices stay unchanged; bridge waits are included. No catch-up retry
repairs a failed round.

Grade against the simulator's independently reported actual begin time. A
successful complete round covers the slot
`floor((started_unix_ns - simulator_begin_unix_ns) / 1_000_000_000)`.
Require every slot in `[0, seconds)`: 180 in a full cell and 12 in the existing
smoke. A pre-measurement or drain-time round cannot fill a measured slot. A
partial/failed round supplies no successful coverage; every refusal/exception
still fails the existing management gate. Startup delay, missed slots and late
completion remain visible. Never substitute scheduled time for actual start.

A nominal round has five protected requests. The registered node-filtered
30-second log window contains at most 220 rows during the fivefold burst, below
the unchanged 1,000-row page limit. Read every page anyway: any continuation or
session refresh adds real traffic and time. Keep actual five-second visible-PWA
polling and record real passkey refreshes. A finite model establishes only its
declared schedule; native admission and measured coverage need separate evidence.

## Independent controls

Before native execution, fix literal request/coverage fixtures independently of
the management loop. For a three-second private-bucket model, start with eight
credits, refill eight/s, consume one/request and cap at eight. Include one UI
request at 0.10 s. Each one-second round has two requests at `k + 0.25` and the
other owner pair plus reader request at `k + 0.75`. All 16 requests must be
admitted. In the legacy fixture all five requests of round `j` start at
`0.25 + 0.5j` seconds. That half-second schedule must be rejected; its first
refusal is at 2.25 s. The production quota code is not the model's oracle.

For an independent two-second coverage fixture, these literal actual starts
are relative to the simulator's reported begin:

| Successful round starts (s) | Missing measured one-second slots |
| --- | --- |
| 0.25, 1.25 | none |
| 0.25 | 1 |
| 0.25, 2.25 | 1 |
| 0.25, 0.75 | 1 |
| -0.25, 1.25 | 0 |

Keep existing half-second controls and their expected outcomes as historical
controls; add separately identified one-second fixtures. Inject a missing
measured round and the legacy overrate schedule; the checker must reject both.
Real failed native smokes stay failed.

## Unchanged workload and admission

All other inputs and gates are unchanged: three seeds, 100 identities, 180
seconds, the second 60–79 fivefold log burst, fixed metric offers, 200 rejection
rounds, exact independent custody, no rejected source committed, original
backlog/drain rule, UI success, process exits and RSS limits. The existing smoke
remains 10 identities/12 seconds, a two-second burst and ten rejection rounds;
it is admission evidence only. Use exact frozen Debian/RPM receipts and
source-matched helpers.

Retain the launcher, serial execution, 6 GiB fixture/zero swap, 4 GB
server-plus-companion subgroup, separate browser group, unchanged CPU placement,
5 GiB live-data reservation and 95 GB aggregate admission stop. Outer deadline
stays 1,800 seconds; the approved outage/soak deadlines do not apply to stress.
Post the exact pre-flight plan on [#59](https://github.com/kmosoti/FabricO11y/issues/59)
before every heavy run. Existing recovery, storage, retention, process-kill,
TLS/auth and full-Spool controls stay unchanged.

Implementation follows this registration in a separate trust-boundary commit.
No stress run starts before independent pre-flight review (or the owner's
60-minute response window) and all candidate/prerequisite gates.
