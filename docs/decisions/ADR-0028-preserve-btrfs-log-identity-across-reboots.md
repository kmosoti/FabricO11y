# ADR-0028: Preserve Btrfs log identity across reboots

Status: Implemented in `889dfac`; native regression checks passed, fresh packaged
Fedora upgrade/reboot verification pending. Date: 2026-10-10.

## Context

The Fedora 44 lifecycle continuation retained three original log rows and then
read the same three lines again after reboot. The file path, inode, content and
offsets were unchanged; Btrfs's runtime device number changed from 51 to 32.
The reader treated that change as rotation. The minimized observation is
registered in the [regression protocol](../experiments/formal/btrfs-cursor-protocol.md).
Ignoring device identity would instead allow an unrelated filesystem with a
matching inode and prefix to inherit a consumed offset.

## Decision

Add optional message field 8, `btrfs_identity`, to the version-one `Cursor`.
Its fields are the full 16-byte filesystem UUID (bytes, tag 1) and containing
subvolume ID (uint64, tag 2). Reject malformed UUID length, all-zero UUID, or
zero subvolume ID. Existing fields, Batch version, frame format, sync ordering,
ACK meaning and stored retry bytes remain unchanged. Absence encodes exactly
as before. Old decoders ignore the additive field.

The Linux adapter obtains both identifiers from the same open regular-file
descriptor using the unrestricted filesystem-info and containing-subvolume
ioctl operations. No new capability or core dependency is introduced. The
[Btrfs ioctl documentation](https://btrfs.readthedocs.io/en/latest/btrfs-ioctl.html)
specifies the unrestricted containing-subvolume lookup. Linux's
[Btrfs implementation](https://github.com/torvalds/linux/blob/v6.18/fs/btrfs/ioctl.c)
returns the filesystem UUID without a privileged filesystem search.

When both stable identities exist, equality of UUID, subvolume and inode replaces
the runtime-device equality check. Length, consumed-prefix and oversized-line
guards still apply. A different UUID or subvolume is a different file even if
runtime device, inode and prefix match. A previously stable identity must not
silently fall back to runtime device identity when its lookup fails. Errors keep
the prior committed cursor and use the existing visible collection-gap path.
Other filesystems retain their current device/inode/prefix behavior.
A successful filesystem-type lookup proving that a replacement is not Btrfs
is a different filesystem, not a failed identity lookup: restart with the visible
rotation notice. Only lookup failure preserves the old cursor without reading.

An old cursor without this field first uses the complete existing raw-device,
inode, length and prefix checks. A subsequent ordinary committed cursor records
the stable identity. Upgrade before reboot therefore preserves the offset while
acquiring the new identity. An already-renumbered legacy cursor cannot establish
past filesystem identity: preserve the existing visible rotation/replay behavior,
never guess from path or prefix. Downgrading to a reader that discards the new
field loses this improvement on subsequent cursor writes.

## Alternatives and limits

Reject ignoring device changes, inferring identity from path/content, and replacing
the device number with a folded hash. Linux's
[Btrfs statfs code](https://github.com/torvalds/linux/blob/v6.18/fs/btrfs/super.c)
folds the UUID and subvolume into 64 bits; storing full values avoids that extra
collision class. UUID uniqueness and ordinary inode/prefix semantics remain
assumptions; cloned filesystem UUIDs and arbitrary host-admin state replacement
are not independently authenticated. This does not promise portable stable
identity on every filesystem or recovery of pre-upgrade missing identity.

## Verification

Keep independent same-device/different-filesystem and same-filesystem/different-
subvolume controls, the actual 51-to-32 reboot counterexample, unchanged-prefix
continuation, prefix replacement, truncation, legacy migration and failed lookup.
Verify additive encoding, validation, exact Spool replay and original core checks.
Then rerun the exact package's Fedora upgrade and actual reboot under SELinux
enforcing, requiring exactly the original three rows. A unit simulation alone
does not close the installation gate.
