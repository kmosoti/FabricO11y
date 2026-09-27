# E2R one-sync group-seal oracle (frozen before candidate)

This is a finite Python standard-library **research model**, not an application
format migration or a device durability test. It instantiates the active E2R
registration in `KILL_TESTS.md` (2026-09 study) and the current FOL2 storage
contract. FOL2 remains the application's two-sync baseline. Sector writes are
atomic at 512 or 4096 bytes; a successful file/directory sync and existing
namespace have the registered meaning. One writer, SHA-256 collision resistance,
and an external record of I/O errors are assumptions.

## Frozen candidate interface

Create `tools/seal-probe/candidate.py` with these pure functions. The fixture definitions were
frozen independently before any candidate was written. The candidate must not import
`oracle_support.py` or `test_oracle.py`; that would collapse the independent
oracle boundary.

```python
encode_groups(groups: tuple[tuple[bytes, ...], ...], sector_size: int) -> bytes
recover(image: bytes, sector_size: int) -> tuple
supervise(external_io_error: bool, recovery_result: tuple) -> str
append_logical(previous: bytes, group_id: int, bodies: tuple[bytes, ...],
               sector_size: int, fail_at: str | None) -> tuple
```

`recover` returns exactly `("ok", committed_groups, prefix_length)`, where
`committed_groups` is a tuple of tuples of original body bytes, or
`("error", original_image)`. It must not modify the input. An `ok` prefix is
seal-aligned, contains every earlier ACKed group, and contains either all or
none of the in-flight group. A complete valid group in a clean image must be
accepted. A complete seal paired with any mismatch must return error and every
input byte. It may return error conservatively on an unsealed final tail.

`supervise` returns `"resume_allowed"` only when the scan is `ok` and the
external error flag is false; otherwise `"rebuild_from_trusted_source"`.
The same valid image must therefore scan identically after successful sync and
after an EIO that left identical visible bytes, but the supervisor decisions
must differ. The file itself cannot encode or infer this external witness.

`append_logical` returns `(image, acked, poisoned, external_io_error,
operations)`. `operations` is `("write:0", ..., "write:N-1", "sync")`, one
write per group sector followed by exactly one sync. `fail_at` is `None` or
one operation name. At a failed write, the just-written sector may be visible;
at a failed sync, all bytes may be visible. Either error poisons the handle,
gives no ACK, and sets the external error witness. The logical model has no
retry or automatic resume after EIO. With no failure, it ACKs only after the
sync. The oracle's concrete failure trace chooses the failed sector visible;
this is one allowed history, not a claim about physical writeback.

Review clarification: `append_logical` rejects an invalid or incompletely committed
previous image and a group id other than the next sequential id with `ValueError`,
before modeling any write or ACK. The caller still supplies a healthy-handle context:
known prior I/O errors require the supervisor's rebuild path even if bytes validate.
The original frozen contract is retained with the E2R experiment evidence; these
input checks were added after an independent reviewer demonstrated an invalid ACK.

## Concrete immutable framing

All integers are unsigned little-endian. Every unit is sector-aligned and
written at its own sector offset. A group is:

1. **Descriptor sector:** bytes 0–3 `FGD1`; 4–11 `group_id` (`u64`, zero based
   and sequential); 12–15 frame count (`u32`, 1..6); 16–39 six `u32` body
   lengths (unused entries zero); all remaining bytes zero.
2. **Frames:** frame `i` starts with `FGF1`, `i` as `u32`, body length as `u32`,
   then exactly that many opaque body bytes, then zero padding through the
   next sector boundary. Bodies are 57..4096 bytes. The descriptor lengths
   determine each frame's sector span. A frame can use more than one sector.
3. **Dedicated seal sector:** bytes 0–3 `FGS1`; 4–11 group id; 12–15 frame
   count; 16–47 SHA-256 of `b"Fabric-E2R-seal-v1\0" + seal_bytes[0:16] +
   descriptor_sector + all_frame_sectors`; remaining bytes zero.

The digest therefore covers descriptor lengths, every header, every body byte,
and **all padding bytes**. The scanner also validates exact field values and
zero padding; no unchecked bytes are treated as committed content. A missing
sector is represented by one all-zero sector. A seal sector containing a
complete `FGS1` seal but mismatching content is an error even if the descriptor
is absent; in particular, the seal-only subset errors. A plausible incomplete
unsealed final group may be discarded. A nonzero expected seal sector with
invalid fields/digest is corruption, not a truncatable absent seal. The model
does not claim cryptographic authentication against a party able to recompute
the hash.

## Frozen workload and verdicts

For each sector size 512/4096, seed 11/12/13, and frame count 1..6, bodies
have lengths `(57, 127, 509, 513, 1023, 4096)[:count]`; each body is filled
by `random.Random(seed).randrange(256)` in frame order. The fixture has one
earlier ACKed group (one 57-byte body, group id 0) followed by the in-flight
group (id 1). Enumerate all persisted sector masks for the in-flight group
when it uses at most 16 sectors. For larger groups use 100,000 masks from
`random.Random(5).getrandbits(N)`, with the empty, full, and seal-only masks
included first. Earlier-group sectors are always present.

For each subset, a full mask must yield both groups. A subset with a present
seal and missing content must fail closed, preserving all bytes. With no seal,
the scanner may return the earlier group and its exact byte-prefix length or
fail closed; it may never expose a partial group or discard the ACKed group.
The all-sector clean control must accept both groups. The oracle also makes
1,000 deterministic covered-byte flips per seed and sector size across a
previously ACKed descriptor, frame region, and seal, requiring fail-closed
recovery without mutation. A complete seal with bad content is always error.

For every group fixture, inject failure at each `write:i` and `sync`
boundary, verifying poisoned/no-ACK/external-error state. Run a no-error
control. The same fully visible bytes appear in a successful-sync trace and
the failed-sync trace; scanner output must be identical, while supervisor
decisions differ. `contract_violations` counts every forbidden outcome; the
threshold is zero. Mutation probes check that an always-error scanner fails
clean controls, truncate-on-mismatch fails post-ACK corruption,
always-truncate fails seal-only, and file-only EIO supervision fails identical
bytes with different external witnesses.

The workload is finite. The support module prints exact mask and operation
counts: **309,456 sector-subset cases**, with 33 exhaustive fixtures and 3
sampled fixtures, **294 injected write/sync boundaries**, and **6,000 covered
byte flips**. Fixture SHA-256 for seed 11, groups of 1 and 6 frames: sector
512 = `2e4ff2bfca3053e9ef8cb9d186ef9589e38a8987b293643a4bfaddcb3452661b`;
sector 4096 = `9ac8dd3a8868cf9d9c0c5c3683b9b93f211a699ac63ddb5746478dbf7b1fa9cb`.
No candidate can affect the chosen cases. Cases with a missing seal
but arbitrary nonzero content are deliberately allowed to fail closed. The
registration does not specify a protocol for arbitrary non-sector-aligned
files, concurrent writers, torn sectors, malicious hash recomputation, or
physical power-loss behavior. Those remain open questions, outside this
oracle. A byte-only scanner also cannot distinguish an entirely erased ACKed
seal from a never-written seal without external trusted ACK metadata; the
one-byte corruption corpus does not create that indistinguishable pair.
The registration also leaves exact framing bytes and body distributions open;
the concrete choices above are frozen experimental fixtures, not a claim that
they are the uniquely correct application format. It does not settle how an
actual caller persists the external EIO witness or obtains the trusted copy
used after that witness is set.
