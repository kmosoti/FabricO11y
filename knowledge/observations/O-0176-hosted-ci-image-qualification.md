---
id: O-0176
date: 2026-11-18
added_by: agent
human_ratified: false
---

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
directory allowed Cargo to run A again. The target must be bound to the
tested source or cleaned of workspace output before use across sources.

## Hosted isolation and restore smoke

The [paired native smoke](https://github.com/kmosoti/FabricO11y/actions/runs/44405033343)
used independent empty Docker and containerd stores for both lanes on one
hosted VM per pair. Both warmed lanes restored their exact application
keys, and warm Fedora also restored its exact native proof key. Root
ownership, permissions, executable bytes, symlinks, and hardlinks survived
the round trip. The smoke exercised DNS and TCP access from both
containers. It used small fixtures and supplies no workload timing evidence.

See [the qualification procedure](../../docs/ci-images.md#compare-on-matched-hosted-vms)
for the full workload and timing criteria.
