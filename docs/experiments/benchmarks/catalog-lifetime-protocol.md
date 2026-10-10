# CR3: real catalog Sources lifetime and cancellation controls

Status: **registered before execution; no outcome claimed**. Complements CQ2 many-Segment
performance; replaces no expired-token or retention oracle gate.
H1: multiple actual shared Sources reuse metadata identity; retired metadata/
filters remain reachable only through held views and release after final drop or
actual task cancellation. H0: views clone/retain unintended owners, cache purge
leaks retired allocations, or dropping/canceling views fails to release them.

Use real eight-log Segments built through `segment::build`, seed2703163393,
Unicode node/body, fixed sequences/times and real generated text filters.
Create a native empty journal/commit thread through Store, then a shared Walk
History. Acquire two Sources, count unique `Arc::as_ptr` identities once per
physical manifest, and verify pointer equality of their real filter handles.
Publish four further Segments while views remain held; acquire current Sources
and require six unique manifest owners, not ten references counted as ten bytes.
Same-content detached `Arc<Manifest>` is a negative identity control.

Invoke real sealer retention with zero retained-byte allowance, refresh the
catalog, and acquire an empty current view. Old Weak metadata/filter handles
must remain upgradable while old Sources are held. Actual Segment directories
must already be deleted and physical reads must fail: Arc metadata does not
grant custody or authorize retaining filesystem history. Drop current/old views
one by one; the final drop must make retired Weak handles unupgradable.

A separate current-thread Tokio test moves actual Sources into a future, waits
for its pause acknowledgement, performs actual retention/cache refresh, verifies
the paused future still owns retired metadata, aborts that task and awaits its
canceled JoinHandle. Only then must Weak manifest/filter handles expire.
This exercises real Rust async cancellation/drop, not a simulated reference map.
It is not an HTTP disconnect test or a process-memory bound. Operations' real
History Gone checks separately establish that expired tokens do not resurrect.
Do not infer a metadata lease, use unsafe code or alter persistent bytes.

Root registers separately before execution and runs only through containment:

```sh
python3 -B tools/resource_group.py -- cargo test --offline --locked -p fabric-server --lib catalog_lifetime_tests -- --nocapture
```

Optional coordinator receipt: id `catalog-lifetime-01`, memory/capacity stage,
600s command limit inside default outer deadline. Mounted scratch fixture is
below8MiB, drive reserve16GiB preflight;20GiB/no swap enforcement unchanged.
Capture exact command/exit, revision/source/protocol hashes, two printed control
receipts, cgroup/IO observations and cleanup. Successful fixtures/commit threads
are removed/joined; panic retains owned origin/state for archival. No workloads
are executed by the preparing PI. Unique-owner counts are allocation identities,
not allocator bytes/RSS; CQ2 supplies separate measured memory/performance.
