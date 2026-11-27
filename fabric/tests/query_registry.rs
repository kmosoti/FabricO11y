use fabric::query::{
    self, Budgets, ConsumedBudget, Coverage, Filter, Operation, ProjectionProgress, QueryRequest,
    QueryResponse,
};
use fabric_ops::registry::{Registry, RegistryError};
use std::cell::Cell;

fn request(operation: Operation) -> QueryRequest {
    QueryRequest {
        version: query::API_VERSION,
        query_id: "test".into(),
        authority: None,
        scope: vec!["source".into()],
        snapshot: Default::default(),
        filter: Filter::default(),
        operation,
        budgets: Budgets::default(),
        cursor: None,
    }
}
fn response() -> QueryResponse {
    QueryResponse {
        version: query::API_VERSION,
        rows: vec![],
        coverage: Coverage {
            canonical_authority: "a".into(),
            scope: vec![],
            snapshot: Default::default(),
            transform: query::TRANSFORM_VERSION.into(),
            schemas: vec![],
            projection: ProjectionProgress::default(),
            canonical_frontier: Default::default(),
            observed_gaps: vec![],
            complete: true,
            truncated: false,
            consumed: ConsumedBudget::default(),
        },
        next_cursor: None,
    }
}

#[test]
fn actual_query_variants_bind_dtos_and_reject_operation_version_before_handler() {
    let registry = Registry::new(&query::contracts::QUERY_V1).unwrap();
    let called = Cell::new(false);
    let bad = query::contracts::negotiate(&registry, request(Operation::ListRuns), 2);
    assert!(matches!(
        bad,
        Err(RegistryError::UnsupportedOperationVersion)
    ));
    if let Ok(gate) = bad {
        gate.call(|_| {
            called.set(true);
            response()
        });
    }
    assert!(!called.get());
    for op in [
        Operation::Capabilities,
        Operation::ListRuns,
        Operation::RunSummary,
        Operation::ToolSummary,
        Operation::ServiceSummary,
        Operation::Activity { bucket_ns: 1 },
        Operation::Evidence,
    ] {
        let gate = query::contracts::negotiate(&registry, request(op), 1).unwrap();
        let result: QueryResponse = gate.call(|req: QueryRequest| {
            assert_eq!(req.version, 1);
            called.set(true);
            response()
        });
        assert_eq!(result.version, 1);
    }
    assert!(called.get());
}
