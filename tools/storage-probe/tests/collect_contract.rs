//! Independent S5 oracle, written from COLLECT_API.md before the adapter exists.
//! The accepted inputs exercise the deliberately restricted offline profile.

use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId, TenantId,
};
use storage_probe::collect::{SourceConfig, adapt_otlp};

#[derive(Clone, Copy)]
enum ExpectedValue<'a> {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64Bits(u64),
    NaN,
    String(&'a str),
}

fn config() -> SourceConfig {
    SourceConfig {
        tenant: 17,
        source: 23,
        resource: 31,
        first_event_id: 101,
    }
}

fn mapped(input: &str) -> Vec<Event> {
    adapt_otlp(input.as_bytes(), &config()).expect("registered input should adapt")
}

fn assert_attr(event: &Event, ordinal: usize, key: &str, value: ExpectedValue<'_>) {
    let actual = &event.attributes[ordinal];
    assert_eq!(actual.key, key, "attribute key at {ordinal}");
    match (&actual.value, value) {
        (Scalar::Bool(a), ExpectedValue::Bool(b)) => assert_eq!(*a, b),
        (Scalar::I64(a), ExpectedValue::I64(b)) => assert_eq!(*a, b),
        (Scalar::U64(a), ExpectedValue::U64(b)) => assert_eq!(*a, b),
        (Scalar::F64(a), ExpectedValue::F64Bits(b)) => assert_eq!(a.to_bits(), b),
        (Scalar::F64(a), ExpectedValue::NaN) => assert!(a.is_nan()),
        (Scalar::String(a), ExpectedValue::String(b)) => assert_eq!(a, b),
        _ => panic!("attribute value mismatch at {ordinal}: {:?}", actual.value),
    }
}

fn assert_identity(event: &Event, id: u64, tenant: u64, source: u64, resource: u64) {
    assert_eq!(event.id, EventId(id));
    assert_eq!(event.tenant, TenantId(tenant));
    assert_eq!(event.source, SourceId(source));
    assert_eq!(event.resource, ResourceId(resource));
}

fn assert_log(event: &Event, body: &str, event_time: i64, observed_time: i64) {
    assert_eq!(event.event_time, EventTime(event_time));
    assert_eq!(event.observed_time, ObservedTime(observed_time));
    match &event.payload {
        Payload::Log { body: actual } => assert_eq!(actual, body),
        other => panic!("expected Log payload, got {other:?}"),
    }
}

#[test]
fn maps_every_supported_layer_and_scalar_in_registered_order() {
    let request = r#"{
      "resourceLogs":[{
        "resource":{"attributes":[
          {"key":"same","value":{"stringValue":"resource α"}},
          {"key":"same","value":{"boolValue":true}},
          {"key":"number","value":{"intValue":"-9223372036854775808"}}
        ],"droppedAttributesCount":0},
        "schemaUrl":"res://v1",
        "scopeLogs":[{
          "scope":{"name":"scope ☃","version":"1.0","attributes":[
            {"key":"same","value":{"doubleValue":-0.0}},
            {"key":"same","value":{"doubleValue":"NaN"}},
            {"key":"same","value":{"doubleValue":"Infinity"}},
            {"key":"same","value":{"doubleValue":"-Infinity"}}
          ],"droppedAttributesCount":0},
          "schemaUrl":"scope://v2",
          "logRecords":[{
            "timeUnixNano":"9223372036854775807",
            "observedTimeUnixNano":0,
            "body":{"stringValue":"你好 🌍\nline two"},
            "attributes":[
              {"key":"same","value":{"intValue":42}},
              {"key":"same","value":{"stringValue":"log β"}},
              {"key":"finite","value":{"doubleValue":1.25}}
            ],
            "severityNumber":24,"severityText":"WARN",
            "traceId":"ABCDEF0123456789abcdef0123456789",
            "spanId":"01234567ABCDEFab","flags":4294967295,
            "droppedAttributesCount":0,"eventName":"event 🎯"
          }]
        }]
      }]
    }"#;
    let rows = mapped(request);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_identity(row, 101, 17, 23, 31);
    assert_log(row, "你好 🌍\nline two", i64::MAX, 0);
    use ExpectedValue as V;
    let expected = [
        ("otel.resource.attr.same", V::String("resource α")),
        ("otel.resource.attr.same", V::Bool(true)),
        ("otel.resource.attr.number", V::I64(i64::MIN)),
        ("otel.resource.schema_url", V::String("res://v1")),
        ("otel.scope.attr.same", V::F64Bits((-0.0_f64).to_bits())),
        ("otel.scope.attr.same", V::NaN),
        ("otel.scope.attr.same", V::F64Bits(f64::INFINITY.to_bits())),
        (
            "otel.scope.attr.same",
            V::F64Bits(f64::NEG_INFINITY.to_bits()),
        ),
        ("otel.scope.name", V::String("scope ☃")),
        ("otel.scope.version", V::String("1.0")),
        ("otel.scope.schema_url", V::String("scope://v2")),
        ("otel.log.attr.same", V::I64(42)),
        ("otel.log.attr.same", V::String("log β")),
        ("otel.log.attr.finite", V::F64Bits(1.25_f64.to_bits())),
        ("otel.log.field.severityNumber", V::U64(24)),
        ("otel.log.field.severityText", V::String("WARN")),
        (
            "otel.log.field.traceId",
            V::String("ABCDEF0123456789abcdef0123456789"),
        ),
        ("otel.log.field.spanId", V::String("01234567ABCDEFab")),
        ("otel.log.field.flags", V::U64(u32::MAX as u64)),
        ("otel.log.field.eventName", V::String("event 🎯")),
        ("otel.log.field.time_present", V::Bool(true)),
        ("otel.log.field.observed_time_present", V::Bool(true)),
    ];
    assert_eq!(row.attributes.len(), expected.len());
    for (ordinal, (key, value)) in expected.into_iter().enumerate() {
        assert_attr(row, ordinal, key, value);
    }
}

#[test]
fn omitted_metadata_and_timestamps_are_distinct_from_explicit_zero() {
    let rows = mapped(
        r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[
      {"body":{"stringValue":"same"}},
      {"body":{"stringValue":"same"},"timeUnixNano":"0","observedTimeUnixNano":0}
    ]}]}]}"#,
    );
    assert_eq!(rows.len(), 2);
    for (position, row) in rows.iter().enumerate() {
        assert_identity(row, 101 + position as u64, 17, 23, 31);
        assert_log(row, "same", 0, 0);
        assert_eq!(row.attributes.len(), 2);
        assert_attr(
            row,
            0,
            "otel.log.field.time_present",
            ExpectedValue::Bool(position == 1),
        );
        assert_attr(
            row,
            1,
            "otel.log.field.observed_time_present",
            ExpectedValue::Bool(position == 1),
        );
    }
    let again = mapped(
        r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[
      {"body":{"stringValue":"same"}},
      {"body":{"stringValue":"same"},"timeUnixNano":"0","observedTimeUnixNano":0}
    ]}]}]}"#,
    );
    for (left, right) in rows.iter().zip(&again) {
        assert_eq!(format!("{left:?}"), format!("{right:?}"));
    }
}

#[test]
fn traversal_uses_physical_resource_scope_log_position() {
    let rows = mapped(
        r#"{"resourceLogs":[{"resource":{},"scopeLogs":[
      {"logRecords":[{"body":{"stringValue":"a"}},{"body":{"stringValue":"a"}}]},
      {"logRecords":[]},
      {"logRecords":[{"body":{"stringValue":"b"}}]}
    ]}]}"#,
    );
    assert_eq!(rows.len(), 3);
    for (index, body) in ["a", "a", "b"].into_iter().enumerate() {
        assert_identity(&rows[index], 101 + index as u64, 17, 23, 31);
        assert_log(&rows[index], body, 0, 0);
    }
}

#[test]
fn trusted_config_sets_identity_and_telemetry_cannot_override_tenant() {
    let bytes = br#"{"resourceLogs":[{"resource":{"attributes":[{"key":"tenant","value":{"intValue":"999"}}]},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"tenant","value":{"intValue":"999"}}]}]}]}]}"#;
    let first = adapt_otlp(bytes, &config()).unwrap();
    let other = SourceConfig {
        tenant: 88,
        source: 23,
        resource: 31,
        first_event_id: 101,
    };
    let second = adapt_otlp(bytes, &other).unwrap();
    assert_identity(&first[0], 101, 17, 23, 31);
    assert_identity(&second[0], 101, 88, 23, 31);
    assert_attr(
        &first[0],
        0,
        "otel.resource.attr.tenant",
        ExpectedValue::I64(999),
    );
    assert_attr(
        &first[0],
        1,
        "otel.log.attr.tenant",
        ExpectedValue::I64(999),
    );
}

#[test]
fn empty_envelopes_yield_no_events() {
    for input in [
        r#"{"resourceLogs":[]}"#,
        r#"{"resourceLogs":[{"resource":{},"scopeLogs":[]}] }"#,
        r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[]}]}]}"#,
    ] {
        assert!(mapped(input).is_empty(), "{input}");
    }
}

#[test]
fn explicitly_present_empty_optional_strings_and_zero_numbers_are_preserved() {
    let rows = mapped(
        r#"{"resourceLogs":[{"resource":{},"schemaUrl":"","scopeLogs":[{"scope":{"name":"","version":""},"schemaUrl":"","logRecords":[{"body":{"stringValue":""},"severityNumber":0,"severityText":"","traceId":"","spanId":"","flags":0,"eventName":""}]}]}]}"#,
    );
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_log(row, "", 0, 0);
    use ExpectedValue as V;
    let expected = [
        ("otel.resource.schema_url", V::String("")),
        ("otel.scope.name", V::String("")),
        ("otel.scope.version", V::String("")),
        ("otel.scope.schema_url", V::String("")),
        ("otel.log.field.severityNumber", V::U64(0)),
        ("otel.log.field.severityText", V::String("")),
        ("otel.log.field.traceId", V::String("")),
        ("otel.log.field.spanId", V::String("")),
        ("otel.log.field.flags", V::U64(0)),
        ("otel.log.field.eventName", V::String("")),
        ("otel.log.field.time_present", V::Bool(false)),
        ("otel.log.field.observed_time_present", V::Bool(false)),
    ];
    assert_eq!(row.attributes.len(), expected.len());
    for (ordinal, (key, value)) in expected.into_iter().enumerate() {
        assert_attr(row, ordinal, key, value);
    }
}

#[test]
fn json_integer_boundary_and_unicode_keys_preserve_their_source_spelling() {
    let rows = mapped(
        r#"{"resourceLogs":[{"resource":{"attributes":[{"key":"é","value":{"intValue":9223372036854775807}},{"key":"é","value":{"boolValue":false}}]},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"ok"},"timeUnixNano":9223372036854775807}]}]}]}"#,
    );
    assert_eq!(rows.len(), 1);
    assert_log(&rows[0], "ok", i64::MAX, 0);
    assert_eq!(rows[0].attributes.len(), 4);
    assert_attr(
        &rows[0],
        0,
        "otel.resource.attr.é",
        ExpectedValue::I64(i64::MAX),
    );
    assert_attr(
        &rows[0],
        1,
        "otel.resource.attr.é",
        ExpectedValue::Bool(false),
    );
    assert_attr(
        &rows[0],
        2,
        "otel.log.field.time_present",
        ExpectedValue::Bool(true),
    );
    assert_attr(
        &rows[0],
        3,
        "otel.log.field.observed_time_present",
        ExpectedValue::Bool(false),
    );
}

#[test]
fn rejects_bad_shapes_ranges_and_unsupported_values_atomically() {
    let invalid = [
        ("malformed JSON", "{"),
        ("missing envelope", "{}"),
        ("wrong envelope shape", r#"{"resourceLogs":{}}"#),
        ("unknown envelope field", r#"{"resourceLogs":[],"extra":1}"#),
        (
            "multiple resources",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[]},{"resource":{},"scopeLogs":[]}]}"#,
        ),
        ("missing scopeLogs", r#"{"resourceLogs":[{"resource":{}}]}"#),
        (
            "wrong resource shape",
            r#"{"resourceLogs":[{"resource":[],"scopeLogs":[]}]}"#,
        ),
        (
            "wrong scopeLogs shape",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":{}}]}"#,
        ),
        (
            "missing logRecords",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{}]}]}"#,
        ),
        (
            "wrong logRecords shape",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":{}}]}]}"#,
        ),
        (
            "unknown resourceLogs field",
            r#"{"resourceLogs":[{"resource":{},"extra":1,"scopeLogs":[]}]}"#,
        ),
        (
            "unknown resource field",
            r#"{"resourceLogs":[{"resource":{"extra":1},"scopeLogs":[]}]}"#,
        ),
        (
            "unknown scope field",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"scope":{"extra":1},"logRecords":[]}]}]}"#,
        ),
        (
            "unknown scopeLogs field",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"extra":1,"logRecords":[]}]}]}"#,
        ),
        (
            "unknown log field",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"extra":1}]}]}]}"#,
        ),
        (
            "missing body",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{}]}]}]}"#,
        ),
        (
            "unsupported body",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"intValue":"1"}}]}]}]}"#,
        ),
        (
            "extra body field",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x","boolValue":true}}]}]}]}"#,
        ),
        (
            "keyvalue extra field",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"boolValue":true},"extra":1}]}]}]}]}"#,
        ),
        (
            "unsupported array value",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"arrayValue":{}}}]}]}]}]}"#,
        ),
        (
            "unsupported bytes value",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"bytesValue":"AA=="}}]}]}]}]}"#,
        ),
        (
            "unsupported kvlist value",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"kvlistValue":{}}}]}]}]}]}"#,
        ),
        (
            "multiple scalar fields",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"boolValue":true,"intValue":"1"}}]}]}]}]}"#,
        ),
        (
            "bad signed integer string",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"intValue":"+1"}}]}]}]}]}"#,
        ),
        (
            "fractional double as string",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"doubleValue":"1.25"}}]}]}]}]}"#,
        ),
        (
            "decimal string with whitespace",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"observedTimeUnixNano":" 1"}]}]}]}"#,
        ),
        (
            "signed integer overflow",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"attributes":[{"key":"a","value":{"intValue":"9223372036854775808"}}]}]}]}]}"#,
        ),
        (
            "negative time",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"timeUnixNano":"-1"}]}]}]}"#,
        ),
        (
            "time overflow",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"timeUnixNano":"9223372036854775808"}]}]}]}"#,
        ),
        (
            "noninteger time",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"observedTimeUnixNano":1.5}]}]}]}"#,
        ),
        (
            "severity out of range",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"severityNumber":25}]}]}]}"#,
        ),
        (
            "flags overflow",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"flags":4294967296}]}]}]}"#,
        ),
        (
            "invalid trace ID",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"traceId":"not-hex"}]}]}]}"#,
        ),
        (
            "invalid span ID",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"spanId":"123"}]}]}]}"#,
        ),
        (
            "resource upstream loss",
            r#"{"resourceLogs":[{"resource":{"droppedAttributesCount":1},"scopeLogs":[]}]}"#,
        ),
        (
            "scope upstream loss",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"scope":{"droppedAttributesCount":1},"logRecords":[]}]}]}"#,
        ),
        (
            "log upstream loss",
            r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"x"},"droppedAttributesCount":1}]}]}]}"#,
        ),
    ];
    for (reason, request) in invalid {
        assert!(
            adapt_otlp(request.as_bytes(), &config()).is_err(),
            "accepted {reason}: {request}"
        );
    }
}

#[test]
fn one_invalid_later_record_rejects_the_whole_batch() {
    let input = r#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[
      {"body":{"stringValue":"valid"}},
      {"body":{"stringValue":"invalid"},"droppedAttributesCount":1}
    ]}]}]}"#;
    assert!(adapt_otlp(input.as_bytes(), &config()).is_err());
}

#[test]
fn event_id_addition_is_checked_at_the_last_position() {
    let input = br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":"first"}},{"body":{"stringValue":"second"}}]}]}]}"#;
    let last = SourceConfig {
        first_event_id: u64::MAX,
        ..config()
    };
    assert!(adapt_otlp(input, &last).is_err());
    let boundary = SourceConfig {
        first_event_id: u64::MAX - 1,
        ..config()
    };
    let rows = adapt_otlp(input, &boundary).unwrap();
    assert_eq!(rows[0].id, EventId(u64::MAX - 1));
    assert_eq!(rows[1].id, EventId(u64::MAX));
}

#[test]
fn input_cap_is_checked_before_json_parsing() {
    // A valid oversized request distinguishes a missing 64 MiB cap.
    const CAP: usize = 64 * 1024 * 1024;
    let prefix =
        br#"{"resourceLogs":[{"resource":{},"scopeLogs":[{"logRecords":[{"body":{"stringValue":""#;
    let suffix = br#""}}]}]}]}"#;
    let mut input = Vec::with_capacity(CAP + 1);
    input.extend_from_slice(prefix);
    input.resize(CAP + 1 - suffix.len(), b'a');
    input.extend_from_slice(suffix);
    assert_eq!(input.len(), CAP + 1);
    assert!(adapt_otlp(&input, &config()).is_err());
}
