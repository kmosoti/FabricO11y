# Closed-evidence reclamation continuation

Status: registered after reclaim01's timeout and before retry02. The initial
[registration](coupled-reclamation-protocol.md) and its180-second deadline remain
historical. Reclaim01 exited1 after180.905seconds, preserving1241 completed
transformations and their manifests. Its negative controls rejected changed
payload, truncation, missing canonical files and unexpected links. Timeout is
not a successful full compaction result.

Retry02 uses the same scopes, exact-byte comparisons, source/protocol receipt,
caps, locking, cleanup and180-second deadline. Already shared inodes are skipped;
all candidates still undergo discovery and ownership checks. It charges the same
preparation/frontier budget. It must not modify reclaim01's receipts or verdict.
Register this separate retry protocol as allowed provenance input to the helper.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_reclaim.py --id catalog-coupled-reclaim-02 --protocol docs/experiments/benchmarks/coupled-reclamation-retry-protocol.md
```

After retry, root archives and removes only reclaim01's retained launcher scratch
through the existing exact catalog cleanup helper (60-second wrapper deadline,
4MiB evidence reservation). The failed helper already removed its inner control
scratch; original failure/provenance remains in the coordinator.

Both transformation manifests receive a separate complete readback in the
continuation closeout. Prepared/replaced pairs must match; a retained object must
have the recorded new compressed and decoded identity and the canonical inode.
Closeout also runs manual documentation checks, records new Python syntax checks,
copies exact launcher receipts, confirms all new units/scratch inactive/removed,
and inventories unchanged evidence caps. Inject a changed hash into the readback
checker and require rejection before grading retained evidence. Run through the
ordinary contained serial launcher, verification stage180seconds/reserve4MiB.
No Rust changed in this continuation; prior17-gate fast receipts keep their
revision, while the new harness/documentation checks get new receipts.
