# FabricO11y work recovery

Temporary transfer branch containing a complete Git bundle split into binary parts.
Recovered branch: milestone/streaming-segment-output
Recovered HEAD: a6d49053f1e655c561e9e5e008139e53909e3a1f
Bundle SHA256: 2ef8f43d696cd3f410f1bfe9beca1e932ec89855ace4b3f4c3b8380586757768
Bundle bytes: 226356787

From an existing FabricO11y checkout with a clean working tree:

```bash
git fetch origin milestone/work-recovery
mkdir -p ../FabricO11y-transfer
git archive FETCH_HEAD | tar -x -C ../FabricO11y-transfer
cat ../FabricO11y-transfer/parts/*.part > ../FabricO11y-transfer/FabricO11y-work.bundle
(cd ../FabricO11y-transfer && sha256sum -c SHA256SUMS)
git bundle verify ../FabricO11y-transfer/FabricO11y-work.bundle
git fetch ../FabricO11y-transfer/FabricO11y-work.bundle refs/heads/milestone/streaming-segment-output:refs/heads/milestone/streaming-segment-output
git switch milestone/streaming-segment-output
git push -u origin milestone/streaming-segment-output
```

This branch is a transfer package, not the application's source checkout.
After the recovered source branch has been pushed and verified, this temporary
transfer branch can be deleted.
