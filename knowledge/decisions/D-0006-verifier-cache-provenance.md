---
id: D-0006
title: Product Toolchain and Verifier Cache Provenance
type: decision
status: accepted
date: 2026-11-18
added_by: agent
human_ratified: false
related:
  - D-0001
  - D-0003
---

# D-0006 · Product Toolchain and Verifier Cache Provenance

Product CI pins Rust 1.100.0. Verus and Charon keep the compiler versions
required by their pinned upstream releases. Advancing the product compiler
and changing a verifier toolchain are separate compatibility decisions.

An older bootstrap could write the current OPAM lock digest for a cached
switch without importing the lock. It could also record the digest before
Aeneas finished rebuilding. The managed switch now includes the complete
lock digest in its name and checks the installed package set on reuse.
The bootstrap records the Aeneas digest after checking the newly built
executable. Untagged legacy installations require a fresh build once.

Retain the legacy switch until an operator removes it. Future cache and
container layouts should retain the validated managed switch and verifier
runtime needed by the checks.
