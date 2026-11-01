# FabricO11y

FabricO11y is a planned Fedora-first, Rust-native evidence platform.

The SDK, Spindle, analytical storage and agent API are **not implemented**.
The former product is preserved locally at jj bookmark `archive/202f8f5c23d04fdad1215b9e59646a4fae5b4c88`;
dirty local worktrees were subsequently deleted at the owner’s request. No old runtime is reused.

Read [product direction](docs/direction.md), [contributing](CONTRIBUTING.md) and
[verification](docs/verification.md). Local backlog/ holds the product implementation programme.

```sh
cargo xtask doctor
cargo xtask check --tier fast
cargo xtask check --tier medium
cargo xtask check --tier extensive
cargo xtask candidate capture /data/FabricO11y/evidence/candidate.txt
cargo xtask candidate verify /data/FabricO11y/evidence/candidate.txt
```

Jujutsu 0.46.0, Git backend; Rust 1.100.0-beta.7, compiler commit
fd8e211ae301c84a86d13cf52a6d3ce08c589e2e. No resource ceiling is imposed.
