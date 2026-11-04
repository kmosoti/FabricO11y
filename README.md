# FabricO11y

FabricO11y is a planned Fedora-first, Rust-native evidence platform for Rust
applications and agent harnesses. The repository contains development tooling and
a typed Rust SDK, persistent JSONL adapter, Linux Spindle user service and early
analysis prototype in the `fabric` crate. Spindle provides bounded local SQLite
custody for SDK, file, journal and host observations. Fabric adds canonical
handoff and source-fenced run, service, tool and evidence queries through Rust,
CLI and MCP. See [local collection setup](docs/collection.md),
[canonical custody](docs/canonical-custody.md) and [typed investigations](docs/investigation.md).

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

Requires Jujutsu 0.46.0 with a Git backend and Rust 1.100.0 stable,
pinned in `rust-toolchain.toml`.
