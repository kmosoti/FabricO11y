# Contributing

Use Jujutsu from PATH with a Git backend and native Cargo commands.
Initialize an existing Git checkout with `jj git init --colocate` and configure
your Jujutsu identity before committing.

Run `cargo xtask check --tier fast` during iteration and the medium tier before
submitting changes. The extensive tier adds Jujutsu and candidate integration checks.
See [verification](docs/verification.md) for candidate capture and receipts.

CI installs a checksum-verified Jujutsu release into the job user's bin directory
and runs checks in pinned Fedora userspace. Record the candidate commit, compiler
and check results in pull requests.
