# D-0355: Local alert journal

Use an explicitly opened, disk-backed SQLite store for the first alert
persistence profile. Provision one authority, scope and exact reducer rule
per store. That rule includes the instance, revision, source and metric
profile used to validate evaluations.

Persist checkpoint, source position, evaluation event and optional
notification intent in one transaction. Bind a caller-stable request ID to
its original expected version and typed evaluation. An exact historical
retry can then recover the same receipt after subsequent evaluations.
Record a notification intent only when the lifecycle changes. Event IDs are
stable within the provisioned store and version history.

Use a finite event budget and a fixed SQLite page ceiling. Full storage
returns an explicit refusal so callers can preserve their source position
for later work. Keep the first profile single-owner, with independently
provisioned scope and no automatic event eviction. Treat source and evidence
references as caller assertions until an adapter performs readback.

Read history in bounded local pages. Keep the first read version fixed through
appends and reopening, and check persisted evaluation and receipt consistency
before returning entries. The local position belongs with the original journal
and does not carry authentication.
