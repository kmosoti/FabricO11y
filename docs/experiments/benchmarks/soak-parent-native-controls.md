# Prospective finite R3 parent-stop native controls

Registered 2026-10-15; commands below are NOT RUN. The owner admits using the
new observation-mode launcher itself as the mechanism under test. The outside
launcher is its control plane; the finite workload and every descendant remain
in one actual5/6GiB, tasks1536, zero-swap service. No second service is created.
This exercises an unqualified launcher and establishes neither S1 nor FULL.

The observation mode sets exact parent limits before its first workload,
requires delegation and retains the7500-second hard maximum. The finite
controller has a60-second own alarm and no Fabric server/browser/VM. It records
real parent fields, process identities, source and output under private DATA
results. Real retain/empty-accounting behavior is unknown until executed.

From a separately pinned clean source, run serial commands with unique CASE
and OUT values, after their exact driver/hash/resource preflight is admitted:

```sh
env FABRIC_STORAGE_ROOT=/run/media/kmosoti/data/FabricO11y \
  python3 -B tools/resource_group.py --observe-final-group --delegate \
  --runtime-seconds 7500 -- env PYTHONPATH=tools/qualification:tools \
  python3 -B tools/qualification/soak_parent_control.py --case CASE --out OUT
```

| CASE | Workload | Prospective observation/outcome |
| --- | --- | --- |
| `positive` | Read real caps, delegate to bounded supervisor, write actual identity, return0. | Actual MainPID0/exited retains same cgroup inode and complete empty accounting; launcher0 with confirmed stop. |
| `orphan` | Spawn an owned120-second idle child, record live identity, return0. | If kernel parent remains populated, observer refuses and owned stop kills/reaps it; launcher2. If systemd itself removes the child before observation, retain as inconclusive for orphan detection, never a detected-control claim. |
| `early` | Record then return7. | Actual failed unit/inside exit7 retained; no successful empty-stop trial, launcher2. |
| `missing` | Record and authenticate exact own inside-wrapper PID/start/cgroup/cmdline, then SIGKILL that wrapper. | Missing completed receipt/nonzero native wrapper outcome is refused; remaining owned descendants stopped; launcher2. |

Each trial reserves fresh1GiB DATA, strict namespace census/admission below95GB,
and complete stdout/stderr/launcher/native receipts. Never call an unrelated
PID or cgroup a fixture. After each command confirm no owned service/process
survives, preserve actual failed output, and remove only verified marker-owned
temporary state after archival. Changed actual kernel behavior, setup failure,
timeout, unconfirmed cleanup or mismatched expected exit remains failed or
inconclusive. No shortened controller timer stands in for7200-second exposure.
