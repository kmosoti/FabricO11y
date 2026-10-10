# Local dependency patches

These five crates retain their upstream names and versions. Their source, tests,
license files and original manifests come from the crates.io archives identified
in [provenance.json](provenance.json). Archive SHA-256 values were compared with
the existing Cargo.lock checksums before extraction.

The sole upstream-file modification in each crate is its normalized Cargo.toml:
the dependency named `paste` now aliases the real registry package `pastey`,
pinned to `=0.2.3`. Cargo.toml.orig remains the upstream reference. Each added
FABRIC-PATCH.md marks the local change. No upstream Rust source is modified.

`paste` is unmaintained ([RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)).
[Pastey's upstream migration instructions](https://github.com/as1100k/pastey)
recommend this dependency alias to retain existing macro imports. Cargo.lock
records `pastey` under its actual name; this patch does not rename a replacement
package to `paste` or add an advisory exception. Compatibility still requires
the native and WASM application checks; upstream migration guidance alone is
not evidence that every application behavior is preserved.

The patch set covers either_of 0.1.9, leptos 0.8.22, reactive_graph 0.2.15,
reactive_stores 0.4.4 and tachys 0.2.19, the five paste consumers in the candidate
lockfile. Replacement pastey 0.2.3 is downloaded through Cargo from crates.io;
its archive SHA-256 observed during review was
`2ee67f1008b1ba2321834326597b8e186293b049a023cdef258527550b9935b4`.
Its declared license is MIT OR Apache-2.0 and it has no normal dependencies.

Remove a local patch when a selected upstream release makes this migration.
For a replacement upstream version, review its delta, refresh provenance and
rerun dependency policy plus application checks. Do not silently change vendored
files while retaining the recorded upstream identity.
