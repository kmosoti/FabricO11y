---
id: O-0182
date: 2026-11-18
added_by: agent
human_ratified: false
---

# Effective resource settings under Linux pressure

The resource-policy candidate was `59505c733adb8b5a66b0d370baa4407373229a9e`.
The checks used transient user systemd units inside `fabrico11y.slice`.

- A unit with `MemoryHigh=96M` reported a 100,663,296-byte cgroup input and
  automatically chose a 9,437,184-byte hot ring. The settings reported the
  detected input, calculated budget, and source of the effective value.
- In another unit with the same memory threshold, `fabric doctor` reported
  idle, active shedding after touching 78 MiB of anonymous pages, and idle
  again after release. Before adjusting for inactive file pages, the parent
  slice had 228,806,656 bytes charged, including 181,309,440 inactive file
  bytes, while the kernel reported zero recent memory stall averages.
- With `RLIMIT_NOFILE=16` and 24 simultaneous connection attempts, the release
  Spindle reported a four-connection effective cap. It held 11 descriptors
  at the sampled point, returned a durable receipt, and accepted a subsequent
  status request. The test used the versioned TOML loader.
- The ring test confirms that dropping optional compressed history preserves
  the exact unflushed sample. The extracted Lean contracts check budget
  rounding and the preservation of live resource charges. The model rejects
  mutations for returning live charges and omitting budget rounding.

The release deployment fixture also passed in 81.29 seconds at this
candidate using the standard slice. It confirmed exact local and canonical
receipt visibility after a receiver restart, and compared the two services'
MELT records through a fixed canonical prefix of 57.

## Fabric Server admission and restart

Candidate `be949cc08b67636ab394871a043c7bafe5d636e9` passed the
receiver pressure fixture in `fabrico11y.slice` with Rust 1.100.0. The two
real-process descriptor tests took 4.10 seconds. One exhausted the receiver's
native descriptors and checked progress and byte-exact recovery. The other
used `RLIMIT_NOFILE=48`, held an incomplete authenticated ingest frame while
measuring descriptor pressure, and confirmed its commit after finishing the
frame. During that trial the receiver remained available for authenticated
readback, blocked fresh analytical planning, and held the projection at its
previous checkpoint. The fixture killed the receiver and reopened the original
SQLite and Parquet directories. It found the same authority and exactly one
copy of the original byte string, then proved the projected row visible.

The same fixture under cgroup memory pressure took 5.32 seconds with
`MemoryHigh=112M`, `MemorySwapMax=0`, and 86 MiB of touched anonymous pages.
The transient unit reported a 106.9 MiB aggregate cgroup peak and zero swap.
Memory pressure was active before submitting the authenticated frame. The
receiver committed and returned the original bytes while new analytics and
projection replay were deferred, then caught up after kill and restart.

A response-write test checked that cancellation does not release an active
query slot before the response write completes, and that new work can acquire
it again after pressure falls. Verus verified the Rust admission predicate.
Charon and Aeneas extracted the code and Lean proved one-unit admission at the
exact boundary and audited the elastic headroom and return contracts.

These trials cover sampled working-set and descriptor pressure, exact custody
and recovery, and analytical response ownership. The cgroup run fixes swap
at zero and uses the SQLite/Parquet backend.
