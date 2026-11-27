---
id: D-0399
date: 2026-11-27
added_by: agent
human_ratified: false
---

# Fixed-barrier exact receipt batches

A reader-authorized origin and authority, explicit caller source barrier and
bounded ordered receipt list define each batch. Custody and projection
frontiers are captured independently. While the one projection mutex is held,
selected immutable segments are validated and scanned once each, and every
ready item must match its full typed projection row and canonical identity.
This does not establish a retained cross-request snapshot. Per-request byte,
row, count and time budgets bound the work; unavailable work stays explicit.

The original one-receipt operation remains unchanged. The optional digest is
for exact canonical observation bytes; application source and payload digest
roles require separate binding. Native engine custody lacks the independent
Parquet witness and is unavailable in this profile.
