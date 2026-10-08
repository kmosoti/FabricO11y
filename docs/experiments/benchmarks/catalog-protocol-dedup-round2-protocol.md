# Exact protocol snapshot reclamation, second registered group

Register before any mutation. Extend the earlier
[path-preserving reclamation](catalog-protocol-dedup-protocol.md) to exactly eight
other immutable protocol-copy groups. Read-only inventory found 1,514 paths and
1,506 redundant distinct inodes. Expected gross allocated reclaim is 15,265,792
bytes; the current intent/completion representation requires 988,946 manifest
bytes. These are forecasts, not executed reclamation results.

All keeper paths below are relative to
`data/lab-completion-run-01/coordinator/catalog-algorithm-checks-01/`.

| Keeper | Bytes | Expected paths | Exact SHA-256 |
| --- | ---: | ---: | --- |
| `protocol-40.txt` | 10279 | 198 | `49fef0c028c709e0929df6c707c3b5b8458df086bb631973f2afc303d2c99f05` |
| `protocol-32.txt` | 8402 | 194 | `1aa7aed1532be128a940ded44d1825fe7628ae5d93473e95fe0411bf1c08395c` |
| `protocol-14.txt` | 9090 | 167 | `edbc48568c5f36263cad448028cab9c81930716179e7143c99786830680c3570` |
| `protocol-28.txt` | 9174 | 160 | `f8d15dd6ff359c009cc40a16b08e770e98b99f34c573f8aaf1af3532555b9d32` |
| `protocol-4.txt` | 6246 | 199 | `391f74ee36aa816127db3eab090de84119ca7fcae98ed5faa0767118aea55d65` |
| `protocol-6.txt` | 5559 | 199 | `cf1da03fc1edc4dfcafe53456c2b540af6792f64e2c47e5deae7bdb841a968cd` |
| `protocol-7.txt` | 5473 | 199 | `edce1b1b09774e282fcc1f59ba6aed4748d2b94cdf82058ba45aa877ded01038` |
| `protocol-18.txt` | 6429 | 198 | `30405ae8b000575a2cf7884b08efcec7dce3d99a7997a4fdff90c0ca3b476bdc` |

Only terminal `catalog-*` coordinator jobs' `protocol-*.txt` snapshots matching
these complete groups may change physical inode. Require exact full bytes as
well as size/hash, terminal producer identity, inactive owned units, regular
non-symlink paths, same filesystem and unchanged stat identities. Validate every
group and its frozen count before replacement. Existing hardlinked snapshots,
original protocols, source archives, working-tree diffs and all other files are
outside scope. Keep every path/content byte; preserve original metadata in the
durable manifest before atomic hardlink replacement and parent-directory sync.
Metadata/inode identity changes are explicit. Treat all snapshots as immutable.

Reject changed-byte and wrong-target controls. Independently read back the whole
manifest and every logical path, including keepers; require complete intent/
completion coverage and no orphaned temporary links. Preserve partial-failure
receipts and bytes without declaring full reclamation. Reuse the original
helper's narrow ownership and comparison routines; archive both exact helpers.

The bootstrap job `catalog-protocol-dedup-02` has a 180-second preparation limit,
1,280 KiB prospective reservation including provenance, manifest, receipts and a
32 KiB failure margin. The current approximately 1.5 MiB query-evidence headroom
must be checked again before writing. Charge elapsed work to the unchanged
14,400-second frontier and stage budgets; do not create a full source archive.
All work runs under the existing data-drive/20 GiB max/16 GiB high/no-swap launcher
and coordinator lock. Successful owned control scratch is removed.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/protocol_snapshot_dedup_round2.py --id catalog-protocol-dedup-02 --seconds 180
```
