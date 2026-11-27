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
    assert_eq!(result["generation"], 5);
    assert_eq!(result["stale"], "unchanged");
}
