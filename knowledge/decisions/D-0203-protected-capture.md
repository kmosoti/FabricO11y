# Protected application capture

A protected batch outbox loads an owner-provisioned policy before replay and
binds its digest and expected origin in a distinct schema version. Ordinary
opens cannot replay the protected store. A changed policy or origin holds the
store without submitting pending data. The policy is a process-lifetime
configuration snapshot, not an authenticated principal or live revocation.

Version 1 protected application capture permits only configured top-level
scalars. Version 2 adds bounded literal object-key paths, fixed redaction and
explicit homogeneous scalar arrays; no path grants a subtree or wildcard.
Both construct the source digest from canonical permitted bytes. A distinct v2
carrier binds the sanitized application result, policy identity, digest and
fixed omission classes to exact replay and readback. Ordinary v1 data and
other ingestion routes retain their existing contracts; no broader enforcement
is implied.

Version 2 retains the existing input cap and uses a separate bounded work
reservation for parse, path lookup and output construction. An array that is
too long or has an invalid element is omitted as a whole. Redaction emits only
a fixed marker and fixed omission class; a selected redacted key remains
visible. The protected decoder accepts both policy versions while rejecting
version-incompatible omission classes. Neither policy protects other ingresses.
