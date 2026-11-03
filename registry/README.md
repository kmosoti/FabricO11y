# Observation semantics

`model/` defines Fabric's local vocabulary using OpenTelemetry Weaver's registry
format. It is not an OTLP schema or a claim of OpenTelemetry SDK compatibility.
The local wire protocol uses explicit nanosecond timestamps/durations and unsigned
sequence numbers; these are not automatically interchangeable with OTel attribute
integer representations. Metrics use explicit units (for example `By` for bytes).

Weaver 0.28.0 validates both registry revisions. `model/`, `templates/`,
`vocabulary.rs` and `descriptor.json` are the immutable version 2 contract; checks
pin their bytes and independently regenerate the artifacts for comparison.
`v3/model/` defines the global run context and authenticated generation lease
semantics. Its generated `v3/descriptor.json` extends the exact version 2
descriptor by digest, and reuses the version 2 data vocabulary without duplicating
its enums. `cargo xtask registry generate` updates only the version 3 descriptor;
it refuses any version 2 drift.

The generated descriptors accompany the checked-in Rust vocabulary. Rust types enforce the wire shape; runtime validation
also checks bounds, finite values, nesting, and identity. The descriptor is
retained with custody records, and changing it under an existing registry identity
is rejected on store open. Incompatible evolution requires a new schema identity.

```sh
sh .github/scripts/install-schema-tools.sh
cargo xtask registry check
cargo xtask registry generate
```

The installer verifies the upstream Linux x86_64 musl archive SHA-256. Checks verify
the installed executable digest and version. Set `FABRIC_SCHEMA_TOOLS` for an
alternate installation directory, or `FABRIC_WEAVER` for the exact pinned binary.
Normal Cargo compilation uses checked-in artifacts and does not download or run
the generator. Repository check tiers validate the registry and reject artifact
drift. Generation is explicit and must be reviewed with the semantic changes.

References: [Weaver registry definition](https://github.com/open-telemetry/weaver/blob/v0.28.0/docs/define-your-own-telemetry-schema.md),
[code generation](https://github.com/open-telemetry/weaver/blob/v0.28.0/docs/codegen.md),
and [OpenTelemetry units](https://opentelemetry.io/docs/specs/semconv/general/metrics/).
