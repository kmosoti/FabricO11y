# Hosted CI tool image qualification

The trials ran the product tree at
`0d14f2c517ed1a0a49d2445be77cda9c9e3bf796` with Rust 1.100.0.
The [trusted publication](https://github.com/kmosoti/FabricO11y/actions/runs/44392799921)
produced two Linux x86-64 images. Both exact images downloaded anonymously
with digest verification.

| Profile | SHA-256 digest | Compressed layer bytes |
| --- | --- | ---: |
| Common | `286ae57fc692879a30ca3d1c3d20da5522689e5107764fb73b3f5f8ec4306b68` | 448,081,591 |
| Native | `170d813a79e72c8a7f0c74bcc8e969b3efa14983c75498dde5f3bbaa5cbb89f0` | 2,948,516,099 |

## Initial job-container comparison

All twelve workload jobs in the [common trial](https://github.com/kmosoti/FabricO11y/actions/runs/44396306741)
passed and warm jobs restored their exact cache keys. GitHub assigned the
lanes across different CPU and runner image configurations. The cohort
supplied only one matched cold pair and one matched warm pair. The timing
gate required three pairs per cache state.

| Matched pair | Fedora baseline | Candidate |
| --- | ---: | ---: |
| Cold | 18m50s | 19m06s |
| Warm | 1m48s | 4m49s |

The warm baseline used the checkout action's REST source archive after
detecting that the base image lacked Git. The candidate used Git and
logged 30 workspace compilations despite an exact cache hit.

With Cargo 1.100.0, moving an unchanged source file's modification time
forward triggered recompilation in a single-binary fixture. Extracting the
exact Git archive restored the original file time and Cargo reused the
binary. Another fixture changed the source from A to an equal-length B
while preserving its earlier modification time. Restoring A's target
directory allowed Cargo to run A again.

Both debug and release forms of a deleted auto-discovered binary and example
survived separate Cargo workspace cleans. The next release build still
succeeded, and the old executables remained runnable. The collection smoke
runs the built `agent_harness` example directly, so a stale example could report
a false pass. Compiled archives require an exact source match. Registry and
Git dependency downloads can use a separate cache across source revisions.

All six cold workloads and one complete warm workload in each lane passed
in the [initial native trial](https://github.com/kmosoti/FabricO11y/actions/runs/44398403873).
Its only matched cold stratum contained one Fedora sample at 38m15s and
two candidate samples at 25m45s and 26m40s. The matched warm Fedora and
candidate jobs took 13m25s and 13m19s. Fedora's native proof cache held
5,014,784,568 compressed bytes. The cold cohort could no longer meet the
three-pair criterion, so the remaining warm repeats were cancelled after
both first warm workloads completed.

## Hosted isolation and restore smoke

The [paired native smoke](https://github.com/kmosoti/FabricO11y/actions/runs/44405033343)
used independent empty Docker and containerd stores for both lanes on one
hosted VM per pair. Both warmed lanes restored their exact application
keys, and warm Fedora also restored its exact native proof key. Root
ownership, permissions, executable bytes, symlinks, and hardlinks survived
the round trip. The smoke exercised DNS and TCP access from both
containers. It used small fixtures and supplies no workload timing evidence.

## Same-VM common cold pairs

The [paired common run](https://github.com/kmosoti/FabricO11y/actions/runs/44405946015)
completed all product workloads in the three cold pairs. The timings cover
anonymous image pull through the final release smoke.

| Pair | Fedora (ms) | Candidate (ms) | Candidate minus Fedora (ms) |
| --- | ---: | ---: | ---: |
| 1 | 1,559,990 | 1,544,340 | -15,650 |
| 2 | 1,125,420 | 1,142,210 | +16,790 |
| 3 | 993,740 | 985,050 | -8,690 |
| Median | 1,125,420 | 1,142,210 | -8,690 |

The first two median columns compare each variant's three times. The last
column takes the median of the three paired differences. The candidate
missed the threshold on the median times. The first warm candidate restored
the exact 1,535,101,138-byte application archive. The warm witness expected
the earlier Cargo fingerprint directory and stopped before running a
workload. Cargo 1.100 puts fingerprints under the package's build directory.
The run was cancelled after that failure. It has no qualified warm timing.

See [the qualification procedure](../../docs/ci-images.md#compare-on-matched-hosted-vms)
for the full workload and timing criteria.
## Alpha.2 matched comparisons

The native and seeded common candidates published from `b8f1cddd9831` each
passed three cold and three warm matched pairs. The measured workload includes
anonymous OCI transfer, startup, source preparation, applicable cache restores,
and the checks. It excludes daemon provisioning and cache saves. Times are in
seconds.

| Profile | Cold baseline median | Cold candidate median | Warm baseline maximum | Warm candidate median |
| --- | ---: | ---: | ---: | ---: |
| Native acceptance | 2496.13 | 1656.11 | 795.93 | 719.09 |
| Seeded verify | 1234.39 | 1210.79 | 129.15 | 108.80 |

The common cold improvement was 23.60 seconds in this cohort. The warm
results apply to exact-source cache restores. Ordinary PRs with new source
commits can reuse dependency downloads and use the compiler seed when its
inputs match.

The [native trial](https://github.com/kmosoti/FabricO11y/actions/runs/44465624718)
and [seeded trial](https://github.com/kmosoti/FabricO11y/actions/runs/44484088120)
retain the environment, phase times, and exact cache receipts.
