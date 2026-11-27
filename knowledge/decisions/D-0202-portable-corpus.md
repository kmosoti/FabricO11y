# Portable corpus views

A versioned binary envelope transfers the existing as-of view membership,
canonical revision and relation bytes, attributed support groups, and explicit
archive/current availability claims. Import requires an expected SHA-256,
checks included record digests and canonical encodings, rejects duplicate or
reordered membership and reconstructs the bounded in-memory projection. The
existing record hash profiles are unchanged.

Availability labels are assertions, not independent proof of stored source
bytes, current access, authorization, custody or retention. A missing selected
record remains missing even when a label claims another availability state;
these describe different observations. The input digest must be authenticated
by the caller. A partial imported view does not establish the source corpus's
admission history. Attestation export, storage integration and retention
qualification remain separate work.
