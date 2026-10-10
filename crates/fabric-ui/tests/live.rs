use fabric_ui::live::{
    MAX_BODY_BYTES, ReadQuery, decode_response, read_query, tail_snapshot, trace_layout,
    validate_answer,
};
use serde_json::json;

#[test]
fn telemetry_integers_survive_decoding_above_javascript_precision() {
    let value = decode_response(br#"{"rows":[{"observed_ns":18446744073709551615,"sequence":9007199254740993,"index":0}],"retained_from_ns":9007199254740993}"#).unwrap();
    let value = validate_answer(value).unwrap();
    assert_eq!(value["rows"][0]["observed_ns"].as_u64(), Some(u64::MAX));
    assert_eq!(value["rows"][0]["sequence"].to_string(), "9007199254740993");
    assert_eq!(value["retained_from_ns"].as_u64(), Some(9007199254740993));
}

#[test]
fn query_boundaries_and_utf8_filter_limit_refuse_invalid_inputs() {
    let query = read_query("logs", "", "", "9007199254740993", "9007199254740994", None).unwrap();
    assert!(matches!(
        query,
        ReadQuery::Logs {
            from_ns: 9007199254740993,
            to_ns: 9007199254740994,
            limit: 200,
            ..
        }
    ));
    for (from, to) in [
        ("1", "1"),
        ("2", "1"),
        ("0", "86400000000001"),
        ("18446744073709551616", "2"),
    ] {
        assert!(read_query("logs", "", "", from, to, None).is_err());
    }
    assert!(read_query("logs", "", &"😀".repeat(1025), "1", "2", None).is_err());
    assert!(read_query("rate", "", "requests", "1", "2", None).is_err());
    assert!(read_query("metrics", "", "", "1", "2", None).is_err());
    assert!(read_query("spans", "", "non-hex", "1", "2", None).is_err());
    assert!(read_query("logs", "", "", "1", "2", Some("a".repeat(4097))).is_err());
}

#[test]
fn transport_and_response_contracts_fail_closed() {
    assert!(decode_response(&vec![b' '; MAX_BODY_BYTES + 1]).is_err());
    assert!(validate_answer(json!({"rows":vec![json!({});1001]})).is_err());
    assert!(validate_answer(json!({"rows":[{"time_ns":1.5}]})).is_err());
    assert!(validate_answer(json!({"rows":[{"sequence":-1}]})).is_err());
}

#[test]
fn trace_coordinates_preserve_adjacent_large_times_and_label_page_missing_parent() {
    let origin = 1_700_000_000_000_000_000u64;
    let value = json!({"rows":[
        {"trace_id":"trace","span_id":"a","parent_span_id":"","start_ns":origin,"end_ns":origin+2,"name":"root","node":"one"},
        {"trace_id":"trace","span_id":"b","parent_span_id":"absent","start_ns":origin+1,"end_ns":origin+2,"name":"child","node":"two"}
    ]});
    let rows = trace_layout(&value).unwrap();
    assert_eq!(rows[1].left_percent, 50.0);
    assert_eq!(rows[1].width_percent, 50.0);
    assert!(rows[1].parent_unavailable);
    assert!(!rows[0].parent_unavailable);
    assert!(trace_layout(&json!({"rows":[{"span_id":"bad","start_ns":5,"end_ns":4}]})).is_err());
}

#[test]
fn tail_snapshots_preserve_identical_rows_without_inventing_event_deduplication() {
    let row =
        json!({"node_id":"01".repeat(16),"sequence":9007199254740993u64,"index":3,"body":"text"});
    let snapshot = tail_snapshot(&json!({"rows":[row.clone(),row.clone()]})).unwrap();
    assert_eq!(snapshot.rows.len(), 2);
    assert!(snapshot.rows[0].contains("9007199254740993"));
    let snapshot = tail_snapshot(&json!({"rows":vec![row;201]})).unwrap();
    assert_eq!(snapshot.rows.len(), 200);
    assert_eq!(snapshot.dropped, 1);
    let snapshot = tail_snapshot(&json!({"rows":[{"body":"😀".repeat(65536)}]})).unwrap();
    assert!(snapshot.rows.is_empty());
    assert_eq!(snapshot.dropped, 1);
}

#[test]
fn scoped_grant_cannot_expand_parent_resources_actions_or_bounds() {
    use fabric_ui::live::scoped_grant;
    let parent = json!({"actions":["telemetry_read","inventory_read"],"installation_wide":false,
        "enrollments":["allowed"],"signals":["logs"],"max_query_window_s":3600,"max_query_rows":200,
        "allowed_log_paths":["/var/log/app.log"],"min_interval_s":5,"max_interval_s":60,
        "enrollment_namespace":"edge-","max_enrollments":10});
    assert!(scoped_grant(&parent.to_string(), &parent).is_ok());
    for (key, value) in [
        ("installation_wide", json!(true)),
        ("enrollments", json!(["other"])),
        ("actions", json!(["identity_manage"])),
        ("signals", json!(["metrics"])),
        ("max_query_window_s", json!(3601)),
        ("max_query_rows", json!(201)),
        ("allowed_log_paths", json!(["/etc/shadow"])),
        ("min_interval_s", json!(1)),
        ("max_interval_s", json!(61)),
        ("max_enrollments", json!(11)),
        ("enrollment_namespace", json!("other-")),
    ] {
        let mut expanded = parent.clone();
        expanded[key] = value;
        assert!(
            scoped_grant(&expanded.to_string(), &parent).is_err(),
            "{key}"
        );
    }
}

#[test]
fn metric_display_keeps_attribute_series_separate_and_raw_integer_values_exact() {
    use fabric_ui::live::metric_plots;
    let origin = 1_700_000_000_000_000_000u64;
    let query = read_query(
        "metrics",
        "",
        "counter",
        &origin.to_string(),
        &(origin + 2).to_string(),
        None,
    )
    .unwrap();
    let answer = json!({"rows":[
        {"node":"one","name":"counter","unit":"count","kind":"sum","attributes":{"route":"a"},"time_ns":origin,"value":9007199254740993u64},
        {"node":"one","name":"counter","unit":"count","kind":"sum","attributes":{"route":"b"},"time_ns":origin+1,"value":4}
    ]});
    let plots = metric_plots(&answer, &query).unwrap();
    assert_eq!(plots.len(), 2);
    assert!(plots[0].label.contains("a"));
    assert!(plots[1].label.contains("b"));
    assert_eq!(answer["rows"][0]["value"].as_u64(), Some(9007199254740993));
    let points: Vec<_> = plots[1].buckets.iter().filter_map(|b| b.min).collect();
    assert_eq!(points[0].x, 0.5);
}
