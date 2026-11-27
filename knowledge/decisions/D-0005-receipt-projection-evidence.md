# Receipt-specific local projection evidence

Issue #165 requires a batch client to distinguish projection lag from missing
evidence. The store's numeric progress and the Spindle receipt each have meaning
within one origin. Custody and live-ring queries can precede a durable projection.

The collector binds the source origin when its origin-qualified poll is accepted.
The pre-upgrade frontier is the boundary for unverifiable legacy rows. A new
positive check requires a matching custody record, a committed frontier, and a
guarded read of the corresponding raw Parquet row. The provenance column is
internal to the storage format. Storage errors and loss of raw evidence after
retention have the same availability semantics as missing proof of origin.

The reader may return evidence from a row eliminated later by retention. It uses
bounded input, a single command deadline, and a read-only catalog path. Switching
Spindle stores requires a fresh collector data directory.
