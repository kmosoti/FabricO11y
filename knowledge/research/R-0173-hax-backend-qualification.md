---
id: R-0173
date: 2026-11-18
added_by: agent
human_ratified: false
---

# Hax 0.4.2 against the pinned frontier proof backend

The pinned direct Charon → Aeneas → Lean pipeline checked the two executable
frontier theorems and the existing axiom inventory. The Verus mutation removing
the completed-publication precondition was rejected.

An isolated run of [cargo-hax 0.4.2](https://github.com/pereq-dev/hax/releases/tag/cargo-hax-v0.4.2)
used the same Rust and Lean sources with overrides for Charon `7b462d9` and
Aeneas `ab34bd2`. The archive SHA-256 was
`64b15097c003919075c7e8cca845db0e29718c0683596b739677b159ceb138f4`.
Charon emitted the admission kernel LLBC. Hax then invoked Aeneas with
`-core-models-lib`. The pinned backend rejected the argument with exit 2
before generating Lean, so neither frontier theorem ran through Hax.

Keep direct extraction as the acceptance path for these pins. A migration
to Hax's separate default backend would need a fresh theorem, axiom, and
mutation comparison. Its setup performance was not measured here.
