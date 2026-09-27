//! Clarification probes fixed independently of candidate implementations.
use storage_probe::collect::{SourceConfig, adapt_otlp};

fn accepts(s: &str) -> bool {
    adapt_otlp(
        s.as_bytes(),
        &SourceConfig {
            tenant: 1,
            source: 2,
            resource: 3,
            first_event_id: 4,
        },
    )
    .is_ok()
}

fn envelope(record: &str) -> String {
    format!(r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{record}]}}]}}]}}"#)
}

#[test]
fn duplicate_object_members_are_rejected_at_each_layer() {
    for s in [
        r#"{"resourceLogs":[],"resourceLogs":[]}"#.to_string(),
        r#"{"resourceLogs":[{"resource":{},"resource":{},"scopeLogs":[]}]}"#.to_string(),
        r#"{"resourceLogs":[{"resource":{"attributes":[],"attributes":[]},"scopeLogs":[]}]}"#
            .to_string(),
        r#"{"resourceLogs":[{"scopeLogs":[{"scope":{},"scope":{},"logRecords":[]}]}]}"#.to_string(),
        r#"{"resourceLogs":[{"scopeLogs":[{"scope":{"name":"a","name":"b"},"logRecords":[]}]}]}"#
            .to_string(),
        envelope(r#"{"body":{"stringValue":"a"},"body":{"stringValue":"b"}}"#),
        envelope(r#"{"body":{"stringValue":"a","stringValue":"b"}}"#),
        envelope(
            r#"{"body":{"stringValue":"a"},"attributes":[{"key":"a","key":"b","value":{"intValue":1}}]}"#,
        ),
        envelope(
            r#"{"body":{"stringValue":"a"},"attributes":[{"key":"a","value":{"intValue":1,"intValue":2}}]}"#,
        ),
        envelope(r#"{"body":{"stringValue":"a"},"timeUnixNano":0,"timeUnixNano":1}"#),
    ] {
        assert!(!accepts(&s), "duplicate member accepted: {s}");
    }
    assert!(accepts(&envelope(
        r#"{"body":{"stringValue":"a"},"attributes":[{"key":"a","value":{"intValue":1}},{"key":"a","value":{"intValue":2}}]}"#
    )));
}

#[test]
fn explicit_null_and_numeric_extensions_are_rejected() {
    assert!(accepts(&envelope(r#"{"body":{"stringValue":"ok"}}"#)));
    for field in [
        "attributes",
        "severityText",
        "traceId",
        "spanId",
        "eventName",
        "flags",
        "severityNumber",
        "timeUnixNano",
        "observedTimeUnixNano",
        "droppedAttributesCount",
    ] {
        let s = envelope(&format!(
            r#"{{"body":{{"stringValue":"ok"}},"{field}":null}}"#
        ));
        assert!(!accepts(&s), "explicit null {field}");
    }
    for s in [
        r#"{"resourceLogs":[{"resource":null,"scopeLogs":[]}]}"#.to_string(),
        r#"{"resourceLogs":[{"scopeLogs":[{"scope":null,"logRecords":[]}]}]}"#.to_string(),
        r#"{"resourceLogs":[{"resource":{"droppedAttributesCount":"0"},"scopeLogs":[]}]}"#.to_string(),
        r#"{"resourceLogs":[{"scopeLogs":[{"scope":{"droppedAttributesCount":"0"},"logRecords":[]}]}]}"#.to_string(),
    ] { assert!(!accepts(&s), "invalid extension {s}"); }
    for field in ["flags", "severityNumber", "droppedAttributesCount"] {
        for value in ["\"0\"", "0.0", "-1", "4294967296"] {
            let s = envelope(&format!(
                r#"{{"body":{{"stringValue":"ok"}},"{field}":{value}}}"#
            ));
            assert!(!accepts(&s), "invalid numeric field {s}");
        }
    }
    for scalar in [
        r#"{"doubleValue":1e400}"#,
        r#"{"doubleValue":"1.5"}"#,
        r#"{"intValue":1.0}"#,
        r#"{"intValue":"+1"}"#,
        r#"{"intValue":"-"}"#,
    ] {
        let s = envelope(&format!(
            r#"{{"body":{{"stringValue":"ok"}},"attributes":[{{"key":"x","value":{scalar}}}]}}"#
        ));
        assert!(!accepts(&s), "invalid scalar {s}");
    }
}
