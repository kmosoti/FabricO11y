#[path = "../../fixtures/scenario.rs"]
mod scenario;
#[test]
fn recorded_list_runs_scenario_preserves_returned_facts() {
    let result: serde_json::Value = serde_json::from_str(&scenario::run()).unwrap();
    assert_eq!(result["completed"]["phase"], "ready");
    assert_eq!(result["completed"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(result["completed"]["selected"], 0);
    assert_eq!(
        result["completed"]["coverages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        result["completed"]["coverages"][0]["observed_gaps"][0],
        "gap-1"
    );
    assert_eq!(
        result["completed"]["coverages"][1]["snapshot"]["alpha"],
        "3"
    );
    assert_eq!(result["after_cancel"], "cancelled");
    assert_eq!(result["after_disconnect"], "unknown");
    assert_eq!(result["after_reconnect"], "ready");
    assert_eq!(result["generation"], 6);
    let effect = &result["detail_start"]["effect"];
    assert_eq!(effect["kind"], "request");
    assert_eq!(effect["generation"], 3);
    let request = &effect["request"];
    assert_eq!(request["operation"]["operation"], "run_summary");
    assert_eq!(request["authority"], "auth-1");
    assert_eq!(request["scope"], serde_json::json!(["alpha"]));
    assert_eq!(request["snapshot"], serde_json::json!({"alpha":"3"}));
    assert_eq!(request["cursor"], serde_json::Value::Null);
    assert_eq!(
        request["filter"]["run"],
        serde_json::json!({"kind":"global","run_id":"00000000-0000-4000-8000-000000000001"})
    );
    let row = &result["detail"]["rows"][0];
    assert_eq!(row["run"], request["filter"]["run"]);
    assert_eq!(row, &result["completed"]["rows"][0]);
    assert_eq!(row["count"], "1");
    assert_eq!(row["sum_ns"], "5");
    assert_eq!(result["detail"]["coverages"][0]["snapshot"]["alpha"], "3");
    assert_eq!(result["stale_detail"], "unchanged");
    assert_eq!(result["stale"], "unchanged");
}
