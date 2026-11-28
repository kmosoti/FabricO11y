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
defaults, and wire behavior remain unchanged; applications must opt in to v4 or
the separate v5 metric-only descriptor.

`v5/model/` defines a separate opt-in, metric-only envelope extending the exact
v3 descriptor by digest; it does not extend or alter trace-only v4. V5 retains
the v3 context, authenticated generation, envelope and wire bounds, and accepts
only metric data. Its optional `data.labels` field is a nested string-to-string
object. Omission and `{}` are accepted; the object has at most eight entries.
After JSON string decoding, each key is 1–64 ASCII bytes. Its first character
must be an ASCII letter or underscore; subsequent characters may be ASCII
letters, digits, underscore, dot, or hyphen. Each value is 0–128 UTF-8 bytes,
including empty.
Unicode control characters and NUL are rejected. Duplicate decoded keys,
including escape aliases, are rejected. The reserved label keys are
`producer`, `generation`, `sequence`, `metric_unit`, `metric_type`, `reset`,
`context_run_id`, `version`, and `labels`. Labels remain nested under
`data.labels`; this schema does not flatten them into sibling metric fields.

V5 is an opt-in versioned wire contract, not an OTLP schema. The versioned
decoder accepts v2, v3, v4, and v5. V5 is accepted as a metric only through
versioned observation custody; Spindle reserves the bounded generation-scoped
series membership with the raw record. The collector can project a committed v5
record into its shared metric row. Spindle and the canonical receiver apply
separate bounded series-admission limits; the per-observation limit of eight
labels is not itself a total cardinality guarantee.

The direct local `ingest.sock` JSONL path accepts the separate tagged
`metric`, `log`, and `trace` records. It does not accept a v5 observation or a
direct metric row that claims Spindle-v5 provenance: those inputs lack the real
Spindle custody and series accounting required for v5 projection. This local
JSON shape is not OTLP JSON. See [architecture](../docs/architecture.md#local-ingest-and-versioned-observation-boundary)
for the fields retained by each projection.

`registry/v3/fixtures.json`, `registry/v4/fixtures.json`, and
`registry/v5/fixtures.json` are independent wire-contract cases. The v4 and v5
fixture bytes are pinned by the registry checker. Duplicate-key V5 cases retain
raw JSON text so literal duplicates and escape aliases are not erased by parsing
the fixture file itself. `cargo xtask registry generate` updates generated
version 3, 4 and 5 descriptors; it refuses any version 2 drift and checks the v4
and v5 fixture digests before writing.

The v2 descriptor accompanies the checked-in Rust vocabulary. Versioned runtime
validation enforces each implemented observation schema, and descriptors are
retained with custody records; changing one under an existing registry identity
is rejected on store open. The descriptors do not imply OTLP transport support or
general reader/writer negotiation. Incompatible evolution requires a new schema
identity.

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
