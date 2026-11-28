# D-0357 · One-shot local intent reservation

Date: 2026-11-28. Status: proposed.

A single explicitly opened alert journal owns its local work state. New
lifecycle transitions commit their event receipt, checkpoint and queued work
row together. The versioned additive migration leaves historical receipt-only
intents `LegacyUnmanaged`; it does not create work or replay them.

A journal may hold one local one-shot reservation at a time. A bounded lease
uses an opaque token tied to the current open session and a durable fence. Its
deadline uses a process-local monotonic `Instant` and is not stored as a
restart-safe clock. State inspection reports the persisted state. Claim and
abandon operations observe expiry; expiry or abandonment makes the work
uncertain. Reopening the journal marks an interrupted lease uncertain. There
is no automatic retry, reclaim or delivery confirmation in this profile.

Authority and scope supplied to this local API are provision checks, not
identity authentication. An intent, lease or uncertain state does not establish
custody, evidence availability or a notification outcome. Remote access,
recipient authorization, delivery and reconciliation require separate
contracts.
