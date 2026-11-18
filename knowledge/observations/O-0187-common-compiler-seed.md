---
id: O-0187
date: 2026-11-18
added_by: agent
human_ratified: false
---

# Common release compiler seed

The tool-only common image missed the cold threshold. The [same-VM trial](https://github.com/kmosoti/FabricO11y/actions/runs/44405946015)
measured 18m45.42s for Fedora and 19m02.21s for the image. Release compilation
took 570 to 917 seconds across the three pairs.

A local prototype ran the complete locked release build at
`0d14f2c517ed1a0a49d2445be77cda9c9e3bf796` in the published common Fedora
image `sha256:286ae57fc692879a30ca3d1c3d20da5522689e5107764fb73b3f5f8ec4306b68`.
Both runs used Rust 1.100.0, sccache 0.19.0, no network, the same image paths,
and an empty Cargo target. The empty-cache run recorded registry compiler
outputs. The seeded run rebuilt workspace products from the current source.

| Recorded result | Empty compiler cache | Seeded compiler cache |
| --- | ---: | ---: |
| Release-build wall time | 545.463s | 182.939s |
| Eligible Rust cache hits | 0 | 285 |
| Cache entry writes | 285 | 0 |

The seed held 313,400,097 bytes. The local isolated reflink copy took 0.08s.
All 13 release executable and example hashes matched. The master and copy
contents also matched before and after the seeded run. The empty-cache time
includes recording overhead. The local timings exclude hosted transfer and
image extraction.

A physically read-only mount caused misses even with the correct key.
sccache 0.19.0 updates entry timestamps when it opens a cached object.
A private writable container layer with `SCCACHE_LOCAL_RW_MODE=READ_ONLY`
allowed reads without new cache entries.
[Relevant sccache implementation](https://github.com/mozilla/sccache/blob/v0.19.0/src/lru_disk_cache/mod.rs#L378-L389)

The prototype's registry path filter admitted more packages than the candidate
can safely cache. The locked graph includes procedural macros that read files
or environment values during expansion. The candidate uses a narrower package
set whose resolved dependency closure excludes proc macros and build scripts.
Measure the restricted image separately with its actual transfer cost.
[sccache Rust input limits](https://github.com/mozilla/sccache/blob/v0.19.0/docs/Rust.md)
