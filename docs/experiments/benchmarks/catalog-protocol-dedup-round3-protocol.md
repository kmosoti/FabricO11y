# Exact snapshots for full lifecycle failure preservation

Register before mutation. The [native lifecycle protocol](catalog-native-lifecycle-protocol.md)
reserves full failed state rather than relying on its estimated compression.
Apply all ownership, immutability, terminal/inactive-producer, byte/stat, durable
intent, atomic replacement, negative-control, full-manifest and per-path readback
requirements of the [second reclamation protocol](catalog-protocol-dedup-round2-protocol.md)
to exactly the eight groups below. Existing reclaimed groups are excluded.
Original metadata is preserved in manifests; every path and content byte remains.

Keeper prefix is `data/lab-completion-run-01/coordinator/catalog-algorithm-checks-01/`.

| Keeper | Bytes | Expected paths | Exact SHA-256 |
| --- | ---: | ---: | --- |
| `protocol-39.txt` | 6839 | 197 | `d1b1d8813176fbdd25ac6d1ffa5217bf3913078f2ca34f050581116fed285627` |
| `protocol-17.txt` | 4469 | 184 | `defeced7110a9c9c2bffcadbc4d61ec76343ed9ce7afec1582548fe5cf558b94` |
| `protocol-15.txt` | 6565 | 180 | `df8443270864a02a7944c862fd831ef02120ba3db6418b7af50fb39f0d287e56` |
| `protocol-21.txt` | 4826 | 179 | `91f39e0bd2240fd061ce7c23acb9b1c7916595f1e72e3c63c84c98cf715b09bd` |
| `protocol-34.txt` | 5071 | 179 | `c85a0c94a4d787de2f52bdf77bd92ec26eed58458bf1ecef7d77d0b603d490e5` |
| `protocol-11.txt` | 4784 | 166 | `a6e50e95f6b8e60106d2bbab2499fe77369eabfd9ef928731758825a13994609` |
| `protocol-25.txt` | 5222 | 165 | `03e6dde58081d81d39ae002fd93fdf05483e099d47dbaac5f30a2cabfb154a4a` |
| `protocol-12.txt` | 5920 | 163 | `dbb8c19718f48410e27e7ffbc0c95daa251d3eb43ebc1913dccac490f278c731` |

Read-only forecasts: 1,413 paths, 1,405 redundant distinct inodes, 11,509,760
allocated bytes reclaimable and 922,931 bytes of serialized manifest. Revalidate
the complete group counts before changing any file. Stop on drift.

Job `catalog-protocol-dedup-03` is a small preparation bootstrap with 180 seconds
and a 2 MiB reservation including provenance, manifest and failure margin. Use
the unchanged coordinator lock, frontier/stage accounting, mounted data drive,
20 GiB maximum, 16 GiB high, no swap and 30-minute outer deadline. Archive the
exact new helper and original guard helper plus dependency hashes. Preserve any
partial result honestly; successful owned control scratch is removed. No limit
is reset and no source archive, original protocol or nonmatching file is changed.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/protocol_snapshot_dedup_round3.py --id catalog-protocol-dedup-03 --seconds 180
```
