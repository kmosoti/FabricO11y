# ADR-0005: Use immutable Zstandard segments plus a SQLite catalog

Status: Accepted

## Context

Evidence payloads need compact archival storage, deterministic integrity checks, crash recovery,
and hot metadata lookup without repeatedly decompressing the archive.

## Decision

Seal canonical JSONL into content-addressed, immutable Zstandard segment files with an
uncompressed bounded manifest and fixed integrity trailer. Store segment metadata and maintained
observation heads in a single-writer SQLite catalog. Versioned dictionaries are immutable and
digest-addressed.

## Consequences

Archive bytes are portable and independently verifiable. Catalogs can be rebuilt from segments,
while ordinary lookup avoids archive scans. Sealing must coordinate filesystem durability and an
idempotent catalog transaction.

## Rejected alternatives

- Store unbounded raw payloads in SQLite: rejected because the catalog is a maintained index, not
  the archive.
- Use compressed files without a frozen container: rejected because corruption and recovery would
  depend on implementation accidents.
- Make Zstandard frames the search index: rejected because compression does not provide query
  semantics.

## Supersession

Supersedes no prior decision.
