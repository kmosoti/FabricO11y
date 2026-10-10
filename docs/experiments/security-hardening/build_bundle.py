from pathlib import Path
import json, hashlib
ROOT = Path(__file__).resolve().parent
BASE = '18f6b367d0f34886847ea25b9168e50eb0425300'
BLOB = {
 'AGENTS.md':'26f216804485c8a2a3263faec117e0602d2c3480',
 'src/bin/fabric-node.rs':'f71b14c58d7afbb7932d3aaf61648980f1efed23',
 'crates/fabric-server/src/console.rs':'08dc1b3028bcfc33ba264065212a285de1f9f7cf',
 'crates/fabric-server/src/lib.rs':'cb4e43f9d14b994beb0e9879e0d400e4e1b205db',
 'src/spindle/otlp.rs':'fc1369a86820f4ef43b2a09cbdcf9b2add84ce21',
 'crates/fabric-adapter-linux/src/log_source.rs':'c725c9a38bad3a613edf886bec2d8b5f76fdadd9',
 'tools/ci/prepare-resource-host.sh':'adec2633322f0f431865fd9e5e00dbc4cc2c4d0e',
 'tools/ci/install-cargo-tool.py':'6308af4f0ae5d4f1747903f58556853344435ca6',
 'tools/resource_group.py':'dbdb1b6a0d6a1300d75b818c9829088e61b1c043',
 '.github/workflows/verification.yml':'31646eeda3b9ac7105696156dcf9c42e87392f9b',
 '.github/workflows/rust.yml':'66dbd9977d2491e94ed4c721e11d89ef6abd2dcf',
 'xtask/checks.json':'c8b7237b29c1e3bdfd9dca0348cc3897a7dce297',
}
def url(path, start=None, end=None):
    value=f'https://github.com/kmosoti/FabricO11y/blob/{BASE}/{path}'
    if start is not None:
        value+=f'#L{start}'
        if end is not None and end!=start: value+=f'-L{end}'
    return value
EDITS=[]
def edit(id, path, start, end, anchor, operation, tests):
    EDITS.append({'id':id,'path':path,'original_start_line':start,'original_end_line':end,
                  'symbol_or_text_anchor':anchor,'operation':operation,'tests':tests,
                  'base_blob_sha':BLOB.get(path),'source_url':url(path,start,end),
                  'execution_status':'proposed_not_applied'})
edit('ADM-00','crates/fabric-server/src/console.rs',3,24,'use crate::access',
     'Declare private admission module; add only required connection/time imports; preserve query permit types.','A01 A11 A14'.split())
edit('ADM-01','crates/fabric-server/src/console.rs',30,41,'pub struct Console',
     'Replace requests, ceremonies, auth_budget and private_auth_budget with one shared bounded admission controller; retain queries. Add immutable configured_origin.','A01 A02 A03 A07 A11'.split())
edit('ADM-02','crates/fabric-server/src/console.rs',207,224,'pub fn new(',
     'Capture exact validated access.origin before moving access; construct production admission policy once.','A01 A03 A13'.split())
edit('ADM-03','crates/fabric-server/src/console.rs',320,346,'async fn auth_admission',
     'Use transport peer key and public lane before stateful work/body extraction; remove fixed-second global budget. Retain origin/version checks and deadline.','A02 A04 A05 A06 A08 A12 A13'.split())
edit('ADM-04','crates/fabric-server/src/console.rs',348,367,'async fn authorize(',
     'Acquire private-lane guard in outer wrapper; hold it through denial audit. Limiter rejection uses bounded counters, not durable per-request audit.','A03 A08 A09 A10'.split())
edit('ADM-05','crates/fabric-server/src/console.rs',369,399,'async fn authorize_request',
     'Remove old request semaphore and private fixed-second budget only; preserve credential parsing, action checks, freshness/CSRF and result revalidation.','A02 A09 A12 A14'.split())
edit('ADM-06','crates/fabric-server/src/lib.rs',120,123,'.serve(app.into_make_service())',
     'Use app.into_make_service_with_connect_info::<std::net::SocketAddr>() at native TLS composition root.','A01 A04 A13'.split())
edit('OTLP-00','src/spindle/otlp.rs',13,22,'use crate::spindle::runtime::Spindle;',
     'Declare deadline_io module and add Instant/TrySendError imports as needed; keep existing public parser API.','O05 O06 O08 O09'.split())
edit('OTLP-01','src/spindle/otlp.rs',24,34,'const MAX_CONNECTIONS: usize = 32;',
     'Retain body/header/queue/connection caps. Add bounded transport phase, connection-age and request-count policy.','O01 O02 O07 O12'.split())
edit('OTLP-02','src/spindle/otlp.rs',67,98,'pub fn start(',
     'Reject non-loopback in start too; replace manual live counter ownership with RAII permit acquired before spawning.','O01 O02 O03 O04 O13'.split())
edit('OTLP-03','src/spindle/otlp.rs',194,203,'fn respond(',
     'Write through absolute-deadline-aware helper; close on expiry/error without recursive response attempts.','O07 O08 O11'.split())
edit('OTLP-04','src/spindle/otlp.rs',206,259,'fn serve(',
     'Apply absolute header/body/commit/response deadlines, keep-alive idle cap, total age and request-count cap. Preserve confirmed durable-commit-before-200.','O05 O06 O07 O08 O09 O10 O11 O12 O14'.split())
edit('OTLP-05','src/spindle/otlp.rs',241,248,'tx.send(Export { body, reply })',
     'Use try_send; distinguish never-enqueued failure from enqueued timeout/unknown completion. Do not discard accepted work on waiter timeout.','O09 O10 O11'.split())
edit('FS-00','crates/fabric-adapter-linux/src/log_source.rs',5,8,'use std::fs::OpenOptions;',
     'Declare private secure_open helper module and public checked-file entrypoint; preserve imports still needed by nested tests.','F01 F02 F03'.split())
edit('FS-01','crates/fabric-adapter-linux/src/log_source.rs',92,105,'pub fn unread_bytes',
     'Replace open plus metadata check with shared all-component secure regular-log opener returning the checked descriptor.','F01 F02 F03 F04 F05 F09'.split())
edit('FS-02','crates/fabric-adapter-linux/src/log_source.rs',139,162,'fn read_lines_with_identity',
     'Use the same secure opener; keep metadata, Btrfs identity, cursor-prefix checks and reads on that descriptor.','F01 F02 F03 F06 F07 F08 F09 F10'.split())
edit('FS-03','src/bin/fabric-node.rs',167,177,'libc::O_NONBLOCK | libc::O_NOFOLLOW',
     'Replace companion log preflight with the same all-component secure regular-log opener; preserve startup diagnostics and source-count semantics.','F01 F02 F03 F05 F08 F09'.split())
edit('CI-01','.github/workflows/verification.yml',10,23,'pull_request:',
     'Extend trigger paths to tools/ci/** and tools/resource_group.py; preserve existing coverage; review required-check filtering before enabling a merge gate.','C01 C08 C09'.split())
edit('CI-02','.github/workflows/verification.yml',39,42,'toolchain: 1.99.0',
     'Insert contained toolchain preflight after toolchain setup; explicitly disable rustup auto-self-update in shared RUSTUP_HOME.','C01 C02 C03 C04'.split())
edit('CI-03','.github/workflows/verification.yml',68,77,'cargo kani setup',
     'Keep pinned Kani; capture setup and canary evidence; full extended registry must really execute and fail on nonzero/incomplete results.','C04 C05 C06 C07'.split())
edit('CI-04','.github/workflows/rust.yml',18,22,'targets: wasm32-unknown-unknown',
     'Insert same contained preflight before first tool install. No blanket environment forwarding.','C01 C02 C03 C10'.split())
edit('CI-05','tools/ci/prepare-resource-host.sh',51,62,'CARGO_HOME=',
     'Inspect-only under minimal candidate: verify relocated homes; do not move to system disk. Change only if diagnosis disproves the minimal repair.','C01 C02 C03'.split())
edit('CI-06','tools/resource_group.py',155,169,'for key in',
     'Inspect-only under minimal candidate: preserve allowlist and containment; diagnose CI-marker loss without importing the full outer environment.','C02 C03 C10'.split())
edit('GATE-01','xtask/checks.json',4,21,'"checks": [',
     'Add separate policy-commit entries for independent security checker tests, security negative controls and CI preflight tests. Preserve existing gates.','G01 G02 G03 G04 G05'.split())

TESTS=[]
def test(id, group, stimulus, expect, negative, layer='integration'):
    TESTS.append({'id':id,'group':group,'stimulus':stimulus,'required_observation':expect,
                  'negative_control':negative,'layer':layer,'status':'not_run_on_application'})
test('A01','admission','Native HTTPS request with real ConnectInfo, then same request without transport metadata.','Real socket peer is recognized; missing metadata fails closed before body/access mutation.','Replace missing peer with localhost/default shared identity.')
test('A02','admission','One peer floods public and private lanes; a distinct peer performs valid requests.','Source/global rate bounds and source concurrency caps hold; all low-rate honest requests in frozen trace are admitted.','Delete peer bucket; or debit global token before checking peer eligibility.','model+integration')
test('A03','admission','Slow bodies from one source fill its own request allowance.','Public source holds <=2 of 8 slots; private source <=4 of16; another peer retains admission headroom.','Apply rate limits but omit per-peer in-flight caps.')
test('A04','admission','Same TCP peer rotates X-Forwarded-For, Forwarded, X-Real-IP and source port.','All requests debit the same trusted peer key.','Use header or source port in PeerKey.')
test('A05','admission','Compare IPv4, IPv4-mapped IPv6, and multiple addresses within one IPv6 /64.','v4-mapped address shares IPv4 bucket; configured /64 grouping is enforced.','Key raw SocketAddr or rotate /128 within prefix.','unit')
test('A06','admission','t=999ms/1001ms boundary, large elapsed time, repeated same time, backward injected time.','No fixed-window double burst; integer credit invariant holds; backward time gives no credit.','Reset at wall-clock second or use wrapping time arithmetic.','property')
test('A07','admission','Fill 1024 peer entries; hold leases; attempt churn/expiry/recreation.','No live entry evicted; no debt reset; no more than1024 entries; existing peers remain serviceable.','Evict an indebted/live entry and recreate it with full burst.','property+integration')
test('A08','admission','Drop waiter mid-body, handler error, task abort, panic in isolated fixture.','Every owned request permit releases exactly once; no counter underflow or leaked admission.','Retain manual counter decrement after handler return.','integration')
test('A09','admission','Unauthorized request triggers slow denial audit; policy changes during read.','Outer private guard covers audit; original scope and final authority recheck still enforce denial.','Drop guard before record_denial; remove final recheck.')
test('A10','admission','Sustained limiter rejections at registered bound.','Only bounded low-cardinality counters; no token/IP/path payloads or durable write per rejection.','Persist every429 with supplied path/token.')
test('A11','admission','Production defaults and reduced test policy under deterministic clock.','Public8/s burst8 global,2/s burst4 peer; private32/s burst32 global,8/s burst8 peer; table1024.','Accidentally expose test-only override via environment.','unit')
test('A12','admission','Invalid/duplicate cookie, invalid bearer, bad CSRF/origin, stale version, revoked/expired/delegated credentials.','No authorization or session-semantic regression; body remains unread for header-stage rejection.','Treat admission as authorization or bypass fresh verification.')
test('A13','admission','Run actual native TLS fixture and packaged browser passkey login; run direct Router tests with explicit MockConnectInfo.','Production receives real peer info; test mocks never enable production fallback; login completes.','Only test request extensions by hand; native TLS path remains unconfigured.')
test('A14','admission','Cancel expensive query waiter while blocking query continues.','Existing two query permits remain owned until actual query worker ends.','Release query permit when HTTP future times out.')

test('O01','OTLP','Invoke check_listen and start with loopback/non-loopback addresses.','Both reject non-loopback; existing loopback config remains compatible.','Validate only config string, not start API.','unit+integration')
test('O02','OTLP','Open 32 simultaneous connections and a33rd in synthetic loopback fixture.','At most32 connection workers; extra connection promptly closed; permit count returns to zero.','Reserve slot only after thread spawn.')
test('O03','OTLP','Inject thread-spawn failure immediately after permit ownership transfer.','Permit is released once; listener still accepts a later valid connection.','Manual failure decrement plus RAII causes double release.','unit')
test('O04','OTLP','EOF, parse failure, I/O error and test panic on worker.','RAII releases slot on each exit; no manual normal-return-only accounting.','Move release back to statement following serve.','unit+integration')
test('O05','OTLP','Send incomplete header bytes more frequently than inactivity timeout.','Connection closes at fixed header deadline within frozen scheduler tolerance.','Set socket timeout once; extend deadline whenever byte arrives.')
test('O06','OTLP','Advertise valid bounded Content-Length and dribble body forever.','Absolute body deadline closes connection; no Export queued, no200.','Use read_exact with only a fixed per-read timeout.')
test('O07','OTLP','Persistent keep-alive/pipelined requests, including already-buffered next header.','Idle/total-age/request-count limits apply even to buffered bytes; final response advertises close.','Reset connection age on each request or forget buffered path.')
test('O08','OTLP','Peer does not read replies; induce partial writes and EAGAIN/EINTR.','Write deadline reapplied to every low-level write; closes without retrying a second response.','Unbounded write_all or timeout reset on progress.')
test('O09','OTLP','Fill queue64 with consumer blocked, submit another complete bounded export.','Immediate overload path from try_send; never blocks before commit timer.','Restore SyncSender::send.')
test('O10','OTLP','Enqueue succeeds; waiter expires; consumer later durably commits.','No false200 and no false assertion that data never committed; retained work follows existing custody semantics.','Delete accepted queue item on timeout or label timeout as definitely uncommitted.')
test('O11','OTLP','Full Spool, pause, failed commit, dropped reply channel, confirmed successful commit.','Only confirmed durable successful commit yields200; error/unknown returns503 or closes.','Send200 when merely queued; spoof success after timeout.')
test('O12','OTLP','Maximum headers/bodies/queue/32workers plus repeated reconnects under node memory cap.','Memory/tasks bounded, no growth trend after drain; report that reconnecting hostile locals can still cause denial.','Call finite connection lifetime an isolation guarantee.')
test('O13','OTLP','Close/drop fixture listener and await all fixture workers.','Tests leave no listener/thread/scratch ownership leaks; cleanup timeout is failure.','Detach fixture threads and infer cleanup from client close.')
test('O14','OTLP','Existing SDK-shaped protobuf requests, bad media type, duplicate length, transfer encoding, oversize request.','Existing accepted valid format and meaningful error classes preserved; limits never broadened.','Raise body cap or accept unsupported transfer encoding to pass test.')

test('F01','filesystem','Open regular approved log through every production read/backlog entrypoint.','Same descriptor supplies fstat/identity/prefix/read; normal collection remains exact.','Securely check path, then reopen it by name.')
test('F02','filesystem','Approved pathname is a leaf symlink to synthetic sentinel.','Read and backlog functions reject; no sentinel bytes collected.','Remove O_NOFOLLOW/NO_SYMLINKS.')
test('F03','filesystem','Parent directory component is symlink; leaf is ordinary regular file.','Both entrypoints reject traversal; no sentinel bytes collected.','Use O_NOFOLLOW only on final component.')
test('F04','filesystem','Use procfs magic-link path to a synthetic open FD.','Rejected by all-component policy without reading target.','Rely on regular-file fstat after following link.')
test('F05','filesystem','FIFO with no writer, Unix socket, directory, and unprivileged /dev/null case.','No reads from nonregular source, no FIFO hang, descriptor closed.','Remove O_NONBLOCK or regular-file check.')
test('F06','filesystem','Deterministic controlled rename before secure open and after returned descriptor.','Before-open link is rejected; after-open replacement cannot redirect existing FD; no filename reopen.','Canonicalize then open or metadata-check then open.')
test('F07','filesystem','Rotation, truncation, invalid UTF8, oversize line, Btrfs namespace change.','Existing cursor/gap/oversize semantics retained; failures never advance unseen bytes.','Advance cursor on denied open.')
test('F08','filesystem','Inject ENOSYS, EPERM, EINVAL and ELOOP from secure opener.','Visible per-source failure; no insecure fallback; other collectors remain available.','Retry via ordinary File::open after secure-open error.')
test('F09','filesystem','Source aliases through an intended distribution symlink; absolute .. component; canonical explicit path.','Migration is explicit; alias rejected, canonical approved path works; no hidden scope normalization.','Auto-canonicalize or silently rewrite configured/granted paths.')
test('F10','filesystem','Synthetic hardlink under trusted UID and synthetic replaced ordinary file.','Document/test that symlink policy is not hardlink/inode-origin authentication.','Claim all arbitrary-file redirection is prevented.')

test('C01','CI','Clean hosted runner with relocated CARGO_HOME/RUSTUP_HOME and shared preflight.','Record actual launcher/toolchain paths; native/WASM pinned compiler resolves to intended storage.','Check PATH alone without rustup which or actual invocation.')
test('C02','CI','Run preflight under real resource_group, with CI markers absent inside.','Self-update policy applies in shared RUSTUP_HOME; no dependency on inherited CI marker.','Fix only by setting CI in outer workflow environment.')
test('C03','CI','Unmounted storage, uncapped cgroup, or wrong actual toolchain path.','Fail before builds; no /tmp/system-disk fallback and no broad env import.','Skip launcher or accept root home to green CI.')
test('C04','CI','Fresh Kani install and cargo kani setup using exact pinned package.','Setup actually exits0; version and proof canary execute under intended toolchain.','Treat installed binary or cached directory as successful setup.')
test('C05','CI','Assert-true canary followed by assert-false Kani canary.','Positive proof succeeds; negative proof fails for reachable assertion, not build/env error.','Accept any nonzero as a successful negative control.')
test('C06','CI','Tool setup exits7 or receipt is missing/stale/partial.','Workflow fails; registry marked not_run; no successful extended receipt.','Continue-on-error or reuse older receipt.')
test('C07','CI','Clean and warm-cache extended registry on target commit.','Every required check executed with current commit/digest/exit; artifacts contain matching receipts.','Check only job green or artifact existence.')
test('C08','CI','PR changes only tools/ci/preflight or resource_group.py.','Extended workflow is eligible; tests validate trigger rules.','Omit those paths from top-level filter.','workflow contract')
test('C09','CI','Required-check ruleset review with applicable and nonapplicable PRs.','No claim based on protected=true alone; no permanently-pending filtered required check.','Require filtered check without compatible trigger design.','configuration review')
test('C10','CI','Preflight receives irrelevant secret-like environment variables.','Only allowlisted diagnostics recorded; no complete env dump or forwarded credentials.','Import user service environment wholesale.')

test('G01','cross-cutting','Replay all new negative controls against frozen checks.','Each defect fails its exact intended semantic gate; build/env errors are inconclusive.','Count any crash/nonzero as detected security defect.','verifier')
test('G02','cross-cutting','Change expected outcome/oracle/spec during implementation.','Policy drift fails; separate explicit policy commit and rerun baseline required.','Weaken assertion to make candidate pass.','process')
test('G03','cross-cutting','Run unchanged fast/extended/browser/query/delivery controls after each admitted slice.','Existing security/durability/oracle behavior remains required.','Disable failing existing test as unrelated.','integration')
test('G04','cross-cutting','Independent observer measures candidate and baseline under frozen workload.','Report p50/p95/p99, throughput, CPU, memory, FD/thread counts, cleanup and repeats; unresolved regression blocks claim.','Invent benchmark numbers or enlarge threshold after seeing result.','performance')
test('G05','cross-cutting','Rebase/HEAD movement or mismatching original blob/anchor.','Re-anchor only affected edits against new evidence; never apply stale numeric line substitutions.','Apply original line ranges to changed source.','provenance')

manifest={'schema_version':1,'spec_id':'FABRIC-SECURITY-HARDENING-2026-10-10',
 'repository':'kmosoti/FabricO11y','base_commit':BASE,'status':'proposed_not_applied',
 'line_number_semantics':'1-based original commit ranges; never final line numbers or a patch',
 'locked_sources':[{'path':p,'blob_sha':s,'url':url(p)} for p,s in BLOB.items()],
 'edits':EDITS,
 'new_files':[{'path':p,'status':'proposed; verify absence before creating'} for p in [
  'crates/fabric-server/src/console/admission.rs',
  'crates/fabric-server/src/console/admission_tests.rs',
  'src/spindle/otlp/deadline_io.rs','src/spindle/otlp/admission_tests.rs',
  'crates/fabric-adapter-linux/src/log_source/secure_open.rs',
  'crates/fabric-adapter-linux/src/log_source/secure_open_tests.rs',
  'tools/ci/security_preflight.py','tools/ci/test_security_preflight.py',
  'tools/security/check_admission.py','tools/security/test_check_admission.py',
  'tools/security/check_transport.py','tools/security/test_check_transport.py',
  'docs/experiments/security-hardening-protocol.md']],
 'test_catalogue':'acceptance-tests.json','stage_order':['R0-freeze','R1-ci','R2-admission','R3-otlp','R4-filesystem','R5-integration'],
 'github_mutations_performed':False,'application_tests_run':False}
(ROOT/'change-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
(ROOT/'acceptance-tests.json').write_text(json.dumps({'schema_version':1,'base_commit':BASE,'status':'proposed_not_run','tests':TESTS},indent=2)+'\n')
rows=['# Original-source edit map\n',f'Base commit: `{BASE}`. All ranges are **before** any edit.\n',
      '| ID | Original source | Required operation | Acceptance tests |\n|---|---|---|---|\n']
for e in EDITS:
    rows.append(f'| {e["id"]} | [{e["path"]}:{e["original_start_line"]}-{e["original_end_line"]}]({e["source_url"]}) | {e["operation"]} | {", ".join(e["tests"])} |\n')
(ROOT/'original-source-edit-map.md').write_text(''.join(rows))
rows=['# Acceptance test catalogue\n', 'These are requirements, not executed FabricO11y test results. A negative control must fail for the named defect, not for a compiler, fixture or environment error.\n']
for group in dict.fromkeys(t['group'] for t in TESTS):
    rows.append(f'\n## {group}\n\n')
    for t in TESTS:
        if t['group']==group:
            rows.append(f'### {t["id"]}\n**Stimulus:** {t["stimulus"]}\n\n**Required observation:** {t["required_observation"]}\n\n**Reject this defective implementation:** {t["negative_control"]}\n\n**Layer:** {t["layer"]}. **Execution:** not run on application.\n\n')
(ROOT/'acceptance-tests.md').write_text(''.join(rows))
print(f'{len(EDITS)} original-source edits, {len(TESTS)} test specifications, {len(BLOB)} pinned source blobs')
