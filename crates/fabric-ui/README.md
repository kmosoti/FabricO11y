# Fabric operator console

Rust/Leptos CSR composition root with native-testable presentation models and a
same-origin authenticated client. The initial screen requires an explicit choice
between a server connection and a deterministic synthetic demonstration. The
client never requests the legacy administrator bearer token.

The live client supports local passkey setup, invitation enrollment, login and
fresh verification; scoped logs/metrics/spans with exact integer timestamps;
opaque snapshot pagination; exact trace-ID and span-name filters; bounded metric
envelopes and actual span waterfalls;
polled log snapshots; effective status/inventory and Spindle controls; and explicit
passkey/session/workload/delegation/grant management. Server authorization remains
a separate effect boundary: showing a control never grants permission.

The [algorithm model](../../docs/architecture/console-algorithms.md) distinguishes
pure models from the live transport's concrete bounds. The
[art-direction flow](../../docs/architecture/console-art-direction.md) connects
operator tasks and evidence to layout; the
[release plan](../../docs/milestones/operator-console.md) owns integrated
security, accessibility and release acceptance. Implemented client controls do
not establish that those gates have run.

The wire client uses version 1, one local request at a time, a 15-second network
deadline and an 8 MiB streaming body cap before Rust JSON decoding. Query pages
request at most 200 rows and 24 hours. The client lowers its row count to the
current grant, labels presets with their actual granted window, and refuses
explicit ranges wider than that grant before fetching.
Unsupported rate queries stay unavailable. Rust retains unsigned timestamps and
integer counters directly, without JavaScript JSON number conversion. Charts use
approximate display coordinates and preserve exact values in the row table.
UTC row labels retain all nine fractional digits; their titles and expandable
fields preserve the exact unsigned nanosecond value. Exact query ranges, row
JSON and evidence JSON start collapsed so observations remain the primary view.
Evidence includes authorized retained bounds, snapshot, gaps, freshness,
unavailable data and completeness exactly as returned by the server.

Overview explicitly refreshes authorized sources and loads the dedicated
companion's recorded process observations. It does not infer current health.
Pipeline distinguishes source custody, delivery, journal commitment, sealing and
retained queries, showing only status values actually returned by the server.
The trace page's related-log action searches literal trace-ID text in the same
applied source and time window; it does not infer a relationship from absent text.

Log tail is a replaceable polled snapshot, capped at 200 rows and 262,144 UTF-8
bytes. Each successful poll replaces the prior page. The API does not expose a
complete generation identity, so the client does not infer cross-poll event
deduplication. Polling, pagination, retention and display eviction can omit rows;
this is neither a lossless stream nor a source-loss measurement. Polling starts
paused, suppresses overlap, skips hidden tabs and resumes its configured schedule
when the document becomes visible. An error requires explicit resume.

Logout, offline notification, cross-tab logout and known absolute session expiry
clear protected page-memory views. Cookies remain HttpOnly server-owned state;
no credential, telemetry, API response or offline action is persisted by the
client. One-time enrollment/workload/invitation secrets have an explicit clear
control and disappear when protected views lock. The service worker handles only
the explicit public shell asset list.

Changing a pending read's source, filter, time or signal aborts its transport and
invalidates its result. The local pending slot stays occupied until completion;
browser abort does not prove server work stopped. Sign out clears protected views
and broadcasts immediately even during a read, then queues one CSRF logout after
that transport finishes. Failed revocation remains explicit and is not retried
automatically. Signed-out recovery and installation help describes the protected
server procedure and trusted HTTPS requirement.

## Build and check

Run from the repository root using the mounted data drive and resource launcher.
Rust 1.99.0, `wasm32-unknown-unknown`, Leptos 0.8.22 and Trunk 0.21.14 are pinned.
The builder refuses existing output directories, records asset/source hashes and
CSP, and retains the 8 MiB public-shell build limit.

```sh
export CARGO_HOME=/run/media/kmosoti/data/FabricO11y/toolchain-cache/cargo
export RUSTUP_HOME=/run/media/kmosoti/data/FabricO11y/toolchain-cache/rustup
python3 -B tools/resource_group.py -- cargo +1.99.0 test --locked -p fabric-ui
python3 -B tools/resource_group.py -- cargo +1.99.0 check --locked -p fabric-ui --target wasm32-unknown-unknown
python3 -B tools/resource_group.py -- python3 -B tools/ui/build.py --out /run/media/kmosoti/data/FabricO11y/results/my-console-build
```

[Console staging](../../tools/packaging/stage_console.py) verifies the frozen build
receipt and creates the manifest/headers expected by the production server's
`console_dir`. Serve assets under `/console/`, WASM as `application/wasm`, and
never use SPA fallback for API/auth routes. Production requires trusted HTTPS.
The PWA caches only public shell assets; cache lifecycle across installed upgrades
remains a separate acceptance obligation.

The existing Firefox fixture harness checks the demonstration. The
[live browser harness](../../tools/ui/live_browser_check.py) stages a real frozen
build, starts the production TLS server and its dedicated Spindle, and uses a
virtual CTAP2 authenticator with user verification in a pinned Chrome. Its
isolated profile trusts only the exact fixture leaf SPKI; it does not install OS
trust or disable certificate checks globally. It uses a combined 4 GB cgroup cap,
a 512 MiB fixture-storage ceiling, and removes owned processes/TLS/profile state.
Screenshots and receipts retain the exact executed command, artifact identities,
observations and finite scope. Virtual authenticators do not qualify hardware,
synced passkeys or mobile OSs. Optional fixtures separately exercise shell
update/rollback, desktop PWA installation, offline owner recovery, bounded
polling and actual cursor expiry; their receipts establish only the assertions
that ran.

The [local Leptos patches](../../vendor/README.md) replace unmaintained paste with
an explicitly named pastey registry dependency; upstream source and licenses are
preserved. The [dependency policy](../../docs/dependency-policy.md) records the
reviewed permissive-license additions. Only executed workspace dependency checks
establish the gate for a particular lockfile and revision.
