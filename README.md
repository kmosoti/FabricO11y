# FabricO11y

FabricO11y is a planned Fedora-first, Rust-native evidence platform for Rust
applications and agent harnesses. The repository contains development tooling and
an early local collection and analysis prototype in the `fabric` crate. Spindle,
the agent API, and the remaining analytical capabilities are planned.

Read [product direction](docs/direction.md), [contributing](CONTRIBUTING.md) and
[verification](CONTRIBUTING.md#verification).

The prototype's concurrent queue reserves fixed payload buffers before encoding
and retains capacity through consumer leases. See [admission](docs/admission.md)
for memory accounting, contention and shutdown behavior. Its file
writer requires a single writer, does not sync the parent directory, and can leave
an incomplete tail after an error. It is not a production custody service. See
the [prototype specification](docs/local-prototype.md) for its intended scope.

```sh
cargo xtask doctor
cargo xtask check --tier fast
cargo xtask check --tier medium
cargo xtask check --tier extensive
```

Requires Jujutsu 0.46.0 with a Git backend and dated Rust beta-2026-10-18 (1.100.0-beta.7)
(compiler commit fd8e211ae301c84a86d13cf52a6d3ce08c589e2e).
