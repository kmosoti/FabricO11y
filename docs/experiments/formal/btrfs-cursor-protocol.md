# Btrfs cursor identity regression protocol

Registered 2026-10-10 before implementation and corrected-candidate measurement.
Scope: [ADR-0028](../../decisions/ADR-0028-preserve-btrfs-log-identity-across-reboots.md),
the observed Fedora upgrade/reboot duplicate-source defect. Existing installation
expectations, delivery/query oracles and resource ceilings remain unchanged.

Preserve the failed lifecycle continuation's exact rows, source offsets, inode,
device numbers, package hashes and boot IDs. Register a small deterministic
fixture with original device 51, reboot device 32, inode 2505 and three consumed
newline records. The old raw-device-only comparison must reread all three;
the candidate with equal full filesystem/subvolume identity must read none,
then read exactly one independently appended fourth line.

Negative controls change each of UUID, subvolume, inode, consumed prefix and
file length separately; no changed identity may inherit the consumed offset.
An unavailable stable-identity lookup must preserve the previous cursor through
the existing error path. Legacy cursors still require the old complete identity
and prefix check before acquiring a stable identity. Test both matching raw
device migration and already-changed legacy device ambiguity. Keep malformed
identity rejection and old absent-field bytes, and replay the committed new
cursor from a fresh Spool reader.

All tests/validators use the resource launcher and mounted data-drive scratch.
Record command/exit, source hashes, effective limits and cleanup. Do not modify
accepted Batch bytes during replay or retry. A passing corrected candidate must
also complete fresh exact-package Fedora installation, predecessor upgrade and
actual reboot with unchanged accounts/configuration/credentials/history and
exactly three retained fixture rows. The previously failed overlay continuations
remain failed and are not reinterpreted as candidate passes.
