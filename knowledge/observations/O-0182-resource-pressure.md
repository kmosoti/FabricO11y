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

This evidence covers collector working-set decisions and Spindle admission.
It does not measure sustained load, loss rates, or application performance.
