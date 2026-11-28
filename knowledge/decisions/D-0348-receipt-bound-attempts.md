# Receipt-bound reported attempts

Decode only the named prior-failure attempt schema from an authenticated
application readback. The caller supplies the expected owner scope, report
reference, original-artifact digest claim, and all five bounded identity and
diagnostic fields. The configured Reader and local owner outbox establish the
receipt-bound application identity and canonical payload digest under one
bounded deadline. A mismatch or malformed recognized payload fails closed.

The original-artifact digest is a bound claim, not an independent verification
of unavailable original bytes. The returned observation is reported data; it
establishes neither verifier execution, causality, nor an enforcement verdict.
This boundary does not bind a corpus claim to a report or provide a new
collector, authority, or verifier.
