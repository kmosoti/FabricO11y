# Held-density RowSet locality control

Prospective standalone data-structure screen. It follows the [matched workload
protocol](cross-system-sweep-protocol.md) and the earlier [pinned RowSet
screen](cross-system-rowset-sweep-proposal.md), while changing only how the
same logical sets map into the finite ID universe. It is a physical-arrangement
control, not a Fabric data model or end-to-end Fabric query comparison. No
outcome is claimed before execution.

## Question and controlled fixture

The prior screen varied density and arrangement together for some shapes, so
those observations cannot attribute representation behavior to either factor.
This follow-up holds each pair's exact cardinalities and overlap fixed across
two bijective layouts. It tests whether representation costs change when the
same memberships occupy contiguous versus shuffled IDs. It does not model
Fabric payload bytes, row ordering, storage, or query planning.

For universe size `N`, density divisor `d` in `{16, 2}`, and `k=N/d`, define
logical sets `A=[0,k)` and `B=[k/2,3k/2)`. Both contain exactly `k` IDs, their
intersection contains exactly `k/2`, and their union contains exactly `3k/2`.
For each seed, map both sets through the same universe bijection:

- `contiguous`: `mapped(i)=(i + seed mod N) mod N`.
- `shuffled`: initialize `mapping=list(range(N))`, then call
  `random.Random(seed).shuffle(mapping)`; `mapped(i)=mapping[i]`.

Sort each mapped input before native construction. Retain complete canonical
input vectors, the seed, and SHA-256 of the complete mapping encoded as
little-endian `u32` values. Before admitting a cell, verify the mapping's
inverse over the entire universe, recover both exact logical inputs through
that inverse, and check actual mapped cardinalities and overlap are exactly
`k`, `k`, `k/2`, and `3k/2`. The independent Python set oracle then checks full
ordered AND and OR outputs and descending top64 bytes against the actual
mapped inputs. The native probe remains unchanged.

Before cell admission, a small mapping checker accepts a known tiny valid
permutation and rejects duplicate, negative, out-of-range, wrong-inverse, and
wrong-logical-recovery controls. The runner retains those control outcomes in
`negative-controls.json`.

## Registered finite grid

Exactly 48 fresh native cells:

`2 universes × 2 densities × 2 layouts × 2 seeds × 3 representations`.

Universes are 32,768 and 262,144 IDs. Densities are 1/16 and 1/2. Layouts are
contiguous and shuffled. New seeds are 2,703,204,355 and 2,703,204,356.
Representations remain sorted Vec, dense bitset, and pinned Roaring from the
prior screen. Rotate representation execution order across density, layout,
and seed. The same seed/density/universe logical pair is used in both layouts;
layout is the sole fixture difference for those paired cells.

Use the prior screen's unchanged native probe, exact Python checker, pinned
upstream source/dependencies, declared-edition checks, license guards, source
archive handling, and cleanup/preservation behavior. Record the new runner and
proposal hashes in the retained execution receipts. Keep construction,
operation, conversion, serialization, requested allocation, incremental live
bytes, and process measurements separate. Include both seeds and all adverse
results. No new performance acceptance threshold is introduced. Two seeds and
reused-input operation batches remain a small screen, not a general performance
or production claim.

## Bounded execution and evidence

The outer coordinator budget is 180 seconds with a 64 MiB evidence reserve; the
runner's inner deadline is 140 seconds. Preserve the 512 MiB owned scratch cap,
20 GiB memory maximum, 16 GiB memory high, no swap, data-drive scratch and
existing cleanup/readback rules. The expected build plus 48 cells is about 20
seconds; the registered deadlines remain the enforceable limits. Run only
through the project resource launcher and serialized coordinator:

```sh
python3 tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id rowset-locality-01 --lab query --seconds 180 --reserve-mib 64 -- python3 -B tools/bench/labs/cross_system/rowset_locality.py --destination docs/experiments/benchmarks/data/cross-system-sweep-01/query/rowset-locality-01 --protocol docs/experiments/benchmarks/cross-system-rowset-locality-proposal.md
```

The run has not occurred. Any result can support only a held-density
representation comparison for these generated ID sets. It cannot nominate a
Fabric representation without an independently registered end-to-end Fabric
query comparison and fresh confirmation.

After a completed sweep, authenticate its 48-cell coverage, fixture mapping
hashes/cardinalities, retained canonical inputs, and exact archived native
outputs with the standalone reducer:

```sh
python3 tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/rowset_locality_report.py --source docs/experiments/benchmarks/data/cross-system-sweep-01/query/rowset-locality-01 --out docs/experiments/benchmarks/data/cross-system-sweep-01/coordinator/rowset-locality-report-01/report.json
```

The reducer reports shuffled-to-contiguous ratios paired by universe, density,
seed, and representation. It imposes no performance threshold and makes no
global winner claim.
