# Scan discovery-gap reproduction

Status: additive pre-execution registration; **no Scan outcome claimed**.
Extends [Walk discovery registration](catalog-discovery-race-protocol.md) with
the same existing-storage schedule in Scan. Its preserved Walk failure and
candidate repair results remain separate. No oracle/contract changes.

H1: Scan acquisition returns exact pre-IO custody OR explicit Interrupted with
exact fresh acquisition. H0: it silently returns incomplete selected coverage.
Expected behavior is H1. Register before first run; preserve any failed result
before changing production discovery logic. Do not weaken its expectation.

Fresh owned filesystem state: seed `0xCA7A10A1`, eight groups, three nodes, 24
Batches, 144 explicitly constructed log rows and independent exact ledger written
before IO. Append/rotate one actual pending journal; active journal stays empty.
Both-plan baseline query oracle controls and no-cut Scan selected custody must
match the ledger before entering the callback.

Private cfg(test) `History::sources_at_discovery_cut` calls the existing Scan
acquisition, with a callback after its Segment enumeration and before journal
file enumeration. The callback uses actual bounded builder and actual Store/
sealer checkpoint/reclaim, retaining unlimited history. Segment 1 must exist and
its sealed journal must be absent. Continue the SAME Scan acquisition; enumerate
selected raw Batch custody through Segment tables and returned journal Groups.
Silent Ok with missing identities fails, without requery normalization. Explicit
Interrupted records movement and requires exact fresh `sources` acquisition;
other errors fail. Keep operation/cut summary, selected custody and pre-IO ledgers.

Hook remains private; normal production acquisition supplies a no-op callback.
No synchronization, durable lease, independent oracle change or Scan repair is
part of this first reproduction. Query PI owns hook/source; operations PI owns
`query_discovery_race_test.rs`. This tests actual acquisition, not a model.

Exact proposed selector (final source must match before root admission):
`query::discovery_race_tests::publication_and_reclaim_between_scan_discovery_steps_preserve_coverage`.
Run `cargo test --offline --locked -p fabric-server --lib SELECTOR -- --exact
--test-threads=1 --nocapture` through root's serial resource coordinator/launcher.
Fresh ID `catalog-scan-discovery-race-01`; set FABRIC_STORAGE_EVIDENCE to its
separate recovery evidence directory. All IO uses mounted FABRIC_SCRATCH_ROOT.

Budget: 300-second cell, existing 16/20 GiB high/max, zero swap, 64 MiB owned
scratch, 16 GiB free reserve, 8 MiB compact evidence. Stop for unexpected baseline
failure, missing callback receipt, IO error, assertion mismatch, OOM, deadline,
storage ceiling or survivor; preserve failure before cleanup. Record command/exit,
source/protocol hashes, kernel peak/events and scratch/evidence size. No timing or
capacity comparison is authorized. One selected acquisition schedule cannot
establish general concurrency, mid-read retention safety or qualification.
