# Dependency constraints

The workspace uses direct version constraints and checked-in lockfiles. Shared
exact constraints for Arrow, Parquet, Chrono, HMAC, parking_lot, proptest, and
rusqlite are declared in the workspace manifest. Each member declares the
dependencies it uses. The separate fuzz workspace retains its own declarations
and lockfile.

| Layer | Current locked versions | Why they differ |
| --- | --- | --- |
| Arrow and Parquet entry crates | `arrow` and `parquet` 59.2.0 | Direct exact constraints in the application workspace. |
| Arrow components | `arrow-array`, `arrow-ipc`, `arrow-schema`, and other resolved `arrow-*` crates 59.3.2 | Arrow and Parquet allow compatible 59.x components. The repository evaluator also constrains these three components directly to 59.3.2. Cargo resolves one version of each Arrow component across the workspace. |
| DataFusion | `datafusion` 55.0.0 and resolved `datafusion-*` components 55.2.0 | The direct exact constraint applies to the entry crate. Its component dependencies use compatible 55.x ranges. |

The Fabric SDK crate (`fabric`) has no normal Arrow dependency. The
observability CLI, projection, and storage use Arrow and Parquet. The evaluator
(`xtask`) disables default features for its direct `arrow-ipc` dependency.
DataFusion enables LZ4 and Zstandard on the shared `arrow-ipc` crate in the
workspace graph.

The differing entry and component versions are compatible with the current
constraints. Treat any version alignment as a dependency update. Compare the
locked dependency graph and feature sets, then exercise historical Parquet
fixtures, bounded query and resource regressions, and the acceptance checks
before adopting a changed resolution.

A November 2, 2026 `cargo audit` 0.22.2 run against RustSec database revision
`263a4d951a0867d4bddc29dddd6d59427c78f9a7` found no known advisories or
informational warnings in the then-current 123-package lockfile. Run a fresh
audit and review license and notice obligations against the candidate lockfile
before distributing a release.
