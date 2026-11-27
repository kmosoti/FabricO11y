# Durable scoped decision pages

A provisioned decision writer reads only its own scope. Signed bounded
continuations bind the receiver, writer, scope, frozen frontier, position,
row count, wall-clock expiry and persisted scope revocation epoch. They survive
receiver restart with the independent plan-cursor key. Each call rechecks the
current scope owner. A changed epoch revokes existing cursors and subscriptions;
credential removal can fail authentication before a typed response is possible.
Tokens are authenticated, not confidential. A changed clock affects expiry.

Committed decision history remains the authoritative record. A row-count
mismatch within an issued snapshot reports a gap. This does not identify loss
predating snapshot creation; no history retention or reclaimer is implemented.
Global IDs can interleave scopes and are not treated as contiguous scope IDs.

An owner may hold an ephemeral bounded subscription. A commit sets a coalescing
pending bit, and polling clears it. Subscription registration and commit
notification serialize with the ledger, and the registration frontier lets the
client reconcile the initial snapshot. A slow or restarted client replays pages;
notification delivery alone proves neither custody nor complete observation.
