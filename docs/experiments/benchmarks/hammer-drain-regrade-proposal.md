# Grade late ACK progress independently of an earlier cycle line

The real-edge trial delivered 33,600 exact logs, 600 exact ACKed Batches and four
oracle-exact query chains. Its original run remains failed because the new
research drain checker compared the final cycle's `batch=75` with that same
line's `acked_through=74`. In every node transcript a later delivery event ACKed
75. The existing local observation harness already uses later delivery events;
this new checker had conflated two observation instants.

The minimized trace is: cycle(batch=75, acked_through=74, backlog=0), then
delivery(sequence=75, status=ack). The new research check compares the latest
cycle with the last observed ACKed sequence, requires every node, and still
requires zero source backlog. Reject missing final ACK, outstanding source
backlog and absent-node controls. Full per-Batch source/ACK/replay hashes,
sequence continuity, clean worker exits and independent query grades remain
separate required checks. This does not assert that the ACK log line by itself
proves durable local cursor persistence.

Regrade a copied snapshot in a separate contained job, preserving the original
failed receipt, original checker and full original state. Freeze original input
hashes and emit new grades and controls. Do not rerun the workload or relabel its
original outcome. No production code, product contract or independent oracle
changes. Future remote cases use the corrected observation check.
