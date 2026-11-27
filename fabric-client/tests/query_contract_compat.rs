//! The schema-only client and native API must share one contract and wire form.
use fabric_client::query_contract::{self, Operation, QueryRequest, RunKey};

const REQUEST: &str = r#"{"version":1,"query_id":"bounded","authority":"receiver","scope":["origin"],"snapshot":{"origin":"18446744073709551615"},"filter":{"run":{"kind":"global","run_id":"11111111-2222-4333-8444-555555555555"},"start_ns":null,"end_ns":null,"resource":null},"operation":{"operation":"activity","bucket_ns":"18446744073709551615"},"budgets":{"rows":1,"output_bytes":1024,"scan_rows":1,"timeout_ms":1},"cursor":null}"#;

#[test]
fn portable_query_keeps_existing_wide_wire_and_validation() {
    let request: QueryRequest = serde_json::from_str(REQUEST).unwrap();
    request.validate().unwrap();
    assert_eq!(serde_json::to_string(&request).unwrap(), REQUEST);
    assert!(matches!(
        &request.operation,
        Operation::Activity {
            bucket_ns: u64::MAX
        }
    ));
    let mut invalid = request.clone();
    invalid.filter.run = Some(RunKey::Global {
        run_id: "00000000-0000-0000-0000-000000000000".into(),
    });
    assert_eq!(
        invalid.validate().unwrap_err(),
        "run_id must be a lowercase UUID"
    );
    assert!(
        serde_json::from_str::<QueryRequest>(
            &REQUEST.replace("18446744073709551615", "18446744073709551616")
        )
        .is_err()
    );
    assert!(query_contract::valid_run_id(
        "11111111-2222-4333-8444-555555555555"
    ));
    assert!(!query_contract::valid_run_id(
        "11111111-2222-4333-8444-55555555555A"
    ));
}

#[cfg(feature = "cli")]
#[test]
fn native_and_portable_query_types_are_identical() {
    fn old_path(value: query_contract::QueryRequest) -> fabric::query::QueryRequest {
        value
    }
    fn client_path(value: fabric::query::QueryRequest) -> fabric_client::QueryRequest {
        value
    }
    let request: QueryRequest = serde_json::from_str(REQUEST).unwrap();
    let native = old_path(request.clone());
    assert_eq!(
        serde_json::to_string(&client_path(native)).unwrap(),
        REQUEST
    );
}
