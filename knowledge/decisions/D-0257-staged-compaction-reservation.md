# D-0257 · Bounded reservation for staged Parquet compaction

Date 2026-11-27. Status proposed. `fabric-store` compaction stages its
replacement before changing partition inputs. The compaction replacement
output limit is 64 MiB and covers the serialized Parquet file, including its
footer. This change does not alter non-compaction staging calls that do not
have an output cap.

For each compaction output, the store calls Linux `fallocate` on the newly
created staging inode for the output limit before beginning Parquet writes.
Reserving the inode that will be written and promoted avoids treating a
free-space observation or a separate reservation file as a reservation for the
actual replacement. Unsupported allocation and allocation errors produce a
typed refusal, abort the staging attempt, and clean up the staging file before
changing its partition inputs. If the output exceeds its byte cap, the
oversized staging file is discarded; compaction may retry with a smaller input
group.

`RetentionJanitor` handles this typed allocation refusal specially: it surfaces
an error and defers quota eviction for the entire pass. Inputs to the refused
attempt remain unchanged, while any atomic replacements successfully completed
earlier in the same pass remain in place. Unrelated maintenance errors keep the
existing quota-eviction behavior. A later pass can still evict older files
under the configured retention policy.

After writing, the store truncates the inode to the emitted length, calls
`sync_all`, and verifies the device, inode and final length. It then validates
the staged Parquet footer and continues through the existing durable
replace-intent, atomic promotion and recovery path. This change does not add a
storage component or replace the existing promotion protocol.

The cap bounds one replacement output, not source inputs, total old-plus-new
space, all compaction work, total partition or retention usage, or process-wide
disk consumption. `fallocate` availability and guarantees vary by filesystem
and storage stack; copy-on-write, thin-provisioned and networked storage may
not provide the same allocation behavior, and later writes can still fail.
Behavior under actual disk exhaustion is not established. Archive audit,
archival integrity and restore qualification also remain outside this
contract.

Retention and eviction policy remain separate. The bounded output reservation
neither verifies archived evidence nor decides which retained data may be
removed.
