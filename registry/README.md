# Observation semantics

`model/` defines Fabric's local vocabulary using OpenTelemetry Weaver's registry
format. It is not an OTLP schema or a claim of OpenTelemetry SDK compatibility.
The local wire protocol uses explicit nanosecond timestamps/durations and unsigned
sequence numbers; these are not automatically interchangeable with OTel attribute
integer representations. Metrics use explicit units (for example `By` for bytes).

Weaver 0.28.0 validates all registry revisions. `model/`, `templates/`,
`vocabulary.rs` and `descriptor.json` are the immutable version 2 contract; checks
pin their bytes and independently regenerate the artifacts for comparison.
`v3/model/` defines the global run context and authenticated generation lease
semantics. Its generated `v3/descriptor.json` extends the exact version 2
descriptor by digest, and reuses the version 2 data vocabulary without duplicating
its enums. `v4/model/` defines an explicit trace-only observation envelope. Its
generated `v4/descriptor.json` extends the exact version 3 descriptor by digest,
retains its context and frame bounds, and restricts data to service requests,
agent runs, and tool executions. The required top-level `span` object carries a
plain lowercase nonzero 32-hex trace ID, a 16-hex span ID, and at most one
optional or null 16-hex parent span ID. A parent is scoped to the same trace and
must differ from its child span. If a service request supplies `data.trace_id`,
it must equal `span.trace_id`. Existing v2 and v3 descriptors, fixtures, SDK
defaults, and wire behavior remain unchanged; applications must opt in to v4.

`registry/v3/fixtures.json` and `registry/v4/fixtures.json` are independent
wire-contract cases. The v4 fixture bytes are pinned by the registry checker.
`cargo xtask registry generate` updates generated version 3 and 4 descriptors;
it refuses any version 2 drift and checks the v4 fixture digest before writing.

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
