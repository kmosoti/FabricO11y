# Coupled completion dispatch

Registered before new execution, 2026-10-07. Owner's “Proceed” accepts the
concrete [completion plan](coupled-completion-plan.md), including its explicitly
described O6 checker/fixture mapping and O7 prospective soundness investigation.
Implementations, checker/semantic changes and registrations remain separate
logical changes under ADR-0018. No historical failed outcome is rewritten.

The remaining frontier is4658.714280seconds before this dispatch. Keep existing
stage limits and aggregate16/20GiB high/max, zero swap, serial cgroup workloads,
30-minute outer deadline,8GiB scratch and16GiB free-drive floor. Source/evidence
and failure archives count. New capacity datasets use `catalog-borrowed-c5-*`
so the existing capacity assignment applies; capacity failure archives are also
charged there. No implicit budget renewal. Reserve at most80MiB further retained
bytes:20capacity,8query,20operations,32coordinator/failures, subject to the actual
remaining192MiB incremental and2GiB aggregate ceilings. Concrete cell reservations
must include failure evidence and binary/source archives.

Admission now checks the reconciled query upper bound from
`coupled_resource_readiness.py` inside `coupled_admit.observe`, with the entire
requested reserve conservatively charged to query. All unassigned/shared inode
paths still charge query; exact capacity assignments are excluded only if every
observed path to an inode has that assignment. Existing original capacity failure
is reported separately from the prospective832MiB assignment. Representative
length/block overages and alias/assignment controls run before readiness approval.

First command verifies only resource readiness, not native performance:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-dispatch-audit-01 --lab coordinator --stage preparation --seconds 90 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_resource_readiness.py
```

O6 companion semantics and negative controls get their own registration and
commit before fixture execution. Reproduce raw-custody loss before correction.
O7 native bounds/identity checker and exact commands get their own registration
before execution. A soundness result cannot retroactively pass C2's physical
identity gate. C5/O8 controls precede source/binary freezes and actual service
protocols. No timing begins while an agent edits its measured source or harness.
Stop dependent service admission for unresolved semantic defects, insufficient
resource/time projection or failed ownership/coverage controls. Record the actual
completion/deferred disposition instead of waiving a gate.
