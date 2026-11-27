# D-0010 · Canonical receipt projection readiness

An explicit canonical target has its own authority and per-origin projection
frontier. A reader supplies the expected authority and exact Spindle receipt.
The check validates the canonical row, then uses the committed projection manifest
and guarded decoding to prove that the full typed row can be read.

Canonical custody can lead the analytical frontier. Keeping both values in the
result distinguishes lag from unreadable evidence. Confirming progress alone
would allow a damaged projection segment to produce a false visible result.
Verifying and reading a single bounded segment with the normal query worker
preserves the server's analytical concurrency bound.

The native engine currently has no separate committed Parquet witness for this
check. Revisit when the native query path can provide equivalent point evidence.

- [Canonical custody and receipt check](../../docs/canonical-custody.md)
- [Local receipt projection decision](D-0005-receipt-projection-evidence.md)
