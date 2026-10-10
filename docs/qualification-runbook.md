# Target-host qualification runbook

Use a registered operating profile, frozen artifacts and independent oracles to
record a target-host result. No current deployment profile is qualified. The
[capability ledger](QUALIFICATION.md#capability-ledger) owns qualification status;
finite acceptance on another host does not establish deployment qualification.

The [unified-cgroup installation run](experiments/formal/installation-acceptance-run-02.md)
passed seventeen baseline checks and rejected all three required mutations.
The [companion-compatible R2 soak](experiments/benchmarks/soak-run-02.md) passed
all ten gates. Their historical failed and inconclusive predecessors remain
recorded. These results replace neither a new host's profile registration nor
its measured evidence.

## Admit the host and protocol

Record CPU model/count, RAM, kernel, distribution, filesystem, virtualization,
cgroup hierarchy, storage availability and concurrent services before execution.
The target profile is defined in [qualification](QUALIFICATION.md#registered-workload-and-decision-rule);
use the registered placement and workload, or register a new revision first.

| Claim | Registered procedure |
| --- | --- |
| Installation | [Acceptance protocol](experiments/formal/installation-acceptance-protocol.md), [isolated build/QEMU plan](experiments/formal/installation-acceptance-qemu-plan.md) |
| History | [History revision 1](experiments/benchmarks/alpha-phase4-history-protocol.md) |
| Stress | [Stress revision 1](experiments/benchmarks/alpha-phase5-stress-protocol.md) |
| Outage | [Outage protocol](experiments/benchmarks/alpha-phase5-outage-protocol.md) |
| Soak with production companion | [Soak R2](experiments/benchmarks/soak-protocol-r2.md) |
| Fleet | [Fleet protocol](experiments/benchmarks/alpha-phase3-fleet-protocol.md) |

Every build, test, validator and trial uses the
[resource launcher](CONTRIBUTING.md#resource-containment), sequentially.
Local limits are 16/20 GiB memory high/max, zero swap and a 30-minute deadline;
remote hosts need explicit smaller budgets suited to their existing services.
A longer deadline needs explicit owner scope. The R2 continuation authorized
7,500 seconds for that finite full trial; this is not blanket authorization for
new qualification runs. Verify mount, aggregate storage and enforcement before
admission; no system-disk or RAM-backed fallback is allowed.

## Build and freeze the companion workflow

The tested R2 workflow uses `prepare_soak.py` and `verify_soak_freeze.py`.
Legacy `freeze.sh` and its flat standalone-server harness layout are not the
current companion workflow; they assume repository `target/release` rather than
the launcher's data-backed `CARGO_TARGET_DIR`. Do not use them for a new R2 run.
Historical protocol commands retain their original text and outcomes.

The normal writer needs no experiment flags. Build all five native artifacts
without allocation instrumentation, explicitly clearing historical selectors:

```sh
python3 tools/resource_group.py -- env \
  -u FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT -u FABRIC_PAGE_STORE_EXPERIMENT \
  -u FABRIC_RUN_MIB_EXPERIMENT -u FABRIC_EARLY_ROW_RELEASE_EXPERIMENT \
  -u FABRIC_SPILL_WORKSPACE_EXPERIMENT \
  cargo build --release --offline --locked \
  -p fabric-server --bin fabric-server --example server_dump \
  -p fabric_o11y --bin fabric-node --example spindle_sim --example spool_dump
```

Choose a fresh identifier and retain build command, toolchain, source snapshot
and binary hashes alongside the run. The freeze helper refuses an existing
owned path and checks source stability; it does not infer build flags from a hash.

```sh
FABRIC_DATA=/run/media/kmosoti/data/FabricO11y
SOAK_ID=local-r2-next
SOAK_FROZEN="$FABRIC_DATA/scratch/soak-r2-$SOAK_ID/frozen"
SOAK_RESULTS="$FABRIC_DATA/results/soak-r2-$SOAK_ID"
mkdir "$SOAK_RESULTS"
python3 tools/resource_group.py -- env FABRIC_SCRATCH_ROOT="$FABRIC_DATA/scratch" \
  python3 -B tools/qualification/prepare_soak.py --id "$SOAK_ID"
python3 tools/resource_group.py -- python3 -B tools/qualification/verify_soak_freeze.py \
  --manifest "$FABRIC_DATA/scratch/soak-r2-$SOAK_ID/provenance/manifest.json" \
  --label smoke-pre --out "$SOAK_RESULTS/smoke-pre.json"
```

The helper prints the actual frozen path. It copies known binaries, harness
imports, containment helpers and source/protocol provenance onto the data drive.
Verification receipts must be fresh and outside the owned freeze. Invoke the
repository verifier as above; the frozen harness is the trial input.

## Smoke, then admitted full trial

```sh
python3 tools/resource_group.py --delegate -- \
  env FABRIC_SCRATCH_ROOT="$FABRIC_DATA/scratch" \
  python3 -B "$SOAK_FROZEN/tools/qualification/runner.py" \
  --out "$SOAK_FROZEN/target/alpha-soak-r2-smoke" --duration-s 600 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B "$SOAK_FROZEN/tools/qualification/soak_tier.py" \
  --seed 0xA11FA001 --bin-dir "$SOAK_FROZEN/bin" \
  --server-cpus 0-1 --sim-cpus 2-3 --companion --smoke
```

This is the registered four-CPU finite acceptance placement, not the distinct
qualification target profile. The runner requires its direct `target/alpha-*`
output below `FABRIC_SCRATCH_ROOT`; nested frozen mini-trees are allowed.
Repository-root `target/alpha-*` outside that root is rejected before creation
or process launch. Keep the ownership marker and symlink protections intact.

Run a distinct postcheck with the same manifest and compare manifest hashes.
Follow R2 smoke admission exactly: production exits, source-complete companion
custody, containment and cleanup must succeed. Its below-64-MiB workload can
produce only the expected sealing-gate failure; record the real exit 1.
A smoke is never a measured soak or qualification pass. Only after admission
and owner deadline scope, use the full command in [R2](experiments/benchmarks/soak-protocol-r2.md),
with the same `env FABRIC_SCRATCH_ROOT="$FABRIC_DATA/scratch"` wrapper after
the launcher. The launcher otherwise selects a distinct temporary work root,
which cannot contain this persistent freeze. Recheck frozen bytes and require
every registered gate. Do not alter thresholds, oracles or workload placement
to obtain a passing result.

## Preserve, clean and report

Retain exact commands/exits, host/profile, frozen manifest, source and binary
hashes, simulator ledger, independent companion Spool dump, recovery transcript,
window metrics, gate summaries, containment receipts and cleanup outcome.
Validate archives and copied manifests before deleting owned scratch, and verify
that descendants and groups are stopped. Count archives against the owner's
100 GB total storage ceiling. Keep failures and interrupted runs as such.

Write a fresh run record with finite scope and update the capability ledger,
verification matrix and current state from executed evidence. Protocol, oracle
and verifier changes belong in separate commits with reasons. Final checks use
the launcher, including documentation validators. A green test suite or a local
acceptance result alone is not target-host qualification.
