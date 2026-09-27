#[cfg(test)]
mod tests {
    use fabric_o11y::{Payload, Scalar};
    use storage_probe::collect::{adapt_otlp, SourceConfig};

    fn cfg(first_event_id: u64) -> SourceConfig {
        SourceConfig { tenant: 7, source: 8, resource: 9, first_event_id }
    }
    fn adapt(input: &str) -> std::io::Result<Vec<fabric_o11y::Event>> {
        adapt_otlp(input.as_bytes(), &cfg(41))
    }
    fn record(body: &str) -> String {
        format!(r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{body}]}}]}}]}}"#)
    }
    fn good() -> &'static str { r#"{"body":{"stringValue":"ok"}}"# }

    #[test]
    fn unknowns_at_all_nine_levels_and_escaped_duplicate_members_fail() {
        let bad = [
            ("root", r#"{"resourceLogs":[],"future":0}"#.to_owned()),
            ("resource item", r#"{"resourceLogs":[{"scopeLogs":[],"future":0}]}"#.to_owned()),
            ("resource", r#"{"resourceLogs":[{"resource":{"future":0},"scopeLogs":[]}]}"#.to_owned()),
            ("scope item", r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[],"future":0}]}]}"#.to_owned()),
            ("scope", r#"{"resourceLogs":[{"scopeLogs":[{"scope":{"future":0},"logRecords":[]}]}]}"#.to_owned()),
            ("log", record(r#"{"body":{"stringValue":"ok"},"future":0}"#)),
            ("body", record(r#"{"body":{"stringValue":"ok","future":0}}"#)),
            ("KeyValue", record(r#"{"body":{"stringValue":"ok"},"attributes":[{"key":"k","value":{"intValue":1},"future":0}]}"#)),
            ("AnyValue", record(r#"{"body":{"stringValue":"ok"},"attributes":[{"key":"k","value":{"future":0}}]}"#)),
            ("escaped duplicate", record(r#"{"body":{"stringValue":"x"},"\u0062ody":{"stringValue":"y"}}"#)),
            ("unknown duplicate", r#"{"resourceLogs":[],"future":0,"future":1}"#.to_owned()),
        ];
        for (name, input) in bad { assert!(adapt(&input).is_err(), "{name}: {input}"); }
    }

    #[test]
    fn duplicate_members_at_every_object_depth_fail_but_array_duplicates_pass() {
        let bad = [
            ("root", r#"{"resourceLogs":[],"resourceLogs":[]}"#.to_owned()),
            ("resource item", r#"{"resourceLogs":[{"scopeLogs":[],"scopeLogs":[]}]}"#.to_owned()),
            ("resource", r#"{"resourceLogs":[{"resource":{"attributes":[],"attributes":[]},"scopeLogs":[]}]}"#.to_owned()),
            ("scope item", r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[],"logRecords":[]}]}]}"#.to_owned()),
            ("scope", r#"{"resourceLogs":[{"scopeLogs":[{"scope":{"name":"a","name":"b"},"logRecords":[]}]}]}"#.to_owned()),
            ("log", record(r#"{"body":{"stringValue":"a"},"body":{"stringValue":"b"}}"#)),
            ("body", record(r#"{"body":{"stringValue":"a","stringValue":"b"}}"#)),
            ("KeyValue", record(r#"{"body":{"stringValue":"a"},"attributes":[{"key":"x","key":"y","value":{"intValue":1}}]}"#)),
            ("AnyValue", record(r#"{"body":{"stringValue":"a"},"attributes":[{"key":"x","value":{"intValue":1,"intValue":2}}]}"#)),
        ];
        for (name, input) in bad { assert!(adapt(&input).is_err(), "{name}: {input}"); }
        let good = record(r#"{"body":{"stringValue":"a"},"attributes":[{"key":"x","value":{"intValue":1}},{"key":"x","value":{"intValue":2}}]}"#);
        assert_eq!(adapt(&good).unwrap()[0].attributes.len(), 4);
    }

    #[test]
    fn ordered_duplicate_attributes_survive_across_layers() {
        let input = r#"{"resourceLogs":[{"resource":{"attributes":[{"key":"x","value":{"stringValue":"r1"}},{"key":"x","value":{"stringValue":"r2"}}]},"scopeLogs":[{"scope":{"attributes":[{"key":"x","value":{"stringValue":"s1"}},{"key":"x","value":{"stringValue":"s2"}}]},"logRecords":[{"body":{"stringValue":"ok"},"attributes":[{"key":"x","value":{"stringValue":"l1"}},{"key":"x","value":{"stringValue":"l2"}}]}]}]}]}"#;
        let rows = adapt(input).unwrap();
        let attrs: Vec<_> = rows[0].attributes.iter().take(6).map(|a| (a.key.as_str(), match &a.value { Scalar::String(x) => x.as_str(), _ => panic!("wrong type") })).collect();
        assert_eq!(attrs, vec![("otel.resource.attr.x", "r1"), ("otel.resource.attr.x", "r2"), ("otel.scope.attr.x", "s1"), ("otel.scope.attr.x", "s2"), ("otel.log.attr.x", "l1"), ("otel.log.attr.x", "l2")]);
    }

    #[test]
    fn scalar_and_timestamp_numeric_boundaries() {
        let accepted = [
            (r#"{"intValue":"-9223372036854775808"}"#, "I64(-9223372036854775808)"),
            (r#"{"intValue":9223372036854775807}"#, "I64(9223372036854775807)"),
            (r#"{"doubleValue":-0.0}"#, "F64(-0.0)"),
            (r#"{"doubleValue":5e-324}"#, "F64(5e-324)"),
            (r#"{"doubleValue":1.7976931348623157e308}"#, "F64(1.7976931348623157e308)"),
        ];
        for (value, expected) in accepted {
            let input = record(&format!(r#"{{"body":{{"stringValue":"ok"}},"attributes":[{{"key":"x","value":{value}}}],"timeUnixNano":"9223372036854775807"}}"#));
            let rows = adapt(&input).unwrap_or_else(|e| panic!("rejected {value}: {e}"));
            assert_eq!(format!("{:?}", rows[0].attributes[0].value), expected, "{value}");
            assert_eq!(rows[0].event_time.0, i64::MAX);
        }
        for value in [r#"{"intValue":"9223372036854775808"}"#, r#"{"intValue":"-9223372036854775809"}"#, r#"{"intValue":18446744073709551615}"#, r#"{"intValue":"01e2"}"#, r#"{"doubleValue":1e309}"#, r#"{"doubleValue":"inf"}"#, r#"{"doubleValue":"-0.0"}"#, r#"{"doubleValue":null}"#] {
            let input = record(&format!(r#"{{"body":{{"stringValue":"ok"}},"attributes":[{{"key":"x","value":{value}}}]}}"#));
            assert!(adapt(&input).is_err(), "accepted {value}");
        }
        for ts in [r#""9223372036854775808""#, "9223372036854775808", r#""-0""#, r#""+1""#, "-1", "1.0", "null"] {
            let input = record(&format!(r#"{{"body":{{"stringValue":"ok"}},"timeUnixNano":{ts}}}"#));
            assert!(adapt(&input).is_err(), "accepted timestamp {ts}");
        }
    }

    #[test]
    fn identity_is_checked_by_physical_position_and_batch_is_atomic() {
        let input = record(r#"{"body":{"stringValue":"same"}},{"body":{"stringValue":"same"}}"#);
        let config = cfg(u64::MAX - 1);
        let rows = adapt_otlp(input.as_bytes(), &config).unwrap();
        assert_eq!((rows[0].id.0, rows[1].id.0), (u64::MAX - 1, u64::MAX));
        assert_eq!(config.first_event_id, u64::MAX - 1);
        let third = record(r#"{"body":{"stringValue":"same"}},{"body":{"stringValue":"same"}},{"body":{"stringValue":"same"}}"#);
        assert!(adapt_otlp(third.as_bytes(), &config).is_err(), "overflow accepted");
        let bad_late = record(r#"{"body":{"stringValue":"first"}},{"body":{"stringValue":"second"},"droppedAttributesCount":1}"#);
        assert!(adapt(&bad_late).is_err(), "partial batch returned");
    }

    #[test]
    fn empty_envelopes_and_exact_cap() {
        for input in [r#"{"resourceLogs":[]}"#, r#"{"resourceLogs":[{"scopeLogs":[]}]}"#, r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[]}]}]}"#] {
            assert!(adapt(input).unwrap().is_empty(), "{input}");
        }
        let mut exact = br#"{"resourceLogs":[]}"#.to_vec();
        exact.resize(64 * 1024 * 1024, b' ');
        assert!(adapt_otlp(&exact, &cfg(41)).unwrap().is_empty(), "exact cap rejected");
        exact.push(b' ');
        assert!(adapt_otlp(&exact, &cfg(41)).is_err(), "cap+1 accepted");
    }

    #[test]
    fn malformed_and_multi_resource_batches_fail() {
        assert!(adapt(&record(good())).is_ok());
        for input in [r#"{"resourceLogs":[]}{"resourceLogs":[]}"#, r#"{"resourceLogs":[{},{}]}"#, r#"{"resourceLogs":[{"scopeLogs":null}]}"#] {
            assert!(adapt(input).is_err(), "{input}");
        }
        let rows = adapt(&record(r#"{"body":{"stringValue":"ok"},"traceId":""}"#)).unwrap();
        assert!(matches!(&rows[0].payload, Payload::Log {body} if body == "ok"));
    }
}
