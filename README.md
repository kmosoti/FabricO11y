# FabricO11y

FabricO11y is a planned Fedora-first, Rust-native evidence platform for Rust
applications and agent harnesses. The repository currently provides development
tooling; the SDK, Spindle, analytical storage and agent API are planned.

Read [product direction](docs/direction.md), [contributing](CONTRIBUTING.md) and
[verification](docs/verification.md).

```sh
cargo xtask doctor
cargo xtask check --tier fast
cargo xtask check --tier medium
cargo xtask check --tier extensive
```

Requires Jujutsu 0.46.0 with a Git backend and Rust 1.100.0-beta.7
(compiler commit fd8e211ae301c84a86d13cf52a6d3ce08c589e2e).
