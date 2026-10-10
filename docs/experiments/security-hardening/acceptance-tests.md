# Acceptance test catalogue
These are requirements, not executed FabricO11y test results. A negative control must fail for the named defect, not for a compiler, fixture or environment error.

## admission

### A01
**Stimulus:** Native HTTPS request with real ConnectInfo, then same request without transport metadata.

**Required observation:** Real socket peer is recognized; missing metadata fails closed before body/access mutation.

**Reject this defective implementation:** Replace missing peer with localhost/default shared identity.

**Layer:** integration. **Execution:** not run on application.

### A02
**Stimulus:** One peer floods public and private lanes; a distinct peer performs valid requests.

**Required observation:** Source/global rate bounds and source concurrency caps hold; all low-rate honest requests in frozen trace are admitted.

**Reject this defective implementation:** Delete peer bucket; or debit global token before checking peer eligibility.

**Layer:** model+integration. **Execution:** not run on application.

### A03
**Stimulus:** Slow bodies from one source fill its own request allowance.

**Required observation:** Public source holds <=2 of 8 slots; private source <=4 of16; another peer retains admission headroom.

**Reject this defective implementation:** Apply rate limits but omit per-peer in-flight caps.

**Layer:** integration. **Execution:** not run on application.

### A04
**Stimulus:** Same TCP peer rotates X-Forwarded-For, Forwarded, X-Real-IP and source port.

**Required observation:** All requests debit the same trusted peer key.

**Reject this defective implementation:** Use header or source port in PeerKey.

**Layer:** integration. **Execution:** not run on application.

### A05
**Stimulus:** Compare IPv4, IPv4-mapped IPv6, and multiple addresses within one IPv6 /64.

**Required observation:** v4-mapped address shares IPv4 bucket; configured /64 grouping is enforced.

**Reject this defective implementation:** Key raw SocketAddr or rotate /128 within prefix.

**Layer:** unit. **Execution:** not run on application.

### A06
**Stimulus:** t=999ms/1001ms boundary, large elapsed time, repeated same time, backward injected time.

**Required observation:** No fixed-window double burst; integer credit invariant holds; backward time gives no credit.

**Reject this defective implementation:** Reset at wall-clock second or use wrapping time arithmetic.

**Layer:** property. **Execution:** not run on application.

### A07
**Stimulus:** Fill 1024 peer entries; hold leases; attempt churn/expiry/recreation.

**Required observation:** No live entry evicted; no debt reset; no more than1024 entries; existing peers remain serviceable.

**Reject this defective implementation:** Evict an indebted/live entry and recreate it with full burst.

**Layer:** property+integration. **Execution:** not run on application.

### A08
**Stimulus:** Drop waiter mid-body, handler error, task abort, panic in isolated fixture.

**Required observation:** Every owned request permit releases exactly once; no counter underflow or leaked admission.

**Reject this defective implementation:** Retain manual counter decrement after handler return.

**Layer:** integration. **Execution:** not run on application.

### A09
**Stimulus:** Unauthorized request triggers slow denial audit; policy changes during read.

**Required observation:** Outer private guard covers audit; original scope and final authority recheck still enforce denial.

**Reject this defective implementation:** Drop guard before record_denial; remove final recheck.

**Layer:** integration. **Execution:** not run on application.

### A10
**Stimulus:** Sustained limiter rejections at registered bound.

**Required observation:** Only bounded low-cardinality counters; no token/IP/path payloads or durable write per rejection.

**Reject this defective implementation:** Persist every429 with supplied path/token.

**Layer:** integration. **Execution:** not run on application.

### A11
**Stimulus:** Production defaults and reduced test policy under deterministic clock.

**Required observation:** Public8/s burst8 global,2/s burst4 peer; private32/s burst32 global,8/s burst8 peer; table1024.

**Reject this defective implementation:** Accidentally expose test-only override via environment.

**Layer:** unit. **Execution:** not run on application.

### A12
**Stimulus:** Invalid/duplicate cookie, invalid bearer, bad CSRF/origin, stale version, revoked/expired/delegated credentials.

**Required observation:** No authorization or session-semantic regression; body remains unread for header-stage rejection.

**Reject this defective implementation:** Treat admission as authorization or bypass fresh verification.

**Layer:** integration. **Execution:** not run on application.

### A13
**Stimulus:** Run actual native TLS fixture and packaged browser passkey login; run direct Router tests with explicit MockConnectInfo.

**Required observation:** Production receives real peer info; test mocks never enable production fallback; login completes.

**Reject this defective implementation:** Only test request extensions by hand; native TLS path remains unconfigured.

**Layer:** integration. **Execution:** not run on application.

### A14
**Stimulus:** Cancel expensive query waiter while blocking query continues.

**Required observation:** Existing two query permits remain owned until actual query worker ends.

**Reject this defective implementation:** Release query permit when HTTP future times out.

**Layer:** integration. **Execution:** not run on application.


## OTLP

### O01
**Stimulus:** Invoke check_listen and start with loopback/non-loopback addresses.

**Required observation:** Both reject non-loopback; existing loopback config remains compatible.

**Reject this defective implementation:** Validate only config string, not start API.

**Layer:** unit+integration. **Execution:** not run on application.

### O02
**Stimulus:** Open 32 simultaneous connections and a33rd in synthetic loopback fixture.

**Required observation:** At most32 connection workers; extra connection promptly closed; permit count returns to zero.

**Reject this defective implementation:** Reserve slot only after thread spawn.

**Layer:** integration. **Execution:** not run on application.

### O03
**Stimulus:** Inject thread-spawn failure immediately after permit ownership transfer.

**Required observation:** Permit is released once; listener still accepts a later valid connection.

**Reject this defective implementation:** Manual failure decrement plus RAII causes double release.

**Layer:** unit. **Execution:** not run on application.

### O04
**Stimulus:** EOF, parse failure, I/O error and test panic on worker.

**Required observation:** RAII releases slot on each exit; no manual normal-return-only accounting.

**Reject this defective implementation:** Move release back to statement following serve.

**Layer:** unit+integration. **Execution:** not run on application.

### O05
**Stimulus:** Send incomplete header bytes more frequently than inactivity timeout.

**Required observation:** Connection closes at fixed header deadline within frozen scheduler tolerance.

**Reject this defective implementation:** Set socket timeout once; extend deadline whenever byte arrives.

**Layer:** integration. **Execution:** not run on application.

### O06
**Stimulus:** Advertise valid bounded Content-Length and dribble body forever.

**Required observation:** Absolute body deadline closes connection; no Export queued, no200.

**Reject this defective implementation:** Use read_exact with only a fixed per-read timeout.

**Layer:** integration. **Execution:** not run on application.

### O07
**Stimulus:** Persistent keep-alive/pipelined requests, including already-buffered next header.

**Required observation:** Idle/total-age/request-count limits apply even to buffered bytes; final response advertises close.

**Reject this defective implementation:** Reset connection age on each request or forget buffered path.

**Layer:** integration. **Execution:** not run on application.

### O08
**Stimulus:** Peer does not read replies; induce partial writes and EAGAIN/EINTR.

**Required observation:** Write deadline reapplied to every low-level write; closes without retrying a second response.

**Reject this defective implementation:** Unbounded write_all or timeout reset on progress.

**Layer:** integration. **Execution:** not run on application.

### O09
**Stimulus:** Fill queue64 with consumer blocked, submit another complete bounded export.

**Required observation:** Immediate overload path from try_send; never blocks before commit timer.

**Reject this defective implementation:** Restore SyncSender::send.

**Layer:** integration. **Execution:** not run on application.

### O10
**Stimulus:** Enqueue succeeds; waiter expires; consumer later durably commits.

**Required observation:** No false200 and no false assertion that data never committed; retained work follows existing custody semantics.

**Reject this defective implementation:** Delete accepted queue item on timeout or label timeout as definitely uncommitted.

**Layer:** integration. **Execution:** not run on application.

### O11
**Stimulus:** Full Spool, pause, failed commit, dropped reply channel, confirmed successful commit.

**Required observation:** Only confirmed durable successful commit yields200; error/unknown returns503 or closes.

**Reject this defective implementation:** Send200 when merely queued; spoof success after timeout.

**Layer:** integration. **Execution:** not run on application.

### O12
**Stimulus:** Maximum headers/bodies/queue/32workers plus repeated reconnects under node memory cap.

**Required observation:** Memory/tasks bounded, no growth trend after drain; report that reconnecting hostile locals can still cause denial.

**Reject this defective implementation:** Call finite connection lifetime an isolation guarantee.

**Layer:** integration. **Execution:** not run on application.

### O13
**Stimulus:** Close/drop fixture listener and await all fixture workers.

**Required observation:** Tests leave no listener/thread/scratch ownership leaks; cleanup timeout is failure.

**Reject this defective implementation:** Detach fixture threads and infer cleanup from client close.

**Layer:** integration. **Execution:** not run on application.

### O14
**Stimulus:** Existing SDK-shaped protobuf requests, bad media type, duplicate length, transfer encoding, oversize request.

**Required observation:** Existing accepted valid format and meaningful error classes preserved; limits never broadened.

**Reject this defective implementation:** Raise body cap or accept unsupported transfer encoding to pass test.

**Layer:** integration. **Execution:** not run on application.


## filesystem

### F01
**Stimulus:** Open regular approved log through every production read/backlog entrypoint.

**Required observation:** Same descriptor supplies fstat/identity/prefix/read; normal collection remains exact.

**Reject this defective implementation:** Securely check path, then reopen it by name.

**Layer:** integration. **Execution:** not run on application.

### F02
**Stimulus:** Approved pathname is a leaf symlink to synthetic sentinel.

**Required observation:** Read and backlog functions reject; no sentinel bytes collected.

**Reject this defective implementation:** Remove O_NOFOLLOW/NO_SYMLINKS.

**Layer:** integration. **Execution:** not run on application.

### F03
**Stimulus:** Parent directory component is symlink; leaf is ordinary regular file.

**Required observation:** Both entrypoints reject traversal; no sentinel bytes collected.

**Reject this defective implementation:** Use O_NOFOLLOW only on final component.

**Layer:** integration. **Execution:** not run on application.

### F04
**Stimulus:** Use procfs magic-link path to a synthetic open FD.

**Required observation:** Rejected by all-component policy without reading target.

**Reject this defective implementation:** Rely on regular-file fstat after following link.

**Layer:** integration. **Execution:** not run on application.

### F05
**Stimulus:** FIFO with no writer, Unix socket, directory, and unprivileged /dev/null case.

**Required observation:** No reads from nonregular source, no FIFO hang, descriptor closed.

**Reject this defective implementation:** Remove O_NONBLOCK or regular-file check.

**Layer:** integration. **Execution:** not run on application.

### F06
**Stimulus:** Deterministic controlled rename before secure open and after returned descriptor.

**Required observation:** Before-open link is rejected; after-open replacement cannot redirect existing FD; no filename reopen.

**Reject this defective implementation:** Canonicalize then open or metadata-check then open.

**Layer:** integration. **Execution:** not run on application.

### F07
**Stimulus:** Rotation, truncation, invalid UTF8, oversize line, Btrfs namespace change.

**Required observation:** Existing cursor/gap/oversize semantics retained; failures never advance unseen bytes.

**Reject this defective implementation:** Advance cursor on denied open.

**Layer:** integration. **Execution:** not run on application.

### F08
**Stimulus:** Inject ENOSYS, EPERM, EINVAL and ELOOP from secure opener.

**Required observation:** Visible per-source failure; no insecure fallback; other collectors remain available.

**Reject this defective implementation:** Retry via ordinary File::open after secure-open error.

**Layer:** integration. **Execution:** not run on application.

### F09
**Stimulus:** Source aliases through an intended distribution symlink; absolute .. component; canonical explicit path.

**Required observation:** Migration is explicit; alias rejected, canonical approved path works; no hidden scope normalization.

**Reject this defective implementation:** Auto-canonicalize or silently rewrite configured/granted paths.

**Layer:** integration. **Execution:** not run on application.

### F10
**Stimulus:** Synthetic hardlink under trusted UID and synthetic replaced ordinary file.

**Required observation:** Document/test that symlink policy is not hardlink/inode-origin authentication.

**Reject this defective implementation:** Claim all arbitrary-file redirection is prevented.

**Layer:** integration. **Execution:** not run on application.


## CI

### C01
**Stimulus:** Clean hosted runner with relocated CARGO_HOME/RUSTUP_HOME and shared preflight.

**Required observation:** Record actual launcher/toolchain paths; native/WASM pinned compiler resolves to intended storage.

**Reject this defective implementation:** Check PATH alone without rustup which or actual invocation.

**Layer:** integration. **Execution:** not run on application.

### C02
**Stimulus:** Run preflight under real resource_group, with CI markers absent inside.

**Required observation:** Self-update policy applies in shared RUSTUP_HOME; no dependency on inherited CI marker.

**Reject this defective implementation:** Fix only by setting CI in outer workflow environment.

**Layer:** integration. **Execution:** not run on application.

### C03
**Stimulus:** Unmounted storage, uncapped cgroup, or wrong actual toolchain path.

**Required observation:** Fail before builds; no /tmp/system-disk fallback and no broad env import.

**Reject this defective implementation:** Skip launcher or accept root home to green CI.

**Layer:** integration. **Execution:** not run on application.

### C04
**Stimulus:** Fresh Kani install and cargo kani setup using exact pinned package.

**Required observation:** Setup actually exits0; version and proof canary execute under intended toolchain.

**Reject this defective implementation:** Treat installed binary or cached directory as successful setup.

**Layer:** integration. **Execution:** not run on application.

### C05
**Stimulus:** Assert-true canary followed by assert-false Kani canary.

**Required observation:** Positive proof succeeds; negative proof fails for reachable assertion, not build/env error.

**Reject this defective implementation:** Accept any nonzero as a successful negative control.

**Layer:** integration. **Execution:** not run on application.

### C06
**Stimulus:** Tool setup exits7 or receipt is missing/stale/partial.

**Required observation:** Workflow fails; registry marked not_run; no successful extended receipt.

**Reject this defective implementation:** Continue-on-error or reuse older receipt.

**Layer:** integration. **Execution:** not run on application.

### C07
**Stimulus:** Clean and warm-cache extended registry on target commit.

**Required observation:** Every required check executed with current commit/digest/exit; artifacts contain matching receipts.

**Reject this defective implementation:** Check only job green or artifact existence.

**Layer:** integration. **Execution:** not run on application.

### C08
**Stimulus:** PR changes only tools/ci/preflight or resource_group.py.

**Required observation:** Extended workflow is eligible; tests validate trigger rules.

**Reject this defective implementation:** Omit those paths from top-level filter.

**Layer:** workflow contract. **Execution:** not run on application.

### C09
**Stimulus:** Required-check ruleset review with applicable and nonapplicable PRs.

**Required observation:** No claim based on protected=true alone; no permanently-pending filtered required check.

**Reject this defective implementation:** Require filtered check without compatible trigger design.

**Layer:** configuration review. **Execution:** not run on application.

### C10
**Stimulus:** Preflight receives irrelevant secret-like environment variables.

**Required observation:** Only allowlisted diagnostics recorded; no complete env dump or forwarded credentials.

**Reject this defective implementation:** Import user service environment wholesale.

**Layer:** integration. **Execution:** not run on application.


## cross-cutting

### G01
**Stimulus:** Replay all new negative controls against frozen checks.

**Required observation:** Each defect fails its exact intended semantic gate; build/env errors are inconclusive.

**Reject this defective implementation:** Count any crash/nonzero as detected security defect.

**Layer:** verifier. **Execution:** not run on application.

### G02
**Stimulus:** Change expected outcome/oracle/spec during implementation.

**Required observation:** Policy drift fails; separate explicit policy commit and rerun baseline required.

**Reject this defective implementation:** Weaken assertion to make candidate pass.

**Layer:** process. **Execution:** not run on application.

### G03
**Stimulus:** Run unchanged fast/extended/browser/query/delivery controls after each admitted slice.

**Required observation:** Existing security/durability/oracle behavior remains required.

**Reject this defective implementation:** Disable failing existing test as unrelated.

**Layer:** integration. **Execution:** not run on application.

### G04
**Stimulus:** Independent observer measures candidate and baseline under frozen workload.

**Required observation:** Report p50/p95/p99, throughput, CPU, memory, FD/thread counts, cleanup and repeats; unresolved regression blocks claim.

**Reject this defective implementation:** Invent benchmark numbers or enlarge threshold after seeing result.

**Layer:** performance. **Execution:** not run on application.

### G05
**Stimulus:** Rebase/HEAD movement or mismatching original blob/anchor.

**Required observation:** Re-anchor only affected edits against new evidence; never apply stale numeric line substitutions.

**Reject this defective implementation:** Apply original line ranges to changed source.

**Layer:** provenance. **Execution:** not run on application.

