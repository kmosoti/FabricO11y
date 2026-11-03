# Experimental Arrow dependencies

The batch evaluator pins `arrow-array`, `arrow-schema` and `arrow-ipc` to 60.1.0.
These dependencies belong to repository tooling; the production `fabric` crate
has no Arrow dependency. IPC compression features are disabled.

The locked Arrow dependency closure contains 70 packages, including dependencies
for other platforms. Cargo metadata declares Apache-2.0, MIT, BSD-2-Clause,
Unicode-3.0, CC0-1.0 and Zlib terms or expressions offering those options. For
`MIT OR Apache-2.0 OR LGPL-2.1-or-later`, the MIT option is available; LGPL is not
required by that expression. `Apache-2.0 AND MIT` and Unicode combinations require
preserving the applicable notices together. Apache license/notice and other
attribution requirements still apply when distributing dependency code or binaries.
This metadata review does not replace inspection of notices when packaging.

A cargo-audit 0.22.2 review of the complete 123-package lockfile against RustSec
revision `263a4d951a0867d4bddc29dddd6d59427c78f9a7` (2026-11-02) reported no known
vulnerabilities and no informational warnings. This is a point-in-time advisory
check, not an audit of dependency implementations. Repeat it when dependencies
change or before distributing a release.
