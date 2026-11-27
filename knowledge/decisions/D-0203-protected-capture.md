# Protected application capture

A protected batch outbox loads an owner-provisioned policy before replay and
binds its digest and expected origin in a distinct schema version. Ordinary
opens cannot replay the protected store. A changed policy or origin holds the
store without submitting pending data. The policy is a process-lifetime
configuration snapshot, not an authenticated principal or live revocation.

Protected application capture permits only configured top-level scalars and
constructs its source digest from canonical permitted bytes. A distinct v2
carrier binds the sanitized application result, policy identity, digest and
fixed omission classes to exact replay and readback. Ordinary v1 data and
other ingestion routes retain their existing contracts; no broader enforcement
is implied.
