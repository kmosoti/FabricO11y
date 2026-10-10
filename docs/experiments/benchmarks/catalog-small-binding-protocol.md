# Small commit groups before index promotion

Registered after the [first algorithm round](catalog-algorithm-round-protocol.md)
and before this follow-up executes. That round's complete
[raw summary](data/catalog-algorithm-round-01/summary.json) rejected commit-planning
H1: eight distinct bindings took1.436 times the legacy median, despite ratios
of0.418/0.084 at256/2048. The original failure remains evidence. Query and
collection candidates are unchanged by this follow-up.

## Hypothesis and mechanism

The two secondary indexes cost more than their saved comparisons for small
groups. Keep the first binding inline, retain additional distinct bindings in
a compact list while at most32 reverse bindings exist, and promote once to the
two ordered indexes beyond that population. A repeated accepted binding must
not enlarge the list. Preserve the first forward mapping and every accepted
reverse binding even when explicit durable facts change. Promotion transfers
ownership; it must not leave an indefinitely duplicated list beside the maps.

The fixed32-binding threshold is a new hypothesis motivated by the measured
eight- and32-binding costs, not a calibrated universal optimum. No threshold
sweep is authorized here. Worst-case scans are bounded by that fixed prefix;
large distinct groups still use logarithmic lookup. One-binding plans retain
their allocation-free binding state. The Strand overlay remains independent.

Use the unchanged legacy comparator, seed42, sizes1/8/32/256/2048, hot/distinct
shapes,131072 offers per arm, and three alternating pairs. H1 retains every
original requirement: each256/2048 distinct ratio at most0.80; median ratios
at1/8 at most1.10 for each shape. Add a32-distinct median guard of1.10. H0 is any
miss. Preserve all raw arms and exact decision/count/digest comparisons. The
hot single-Strand large-group cases remain algorithm stress, not service rate.

Run the existing delivery controls plus deterministic transitions at32/33
bindings, repeated credentials/Spindles across promotion, changing supplied
durable facts and bounded storage for repeated accepts. These compare against
the frozen legacy decision loop; they do not alter the delivery specification.
The independent app and delivery checks run in the final fast profile.

## Execution

Root alone runs this follow-up, serially, under the unchanged resource launcher,
data-drive paths and existing frontier. Maximum300 seconds,2 MiB prospective
reservation after coordinator snapshots, and512 KiB driver-output cap. Exact
post-format source snapshots, commands, exits and hashes accompany the results.
No executable archives, service experiments or remote runs are needed. Successful
owned scratch is removed; failures remain in the existing preservation workflow.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-small-binding-01 --lab coordinator --stage preparation --seconds 300 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 2 -- python3 -B tools/bench/labs/catalog/small_binding.py
```

About2889.63 seconds remain in the shared frontier after the first round.
Its final admission observed1,067,393,024 allocated query-evidence bytes against
1 GiB, before small receipt closeout writes. No budget is reset. Final fast and
manual documentation checks retain the first protocol's separate900-second,
2 MiB-reservation allowance and exact Bun archive-reference cleanup.
