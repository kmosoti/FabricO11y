# S5 bounded OTLP/JSON file adapter

Status: registered before implementation. Add `collect` to the research package and
`fabric-research adapt-otlp REQUEST_JSON CONFIG_JSON FRESH_INPUT` to the lifecycle CLI.
This is an explicitly restricted, offline OTLP/JSON Log export profile, not a full
OTLP receiver, HTTP service, metrics/traces adapter or schema compatibility claim.
The retained request is the source for retry; output uses the S2 lossless Event codec.

```rust
use std::io;
use fabric_o11y::Event;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceConfig { pub tenant: u64, pub source: u64, pub resource: u64,
                          pub first_event_id: u64 }
pub fn adapt_otlp(bytes: &[u8], config: &SourceConfig) -> io::Result<Vec<Event>>;
```

Cap input at 64 MiB before parsing. Validate the entire request before returning any
Events or opening an output/log file. Preserve original resource/scope/log ordering.
Assign EventId as checked first_event_id + physical input position; duplicates in
content remain distinct. TenantId/SourceId/ResourceId come from trusted caller config,
never from telemetry fields. A repeated request with unchanged config maps identically.
A fresh output requires file sync and parent sync; emit JSON `adapted_events` and
`output` only after success. Input and config are never mutated.

Supported request envelope: exactly `resourceLogs`, an array with zero or one item.
More resource groups are rejected because this profile binds one ResourceId. Each
item supports `resource` (object with optional attributes and droppedAttributesCount),
`scopeLogs` (array), and optional `schemaUrl` string. Each scopeLogs item supports
optional `scope` (name/version strings, attributes array, droppedAttributesCount),
`logRecords` array and optional schemaUrl. Required arrays may be empty. Unknown
fields are rejected by this strict profile; unlike a general OTLP receiver it does
not silently ignore future extensions. Explicitly unsupported structures fail.

A LogRecord supports `timeUnixNano`, `observedTimeUnixNano`, `body`, `attributes`,
`severityNumber`, `severityText`, `traceId`, `spanId`, `flags`, `droppedAttributesCount`,
and `eventName`. Body is required and has exactly `stringValue: string`; other AnyValue
bodies fail. Attributes are ordered KeyValue objects with exactly key/value; preserve
duplicate keys. AnyValue has exactly one scalar field: stringValue, boolValue,
intValue (signed i64 decimal string or JSON integer), doubleValue (JSON number or
exact strings `NaN`, `Infinity`, `-Infinity`). Arrays, kvlists and bytes fail. Floating
point strings have the protocol's canonical meaning; they do not carry arbitrary NaN
payload bits. Finite parsed doubles and signed zero retain their Rust f64 bits.

Timestamps accept decimal strings or unsigned JSON integers, require <=i64::MAX,
and default to zero when omitted. Zero is retained as the protocol's unspecified
value, never replaced with a clock reading. Append presence flags in attributes so
an omitted timestamp is distinguishable from an explicit zero. Numeric strings use
digits only (signed intValue may have one leading minus); no whitespace, plus sign,
exponent or overflow. severityNumber is a u32 <=24; flags is u32. If present, traceId
is empty or 32 hex digits and spanId empty or 16 hex digits, retaining exact text.
All droppedAttributesCount fields must be zero (or absent); nonzero rejects because
this profile refuses known upstream loss. No promise of global source completeness
follows from accepting a batch.

Map attributes in this stable order: resource attributes to `otel.resource.attr.<key>`;
resource schema URL to `otel.resource.schema_url` when present; scope attributes to
`otel.scope.attr.<key>`; scope name/version/schema URL to `otel.scope.name`,
`otel.scope.version`, `otel.scope.schema_url` when present; log attributes to
`otel.log.attr.<key>`; optional severityNumber/severityText/traceId/spanId/flags/eventName
in that order to `otel.log.field.<wire-field-name>` (numeric values become Scalar::U64);
finally Bool flags `otel.log.field.time_present` and `otel.log.field.observed_time_present`.
No attribute key normalization or deduplication. Missing optional metadata creates no
attribute except the two presence flags. Payload is Log body; event/observed times
are the parsed/default nanoseconds. This mapping is version-1 profile behavior.

The receiver boundary for this experiment is the bounded caller-owned file request.
EventBuffer backpressure and borrowed append ownership remain the lifecycle CLI's
ingest contract. Buffer full returns the exact Event; do not drop on probabilistic
hints. The input cap and queue cap are different bounds. No network authentication,
transport retry, encryption or physical-device throughput is claimed here.

Parsing clarifications fixed before candidate implementation: an omitted `resource`
is an empty resource object. Any explicitly null optional value is rejected. Duplicate
JSON object member names at every level are rejected (duplicate keys in the ordered
KeyValue attribute array are still retained). severityNumber, flags and every
droppedAttributesCount use unsigned JSON integers, never numeric strings. A finite
double that overflows f64 is rejected; the three named nonfinite strings remain the
only accepted nonfinite representations. These rules define this strict profile.

CLI configuration (fixed before command implementation): CONFIG_JSON is an object
with exactly the four required SourceConfig field names, each a JSON unsigned
integer. Reject duplicate/unknown/missing fields, nulls and numeric strings. Apply
the same 64 MiB file cap. Decode both request and configuration completely before
creating FRESH_INPUT; existing outputs are never replaced.
