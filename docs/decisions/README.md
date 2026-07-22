# Architecture Decision Index

Architecture decision records use `ADR-NNNN-short-title.md`. Each record must state its status,
context, decision, consequences, rejected alternatives, and supersession relationship.

The entries below are the planned initial ADR set. `Planned` means the topic is recognized but no
decision record has been accepted yet.

| ADR | Title | Status | Owning work package |
|---|---|---|---|
| ADR-0001 | Adopt evidence-aware observability scope | Planned | FO00 |
| ADR-0002 | Separate observation, correction, frontier, and answer envelopes | Accepted | FO01 |
| ADR-0003 | Put domain semantics in a pure Rust core | Accepted | FO02 |
| ADR-0004 | Make the embedded SDK primary and the daemon optional | Accepted | FO05 |
| ADR-0005 | Use immutable Zstd-compressed segments plus a SQLite catalog | Accepted | FO03 |
| ADR-0006 | Treat maintained indexes as the primary search path | Planned | FO08 |
| ADR-0007 | Model multiple time axes and explicit causal edges | Accepted | FO01 |
| ADR-0008 | Include completeness metadata in every derived answer | Planned | FO07 |
| ADR-0009 | Keep vector search and active probes out of the 0.1 core | Planned | FO00 |
| ADR-0010 | Preserve repository-agnostic Blackcell and PraxisLedger integration | Planned | FO00 |

An ADR becomes `Accepted` only when its record is added and its downstream contracts agree. Code or
schemas must not rely on a planned decision as though it were already frozen.
