# Preserve identical protocol snapshots with shared storage

Registered before mutation. Read-only inspection after the
[algorithm round](catalog-algorithm-round-findings.md) found 195 distinct inodes
containing the same 29,418-byte historical protocol snapshot. Each occupies
32,768 allocated bytes. The conservative query-evidence cap has approximately
2.6 MiB remaining; normal coordinator snapshots would consume much of that.

The only allowed group consists of terminal `catalog-*` coordinator jobs'
`protocol-*.txt` files with SHA-256
`49736fe0c829d9d86751b913b62892612b1b7a605f045572dac1d2c5be783fd9`,
and byte-for-byte equality with
`data/lab-completion-run-01/coordinator/catalog-plan-docs-01/protocol-0.txt`.
At least two distinct inodes are required. Other hashes, source archives,
working-tree diffs, protocol originals and failure results are outside scope.

Preserve every logical path and every content byte. Replace redundant physical
copies with hardlinks to the verified keeper on the same filesystem. Record
original mode, owner, timestamps, size, allocated blocks and inode in a durable
manifest before replacement. Metadata and inode identity change; their original
values are retained in that manifest, not claimed to survive on each hardlink.
Historical snapshots are immutable by convention and the coordinator lock
establishes a single-writer assumption. Do not edit any linked snapshot later.

Check terminal job identity and inactive owned units, reject symlinks and paths
outside the exact owned tree, compare complete bytes independently of hashes,
recheck source identity before atomic replacement, sync parent directories, and
read back bytes and inode identity afterward. Persist intent and completion
events. A partial failure keeps every logical file readable with its original
bytes and retains the unfinished receipt; it does not claim full reclamation.
Synthetic changed-content and incorrect-target controls must be rejected before
real transformations. Success requires complete manifest readback and no orphaned
temporary links. Expected allocated reclaim is 6,356,992 bytes before overhead.

Use a small bootstrap following the already recorded reclamation pattern:
exact helper/protocol copies, dependency hashes, revision, commands, cgroup and
resource observations. No new full source archive is necessary for this exact
file transformation. Reserve 256 KiB before creating evidence. Maximum 120 seconds
inside the unchanged 20 GiB max/16 GiB high/no-swap launcher, on the mounted data
drive. Charge elapsed time to the existing preparation, campaign and 14,400-second
frontier budgets; approximately 2,548.34 frontier seconds remain. Successful
owned control scratch is removed; failures retain their bounded evidence.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/protocol_snapshot_dedup.py
```

The registered ID is `catalog-protocol-dedup-01`. This permits exact storage
deduplication only; evidence limits and verification policies are unchanged.
