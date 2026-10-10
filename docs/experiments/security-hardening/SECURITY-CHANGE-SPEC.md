# FabricO11y security hardening: change specification

**Specification:** FABRIC-SECURITY-HARDENING-2026-10-10  
**Repository:** `kmosoti/FabricO11y`  
**Reviewed base:** `18f6b367d0f34886847ea25b9168e50eb0425300`  
**Status:** Proposed implementation contract. No repository changes, pull requests, cloud scans, application builds, or application test runs were performed for this document.  
**Suggested branch:** `milestone/security-hardening`  
**Primary audience:** Implementing agent, independent verifier, and repository owner.

## 1. Decision and scope

Implement four small, separately verifiable changes: peer-aware admission for the console; bounded blocking I/O and queue submission for local OTLP; all-component symlink rejection for selected log files; and a diagnosed repair of the extended-verification toolchain setup. Preserve the existing security and durability semantics while doing so.

This is not a full application security audit. The earlier review identified source-level concerns, not four reproduced production exploits. The current base was rechecked and remains the same commit. Do not convert preliminary severity labels into CVSS scores or claims of exploitation.

| Item | What the reviewed source establishes | What this specification will establish if its gates pass |
|---|---|---|
| SEC-ADM | Public authentication has a shared fixed-second budget of 8, private verification a shared budget of 32; same-source concurrency is not separately bounded. | One normalized transport peer cannot independently consume the entire public/private request allowance under the registered single-source threat model. |
| SEC-OTLP | The local receiver caps connections at 32, uses a 30-second socket read timeout, has a blocking `SyncSender::send`, and has no explicit response-write deadline. | Finite request/connection occupancy, nonblocking overload rejection, and correct permit ownership. This does **not** establish isolation from a continuously reconnecting hostile local process. |
| SEC-FS | The selected-log reader follows symlinks in both the data-read and backlog paths. The companion CLI checks only the final component. | Selected-log opens do not traverse symlinks or magic links in any pathname component; data and identity checks use the returned descriptor. |
| SEC-CI | The recorded extended job failed in Kani setup and skipped the extended registry. Tool storage is relocated, while the containment launcher forwards a narrow environment. | Pinned tool setup, positive/negative proof canaries, and the actual required extended registry execute on the intended revision with attributable receipts. |

**Out of scope:** Changing authorization grants, passkey verification, session lifetimes, CSRF semantics, token formats, batch formats, journal ordering, commit acknowledgments, core dependencies, production deployments, tags/releases, or workspace permissions. Do not raise memory ceilings, disable tests, add a permissive transport fallback, or rewrite the HTTP stack as incidental cleanup.

The generated source map and JSON manifest are part of this specification. `evidence-receipt.schema.json` and its truthful not-run template define the final evidence envelope; schema validation checks coverage and shape, not whether claims are true. They identify original source spans and blob identities. New module names are proposed paths: check for collisions before creating them. Documentation and registry changes are separate policy commits, as required by the repository's agent contract.

## 2. What was checked during specification development

Three isolated checks were run outside the application. Their source and results are included in `probes/` and `design-probe-results.json`. They are assistant-written demonstrations, not independent verification and not FabricO11y qualification.

### 2.1 Admission counterexample

A deterministic 60-second trace issues an attacking burst of 100 requests at each integer second. A different peer offers two requests 1 and 2 milliseconds later every five seconds, for 24 honest offers. The results are:

| Candidate model | Honest admitted | Attacker admitted | Disposition for this trace |
|---|---:|---:|---|
| Existing-style global fixed window | 0/24 | 480 | Reject as single-source isolation. |
| Global token bucket only | 0/24 | 480 | Reject; changing the clock model does not add isolation. |
| Global debit followed by peer eligibility | 0/24 | 122 | Reject; rejected source traffic still drains shared credits. |
| Atomic eligibility check followed by both debits | 24/24 | 122 | Retain for implementation and adversarial testing. |

This is one scheduled counterexample, not a general availability theorem. The production policy must also constrain concurrent slow requests, peer-table growth, and cancellation. An initial probe incorrectly assumed its 100/200-millisecond arrivals would see an empty bucket despite refill. That assertion failed. The original probe and failure record are retained under `probes/rejected/`; the final trace explicitly models the intended adversarial scheduling. No application conclusion is drawn from that probe defect.

### 2.2 Deadline counterexample

A model receiving one byte every four seconds never trips a 30-second inactivity timeout over a 120-second observation. A five-second absolute header deadline still expires at five seconds. The implementation must reapply the remaining absolute time to each low-level read, not merely set a timeout once before `read_line` or `read_exact`.

### 2.3 Filesystem counterexample

An isolated Linux syscall probe created synthetic regular files, a leaf symlink, and a parent-directory symlink. A leaf-only `O_NOFOLLOW` open rejected the leaf symlink but successfully opened through the parent symlink. An `openat2` call with `RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS` rejected both with `ELOOP` and accepted the regular path. This observation supports rejecting the leaf-only fix. It does not establish race safety for a future Rust wrapper or security against hardlinks, mount manipulation, or a compromised OS owner.

## 3. Evidence and line-number discipline

All line numbers mean **1-based lines of the reviewed base**, not line numbers after edits. The authoritative machine-readable record is `change-manifest.json`; `original-source-edit-map.md` is its readable rendering. Each proposed edit has an ID, source span, symbol/text anchor, original Git blob SHA, intended operation, and acceptance-test IDs.

An implementing agent must first run the provided read-only `verify_base.py` against its checkout. It checks the base's blobs and anchors, then reports working-tree drift. A changed current file is not permission to overwrite it. Inspect the relevant diff, re-anchor the affected operation, preserve existing work, and record the newly reviewed identity before proceeding. Do not mechanically replace current lines using these historical numbers.

Read `AGENTS.md`, `docs/CURRENT.md`, the relevant product contract/ADRs, and the current verification matrix before implementation. Source inspection for this specification does not replace that checkout-local preflight.

## 4. Iterative generate, challenge, verify, select procedure

### 4.1 Candidate states

Use a persistent local work queue, with each candidate in exactly one of:

`proposed -> counterexample_ready -> implementing -> verifying -> accepted`

Alternative terminal states are `rejected`, `blocked`, or `superseded`. A failed command is not a completed gate. An unavailable dependency is not a passing test. `accepted` requires the observations below, not an agent's review opinion.

For each issue, initially compare at most three meaningfully different mechanisms: the smallest local repair, a stronger bounded variant, and an architectural alternative only where a counterexample motivates it. Do not generate variants that merely rename the same algorithm. Additional iterations require a concrete surviving counterexample or a measured trade-off; do not loop for aesthetic perfection.

### 4.2 Hard acceptance rules

Freeze the threat model, invariants, fixtures, metrics, limits, expected negative-control failures, and source identities before evaluating a candidate. The independent checker must evaluate externally visible observations or a separately specified state model. It must not import the candidate's policy function and call the same logic an oracle.

A candidate is rejected immediately if it permits unauthorized access; produces a successful acknowledgment without the required durable commit; grows resources without the specified bound; follows a prohibited path; treats unknown completion as known failure or success; weakens the oracle; fails cleanup; or changes a protected contract without a separate explicit policy decision.

Only compare performance after correctness gates pass. Among candidates satisfying the same required behavior, prefer the smaller implementation unless another has a repeatable, material benefit. “Suboptimal” means dominated on the frozen criteria or rejected by a specific counterexample, not disliked by a reviewer.

### 4.3 Rejection and cancellation

Retain the candidate patch, minimized input, source/checker hashes, raw observations, command, exit status, and rejection reason. Stop only processes/cgroups and worktrees owned by that candidate. Verify that its descendants have stopped before deleting owned scratch. Never use broad process-name killing, `git reset --hard` on a user's working tree, force pushes, or deletion of published history. Continue with the surviving candidate without modifying the test's expected outcome to rescue the loser.

If a test itself is invalid, retain its failure, correct it in a separate verifier/policy change, and rerun both the baseline and every surviving candidate. The failed initial design probe in this bundle is an example of an invalid test assumption, not a security finding.

### 4.4 Implementation sequence

| Iteration | Deliverable | Reject/stop condition |
|---|---|---|
| R0: Freeze | Verify identities; record actual host/resource limits; register protocol and exact counterexamples. | Stale anchors, unowned work, missing containment, or an unregistered changed contract. |
| R1: Restore verification | Reproduce the Kani setup problem and admit the smallest validated toolchain repair. | A green setup step without a real proof canary and actual extended execution. |
| R2: Console admission | Implement trusted peer identification and atomic bounded admission; preserve authorization. | One peer consumes all request slots/credits, header spoofing changes identity, or authority checks regress. |
| R3: OTLP transport | Bound reads, writes, queue submission and connection ownership. | A blocking stage escapes its declared deadline; false ACK; leaked slot; or a claim of local identity isolation. |
| R4: Selected-log opens | Adopt one all-component policy across data, backlog, and companion preflight. | Leaf-only protection, pathname reopen, insecure fallback, or cursor advancement after refusal. |
| R5: Integration | Fast/extended/browser/negative controls, repeated performance comparisons, complete evidence. | A required result is missing, stale, incomparable, or a safety/resource invariant fails. |

R2 and R4 can be developed in separate owned worktrees after the relevant R0 protocol is frozen. Final integration is serial and reruns shared checks. Do not start production load tests as part of this procedure.

## 5. SEC-ADM: console admission

### 5.1 Selected design

Add a private server-adapter module `crates/fabric-server/src/console/admission.rs`. Keep this policy out of the dependency-free semantic core; no new crate or generic middleware framework is necessary. It owns only in-memory admission state and receives monotonic elapsed time as an explicit input. It never reads access state, performs filesystem I/O, verifies a passkey, or awaits while holding its state lock.

Use the real transport peer supplied by the native Axum serving root. Change `crates/fabric-server/src/lib.rs:122` from `.serve(app.into_make_service())` to the `into_make_service_with_connect_info::<std::net::SocketAddr>()` variant. Confirm compatibility against the pinned Axum/axum-server versions with compilation and an actual TLS request, not just a mocked request extension.

Normalize IPv4-mapped IPv6 into IPv4. For this candidate, group native IPv6 addresses by /64 and exclude source ports from the key. The /64 choice is an explicit operational policy, not a claim that a /64 always equals one person. Never derive the key from `Origin`, `Host`, `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, a claimed account ID, or attacker-supplied JSON.

If trustworthy peer metadata is absent, fail closed with a configuration-class response before body consumption or access-state mutation. Tests must attach explicit mock connection metadata; production must not substitute a synthetic localhost/default key. Deployments behind a proxy are one peer under this minimal policy. Do not enable forwarded-address trust implicitly; proxy-aware identity requires an independently scoped and tested trust configuration.

### 5.2 Initial policy to freeze in R0

These are proposed starting parameters, not measured optimal values. Freeze the selected values before comparing implementations.

| Lane | Global refill | Global burst | Global concurrent requests | Peer refill | Peer burst | Peer concurrent requests |
|---|---:|---:|---:|---:|---:|---:|
| Public passkey endpoints | 8/s | 8 | 8 | 2/s | 4 | 2 |
| Private console endpoints | 32/s | 32 | 16 | 8/s | 8 | 4 |

Keep the existing separate two-query permit pool and its blocking-worker ownership. Do not confuse a request-admission guard with a query execution permit. Public/private rate state is separate so one lane does not spend the other's credits. Peer-state cardinality is bounded at 1,024 total normalized peer entries; each entry may hold both lane states. Target limiter-owned memory at or below 512 KiB at that cap, measured including actual container overhead. Do not claim that this caps HTTP/TLS connection memory or the whole server.

### 5.3 Required types and operations

The following are design signatures, not a compiled patch:

```rust
enum Lane { PublicAuth, PrivateConsole }
enum PeerKey { V4([u8; 4]), V6Prefix64([u8; 8]) }
struct AdmissionPolicy { /* explicit finite limits */ }
struct Admission { /* shared bounded state */ }
struct AdmissionGuard { /* owns one peer and one global live slot */ }
enum Reject { RateLimited, ConcurrentLimit, PeerTableFull, InvalidClock, Unavailable }

// Time comes from a process-relative monotonic clock at the adapter boundary.
fn try_acquire(
    &self, lane: Lane, peer: PeerKey, elapsed: std::time::Duration,
) -> Result<AdmissionGuard, Reject>;
```

Keep constructors and test policy injection private. Test-only small limits must not become an environment variable that can disable production protection.

Represent token credit using checked/saturating integer fixed-point arithmetic, not floating point or Unix wall-clock seconds. One token can be `1_000_000_000` credit units. Refill is:

`credit' = min(burst * scale, credit + elapsed_ns * rate)`

Clamp elapsed contribution before multiplication or use checked wider arithmetic. A backward injected time must not produce credit. Test long process lifetimes, arithmetic limits, repeated timestamps, and exact boundaries.

### 5.4 Atomic reservation rule

Under one short lock: locate/create the bounded peer entry; refill relevant peer/global state; check **all** credit and live-slot conditions; then debit both budgets and acquire both live slots together. Rejections must not consume another source's global credit. No rate-token refunds after admission: the request has already consumed work. Live-slot releases occur exactly once through guard ownership on completion, error, cancellation, or unwinding.

Do not acquire a global token, discover the peer is ineligible, and leave the global debit in place. Do not acquire a global semaphore and wait for peer allowance. This is immediate admission or immediate rejection, never an unbounded waiting queue.

The admission lock must never panic on a poisoned state. Fail new admission closed as unavailable; guard destruction must not panic or silently create fresh capacity. Keep any recovery of bookkeeping separate from restoring service authority.

A peer entry may be reclaimed only when both lanes have no live guards, rate credit can be reconstructed as fully replenished, and its idle-retention interval has elapsed. Start with 120 seconds of idle retention. A denied request must not create unbounded new entries. At table capacity, decline unrecognized peers rather than evict live/indebted entries and grant fresh bursts. Preserve service for already tracked peers. Table saturation by many distinct source identities is a remaining availability limit, not solved by this policy.

### 5.5 Exact integration operations

Apply ADM-01 through ADM-06 in the source map. In `Console` replace `requests`, `ceremonies`, `auth_budget`, and `private_auth_budget` with the shared controller. Retain `queries`, cursors, access, and application state. Capture the validated configured origin once in immutable adapter state so initial origin comparison does not take the persistent access-state lock.

In `auth_admission`, establish peer identity and acquire the public guard before body extraction or stateful authentication. Preserve exact origin checking, client-version checking, and the existing request timeout. Limiter refusal should return a bounded generic 429 response with `Retry-After: 1`; table exhaustion can use the same public response while retaining a distinct low-cardinality internal reason.

In the private path, acquire the guard in the **outer `authorize` wrapper**, not solely inside `authorize_request`. Hold it until denial auditing has completed. Otherwise denied traffic can escape the request-accounting bound during synchronous audit work. Remove only the old request semaphore/fixed-second budget in `authorize_request`; preserve credential disambiguation, action selection, body-dependent scope checks, CSRF, freshness requirements, and final authority rechecks.

Limiter refusals do not generate a durable audit entry each time. Use bounded counters by lane and reason and an aggregate operational observation. Do not put raw IPs, arbitrary paths, tokens, cookies, principal guesses, or headers into metric labels. Existing authorization-denial auditing remains required for admitted requests.

### 5.6 Important timing limitation

`tokio::time::timeout` does not preempt synchronous code that does not yield. The existing access adapter uses synchronous locking and persistence. This change must not claim a hard wall-clock limit for `fsync` or WebAuthn work merely because middleware has a timeout.

Measure contention and executor responsiveness. If the retained design fails the frozen latency/resource gates because synchronous work blocks the runtime, reject that candidate and isolate the relevant operation behind a bounded blocking executor with permit ownership extending through actual completion. Do not immediately rewrite the whole access subsystem or release permits when an async waiter alone stops.

### 5.7 Closure criteria

A01-A14 must pass, including real native HTTPS/browser operation and preserved query-permit ownership. The single-source protection claim applies only when the honest client has a different trusted peer key and load remains within the registered envelope. NAT peers, all clients behind one proxy, distributed attacks, pre-routing TLS overload, and sustained compromised authorized workloads are not declared solved.

## 6. SEC-OTLP: bounded local transport

### 6.1 Selected design and threat-model correction

Retain the existing blocking-thread transport and valid HTTP/protobuf shape. Add one small deadline-I/O helper and RAII connection ownership. A wholesale switch to Tokio/Hyper is not justified unless the bounded local repair fails a registered correctness or compatibility gate.

A loopback TCP address is not a local process identity. Every local process able to connect can compete for capacity. Deadlines cap how long each accepted connection holds resources; a continuously reconnecting attacker can still take newly available slots. Therefore this iteration closes the unbounded-stage/ownership defects and **mitigates**, rather than fully resolves, the hostile-local availability concern.

For a future requirement that untrusted local users must be isolated, the admitting identity must change. A credible separate design is an owner-controlled Unix-domain socket, explicit allowed UIDs, peer-credential verification, per-UID quotas, and the plain loopback listener disabled in that profile. SDK/transport compatibility must be demonstrated first. Neither a shared loopback-IP bucket nor a bearer header checked after connection admission proves that stronger isolation. Do not claim that stronger finding closed by this specification's minimal TCP changes.

### 6.2 Required policy

Retain `MAX_CONNECTIONS = 32`, `QUEUE = 64`, `MAX_HEAD = 16 KiB`, and `MAX_EXPORT = MAX_BATCH - 96 KiB`. Proposed phase limits to freeze are:

| Operation | Limit |
|---|---:|
| Header read, including an idle keep-alive connection waiting for its next request | 5 seconds absolute |
| Body read after a complete valid header | 10 seconds absolute |
| Wait for commit result after successful queue admission | At most the existing 30 seconds |
| Response write | 5 seconds absolute |
| Total request residence | 50 seconds, additionally bounded by remaining connection lifetime |
| Connection lifetime | 120 seconds from acceptance, never reset by request activity |
| Requests per connection | 128, including rejected parsed requests |

All phase deadlines are clipped to the remaining total-request and connection lifetimes. These are I/O/wait bounds plus bounded parsing work, not a guarantee against an arbitrarily stalled OS scheduler or an unpreemptible syscall. The tests must distinguish semantic deadline errors from documented scheduling tolerance.

### 6.3 Connection ownership

Apply OTLP-01 and OTLP-02. Add a private `ConnectionPermit` acquired before spawning the worker and moved into its closure. Drop releases the count whether serving returns, I/O fails, a test worker panics, or the spawn closure is destroyed on spawn failure. Remove the manual success-path and error-path decrements once RAII owns the release.

The current listener is single-threaded; do not invent an observed concurrent-increment bug. RAII is selected to make ownership correct on every exit and robust to future refactoring. Maintain an actual cap check at the acquisition operation. Validate loopback in `start` itself as well as in configuration parsing.

Tests must own listener/worker lifetimes and join or otherwise verify their termination. Do not accumulate detached fixture threads. Use a small test-only serve/accept helper where needed, without adding a production environment override or an unbounded generic server framework.

### 6.4 Low-level deadline mechanism

Add `src/spindle/otlp/deadline_io.rs` with a private socket reader/writer adapter. Before **each underlying socket read or write**, compute positive remaining time from the fixed monotonic deadline, set that timeout, perform the operation, and recheck before retrying interrupted/partial progress. If no time remains, return a timeout without making another blocking call. Never pass a zero `Duration` to the socket timeout API.

Wrap the actual socket beneath `BufReader`, so `read_line` and `read_exact` cannot repeatedly obtain a fresh full timeout. Check deadlines at phase transitions and while handling already-buffered next requests. A byte arriving before the deadline does not extend the deadline. Handle platform timeout errors including `WouldBlock`/`TimedOut`; the target is Linux, but tests should not assume one exact message string.

Use a bounded response-write helper, not unbounded `write_all`. A partial write uses the same absolute deadline. Once writing fails or time expires, close; do not attempt another error response through an already blocked writer.

Keep the existing header/body size checks and protobuf validation. Do not broaden accepted formats to make fixtures pass. Parser-tightening unrelated to these bounds is a separate documented change.

### 6.5 Queue and acknowledgment state machine

Replace the blocking call at `otlp.rs:242` with `try_send`:

`validated -> not_enqueued | queued -> confirmed_committed | confirmed_unavailable | completion_unknown`

A full/disconnected queue means the export was not admitted by this attempt. Return a bounded 503/close without waiting for queue capacity. Once an export has been enqueued, a timeout or disconnected reply channel can leave its commit outcome unknown. Do not label that outcome “definitely not committed.” Do not delete accepted work simply because the HTTP waiter expired.

Only `Commit::Committed(_)` observed from the consumer may produce HTTP 200. Queue acceptance, socket readability, an empty channel, or a timer expiration is never a successful commit. A late commit may be followed by an exporter retry; retain the existing at-least-once possibilities rather than inventing exactly-once OTLP semantics.

Prefer a separate local wait-outcome type over manufacturing a `Commit::Unavailable` value to represent timeout. If an enum comment says “not committed,” it must not describe an unknown outcome. Keep `drain`, Spool ownership, and reply-after-commit ordering unchanged except for mechanically necessary type handling. Malformed/overloaded/deadline-expired requests should close their connection after the bounded response.

### 6.6 Bounds and acceptance

At most `(64 queued + 32 serving) * MAX_EXPORT` raw export bytes are attributable to those two ownership locations, excluding consumer batches, parsing expansion, buffers, and thread stacks. Measure those excluded components; the expression is not a process-memory guarantee. Keep the node's existing package resource ceiling. Do not double-copy payloads to add the timeout mechanism.

O01-O14 must pass. Explicitly test queue-full before the wait timer, slow headers, slow bodies, blocked writers, buffered pipelining, spawn failure, dropped clients, late commit, pause/full Spool, and cleanup. Record hostile reconnect behavior honestly as residual risk. Test the retained SDK request shape and normal keep-alive behavior before treating deadline settings as compatible.

## 7. SEC-FS: selected log path resolution

### 7.1 Selected design

Use one Linux adapter helper, proposed as `log_source::open_regular_log(&Path) -> io::Result<std::fs::File>`, backed by a small private module `log_source/secure_open.rs`. It returns the actual checked descriptor, not a supposedly safe pathname.

Use `openat2` with `O_RDONLY | O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW`, zero mode, and `RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS`. The selected API accepts the existing absolute log paths; it is not a new directory sandbox. Do not add `RESOLVE_BENEATH` with an absolute path and assume the kernel will accept it. If a later design anchors relative paths beneath a directory FD, specify that different contract explicitly.

Validate absolute, nonempty, NUL-free input and reject parent-directory components. Do not silently canonicalize configured paths or alter scope matching. Use existing libc facilities and platform syscall constants rather than architecture-specific numeric syscall IDs. Prefer an available safe wrapper only if already present and suitable; do not add a new core dependency. Any unsafe adapter shim must document ABI layout, zero initialization, pointer lifetime, returned-FD ownership, and the error path that never constructs a `File` from a negative FD.

On the returned descriptor, inspect metadata and reject nonregular files before reading. Keep `O_NONBLOCK` to avoid FIFO-open hangs. This does not promise that opening an arbitrary device has no side effects; Linux permissions and trusted source configuration remain important. Do not use a newly introduced kernel-only flag that older supported deployment kernels may lack.

### 7.2 Integration points

FS-01 replaces the open/regular-file block in `unread_bytes` at lines 92-105. FS-02 replaces the equivalent block in `read_lines_with_identity` at lines 139-162. Both call the same helper. `fstat`, Btrfs identity inspection, consumed-prefix verification, seeking, and reading must remain tied to that returned descriptor.

FS-03 changes the companion `--server-log` preflight in `src/bin/fabric-node.rs:167-177` to use the same helper rather than its current leaf-only `O_NOFOLLOW` check. Remove an import only if it becomes unused; do not refactor unrelated startup behavior.

Public `read_lines`/`read_lines_costed` signatures and cursor persistence stay compatible. A rejected source must not advance an offset, fabricate consumption, or prevent unrelated metrics/log sources from running. Preserve the existing bounded gap/error representation. New reason codes must be low-cardinality and must not leak the contents of a refused target into operational logs.

### 7.3 Error and compatibility policy

`ELOOP`, missing files, and permissions failures become visible per-source failures. An unavailable/blocked `openat2` mechanism, such as `ENOSYS` or security-policy `EPERM`, must **not** trigger `File::open`, `canonicalize + open`, or leaf-only fallback. Report secure opening unavailable and leave that source uncollected. Prove compatibility on the supported kernel/package targets before release.

This is an explicit tightening: existing symlinked logs, including paths traversing distribution symlinks, will no longer collect through those aliases. Document how an operator selects and grants the intended physical path. Do not silently rewrite grants or add a remote-client-controlled `follow_symlinks=true` escape hatch. If trusted symlink support is later required, use a separate reviewed policy with an explicit trusted directory boundary and race-safe resolution.

No-symlink resolution does not authenticate file content or stop ordinary file replacement, hardlinks, bind mounts, or a compromised service/OS owner. Keep the existing least-privilege ACL guidance and state those remaining assumptions. A changing regular log file is legitimate behavior; do not pin its inode forever and break rotation to claim stronger isolation.

### 7.4 Verification

F01-F10 must pass for **all three** entrypoints. Keep deterministic tests for leaf and parent links, magic links, FIFO/nonregular refusal, unsupported secure-open operation, failure-to-gap handling, and normal cursor behavior. Add controlled rename tests that distinguish replacement before open from replacement after the FD is returned. Random race stress supplements these tests; a race not reproduced is not a proof of safety.

Reject the following candidates outright: `canonicalize` then open; `symlink_metadata` then open; `O_NOFOLLOW` only; securing the data reader but not the backlog reader; checking a safe FD then reopening its path; or treating a denied read as consumed input.

## 8. SEC-CI: restore actual verification

### 8.1 Facts versus diagnosis

The recorded job `114280952330` in run `38075314261` failed during Install Kani; the extended registry step was skipped. Its retained log reported rustup not installed at the relocated Cargo home. The setup script directs Cargo and rustup homes to mounted storage. The containment launcher explicitly forwards PATH and tool homes but not CI/GITHUB_ACTIONS markers.

A plausible causal chain is: the contained rustup invocation does not detect CI, attempts self-update or installation-location validation, and fails because the manager launcher belongs to a different installation location. This is a **diagnosis to reproduce**, not an established fact merely because it explains the error. A repair is accepted only after the exact contained setup path succeeds from a clean runner and the proof checks execute.

### 8.2 Minimal candidate

Add `tools/ci/security_preflight.py` and its tests. It runs through the existing resource launcher and calls `require_limits()`. Record only allowlisted diagnostics: command paths, resolved tool paths, versions, tool-home locations, relevant storage/cgroup evidence, and exit status. Do not dump the whole environment or credentials.

Verify that Cargo/rustup toolchain storage is on the intended mounted disk and `rustup which cargo`/`rustup which rustc` resolve the selected pinned toolchain. A trusted runner-provided rustup launcher outside that disk may be legitimate; distinguish a launcher from the toolchain/data it selects. PATH membership alone is insufficient.

Set `rustup set auto-self-update disable` under the **same `RUSTUP_HOME`** used by subsequent contained commands. This is an explicit CI policy, not reliance on an inherited environment marker. Run the preflight after the toolchain action in both workflows, before tool installation. Keep the native/WASM toolchain and Kani package pinned as in the base.

Do not modify the general environment-forwarding allowlist just to make CI look normal. In particular, do not use wholesale user-service environment import. Do not bypass containment or move caches back to the system disk. Leave `prepare-resource-host.sh` and `resource_group.py` unchanged under this minimal candidate unless reproduction proves an additional repair necessary.

### 8.3 When to reject the minimal candidate

If the same installation-location failure persists with explicit self-update disabled, stop extending speculative PATH workarounds. Retain the actual trace and investigate manager/proxy installation consistency. A separate candidate may install a complete, pinned and integrity-verified rustup manager/proxy layout in the relocated home. It must demonstrate the binary/proxy/toolchain relationship and stay within the same storage/security boundary. Do not manufacture an unverified installer version or checksum, copy arbitrary caches, or add symlinks solely until the job turns green.

Restoring the original system-disk cache location is rejected because it violates the repository's resource contract. Ignoring setup errors, disabling Kani, deleting proof checks, or adding `continue-on-error` is rejected because it removes the assurance rather than repairing it.

### 8.4 Exact workflow changes

Apply CI-01 through CI-06. Add `tools/ci/**` and `tools/resource_group.py` to the extended workflow's applicable pull-request paths; otherwise a repair to the tooling can evade its own workflow. Inspect actual required-check/ruleset configuration before prescribing branch-protection changes. A repository reported as protected is not evidence that this particular check is required.

After Kani setup, run a small positive canary and a deliberately failing reachable-assertion canary using the real verifier. Preserve the canary's known assertion failure separately from compile/environment errors. The latter are not successful negative controls. Then run the unmodified required extended registry and retain its receipts. A setup failure must produce a failed workflow and a truthful `not_run` registry result, never a successful receipt copied from another revision.

Verify both clean and warm-cache paths. Record the actual workflow run, commit, tool version, command exits, check IDs, receipts, and cleanup. Do not relabel the historical failed run as repaired; only a new attributable run demonstrates the fix.

## 9. Verification, performance, and documentation contract

### 9.1 Required test artefacts

`acceptance-tests.json` and `acceptance-tests.md` specify 53 test cases, each with a stimulus, required observation, negative control, test layer, and initial `not_run_on_application` status. Implement the tests before treating the corresponding code change as accepted.

New independent checkers belong under `tools/security/` and consume recorded decisions, requests, replies, times, and resource observations. They must validate input/receipt identity, reject missing observations, and distinguish assertion failures from harness errors. Register checker self-tests and semantic negative controls in `xtask/checks.json` in a separate policy commit. Native unit/integration tests continue to run through the existing workspace test gate.

Use the existing counterexample registry's schema after reading it. Record the original review or minimized probe as origin, the violated invariant, the source/checker revision, and the fixing commit after one actually exists. Do not invent fix SHAs, PASS receipts, or benchmark measurements.

### 9.2 Commands and containment

The following are existing commands to run on the implementing environment, not commands run for this specification:

```sh
python3 -B tools/resource_group.py -- cargo xtask checks --profile fast
python3 -B tools/resource_group.py -- cargo xtask checks --profile extended
python3 -B tools/resource_group.py -- cargo test --locked -p fabric-server
python3 -B tools/resource_group.py -- cargo test --locked -p fabric-adapter-linux
python3 -B tools/resource_group.py -- cargo test --locked -p fabric_o11y
python3 -B tools/resource_group.py -- cargo deny --locked --workspace check
```

Use the registered packaged-browser fixture as well as direct Router tests. Do not guess that a newly named standalone test command exists before implementing/registering it. Run actual project experiments and their validators through the same launcher. Preserve the designated mounted scratch and the actual host-appropriate ancestor cgroup limits. No uncapped fallback, swap re-enablement, global cleanup, or destructive production fault injection is authorized by this specification.

### 9.3 Proposed comparison protocol

Freeze actual limits and workloads in R0. Use the same commit baseline, machine, kernel, toolchain, TLS/browser configuration, input bytes, seeds, and cgroup. Alternate baseline/candidate order and collect at least seven paired measurement runs after warmup for performance-sensitive slices. Evaluate correctness on every run, not only a selected median.

Record p50/p95/p99 latency, admitted/refused counts by defined reason, completed operation throughput, CPU time, peak memory, file descriptors, thread counts, queue occupancy, durable-write counts where material, and cleanup. Unavailable metrics remain unavailable.

Proposed initial performance gates are: no more than 10% throughput regression on the registered below-limit benign workload; p99 latency no greater than `max(1.10 * baseline_p99, baseline_p99 + 5ms)`; no resource-cap increase; and no increasing post-drain FD/thread/memory trend. These are **proposed acceptance tolerances**, not measurements. The owner/protocol can set justified tolerances in R0 before candidate results are known. If the result is too noisy to distinguish from a gate, report inconclusive and repeat within the approved run budget; do not widen a threshold after seeing a miss.

Under the registered single-source admission attack, all low-rate honest requests from the distinct peer must be admitted. For OTLP, require finite phase occupancy and capacity recovery when stalled clients cease; do not demand an impossible per-user fairness result from an identity-free TCP interface. A stronger local-isolation requirement keeps the corresponding finding open until a different authenticated/OS-enforced admission boundary is implemented.

### 9.4 Documentation edits

Update the relevant sections of `docs/access-operations.md` and `docs/architecture/identity-access.md` with peer identity, proxy/NAT behavior, bounded admission, and residual limits. Update the Spindle architecture and trace ADR with finite I/O/queue/ACK behavior without weakening durable acknowledgment. Update selected-log deployment/operations guidance and `packaging/etc/node.conf.example` with explicit no-symlink policy, physical-path migration, and secure-open-unavailable behavior.

Update `docs/formal/console-security-map.md`, the verification matrix, and `CURRENT.md` with actual check identities and statuses. Add a dedicated registered security-hardening protocol. Keep architecture diagrams and the learning path aligned where behavior changed. Read each document before editing and identify its exact current section in the implementation change record; this bundle does not invent uninspected document line numbers. Product-contract and verifier-policy changes must be distinct from implementation commits.

## 10. Completion and honest residual status

An issue may be marked implemented only after its code exists. It may be marked verified only after the required matching checks ran. A release may not cite this specification as a successful security scan or qualification result.

The final implementation receipt must include the implementing commit, base/diff identity, frozen protocol/checker identities, per-test commands and exits, expected negative-control observations, measured resources, preserved counterexamples, cleanup confirmation, and unresolved assumptions. It must link every edit ID to its tests and every claim to actual evidence.

At the end of the selected work:

* SEC-ADM can close only for the stated distinct-peer, bounded single-source threat model.
* SEC-OTLP can close for bounded stage/permit handling; hostile-local liveness remains explicitly mitigated/open unless a new identity-enforcing profile is accepted and verified.
* SEC-FS can close for symlink/magic-link path traversal, not arbitrary file provenance or OS-owner compromise.
* SEC-CI can close only with actual current-revision setup/canary/extended receipts, not a proposed shell change.

Do not describe the application as production-hardened on the strength of these four changes alone. Authorization transitions, protobuf expansion, query resource exhaustion, dependency advisories, secret handling, and persistence fault behavior remain part of a broader audit.

## 11. Source references

Source links are pinned where the application is concerned. See `change-manifest.json` for all reviewed blob SHAs.

- [Repository operating contract](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/AGENTS.md).
- [Console admission and authorization wrapper](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/crates/fabric-server/src/console.rs#L320-L422).
- [Native TLS composition](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/crates/fabric-server/src/lib.rs#L120-L123).
- [Local OTLP transport](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/src/spindle/otlp.rs).
- [Log source opens](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/crates/fabric-adapter-linux/src/log_source.rs#L92-L162) and [companion preflight](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/src/bin/fabric-node.rs#L167-L177).
- [Extended workflow](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/.github/workflows/verification.yml), [recorded failed run](https://github.com/kmosoti/FabricO11y/actions/runs/38075314261), and [job](https://github.com/kmosoti/FabricO11y/actions/runs/38075314261/job/114280952330).
- [Mounted tool-home preparation](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/tools/ci/prepare-resource-host.sh#L51-L62), [contained environment](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/tools/resource_group.py#L153-L169), and [pinned tool installer](https://github.com/kmosoti/FabricO11y/blob/18f6b367d0f34886847ea25b9168e50eb0425300/tools/ci/install-cargo-tool.py).
- [Axum ConnectInfo contract](https://docs.rs/axum/latest/axum/extract/connect_info/struct.ConnectInfo.html) and [MockConnectInfo test support](https://docs.rs/axum/latest/axum/extract/connect_info/struct.MockConnectInfo.html). Use the application's locked version when implementing; these upstream pages explain the API contract, not the compiled integration.
- [Rust TcpStream timeout/clone semantics](https://doc.rust-lang.org/std/net/struct.TcpStream.html).
- [Linux openat2 resolution semantics](https://man7.org/linux/man-pages/man2/openat2.2.html). The specified behavior uses the established all-component resolution flags, not newer optional open flags.
- [Rustup team's CI self-update guidance](https://blog.rust-lang.org/2025/03/02/Rustup-1.28.0/). The historical guidance motivates the minimal candidate; the actual runner reproduction decides whether it fixes this failure.
