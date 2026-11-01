# Contributing

Use jj, native Cargo and the three check tiers. The first milestone is tooling;
product increments start after its merge. Do not rewrite or publish archive bookmarks.

Configure your own jj identity before publication. `jj util snapshot` captures
changes; `jj log --no-graph -r @ -T commit_id` identifies the candidate. Check
conflicts before capture. A new jj workspace is a new working commit, not an
immutable checkout: verify its content against the captured parent.

backlog/ is local, excluded using .git/info/exclude. Never force-track it.
/data/FabricO11y is preferred for artifacts, not required for routine builds.
A passing check is not publication authorization. Use an explicitly named bookmark
for any authorized push. Record exact candidate, compiler and checks in PRs.

Use the existing user jj from PATH (currently ~/.cargo/bin/jj). CI installs
a checksum-verified release into its user bin, not /usr/bin. No Copr or system
RPM policy is required. Record the executable/version actually used.
