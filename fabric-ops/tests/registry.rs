use fabric_ops::Shape;
use fabric_ops::registry::{
    Descriptor, MAX_OPERATIONS, Operation, OperationError, QUERY_V1, Registry, RegistryError,
};
use std::cell::Cell;

struct QueryRequest;
struct QueryResponse;
struct ListRuns;
impl Operation for ListRuns {
    type Request = QueryRequest;
    type Response = QueryResponse;
    const DESCRIPTOR: Descriptor = QUERY_V1[1];
}
struct WrongShape;
impl Operation for WrongShape {
    type Request = QueryRequest;
    type Response = QueryResponse;
    const DESCRIPTOR: Descriptor = Descriptor {
        shape: Shape::Bidi,
        ..QUERY_V1[1]
    };
}

#[test]
fn operation_version_is_checked_before_the_typed_handler() {
    let registry = Registry::new(&QUERY_V1).unwrap();
    let called = Cell::new(false);
    let result = registry.negotiate::<ListRuns>("fabric.query", 1, "list_runs", 2);
    if let Ok(token) = result.as_ref() {
        let _ = token;
        called.set(true);
    }
    assert!(matches!(
        result,
        Err(RegistryError::UnsupportedOperationVersion)
    ));
    assert!(!called.get());
    let token = registry
        .negotiate::<ListRuns>("fabric.query", 1, "list_runs", 1)
        .unwrap();
    let _: QueryResponse = token.call(QueryRequest, |_| {
        called.set(true);
        QueryResponse
    });
    assert!(called.get());
    assert!(matches!(
        registry.negotiate::<ListRuns>("fabric.query", 2, "list_runs", 1),
        Err(RegistryError::UnsupportedProtocolVersion)
    ));
    assert!(matches!(
        registry.negotiate::<ListRuns>("fabric.query", 1, "not_an_operation", 1),
        Err(RegistryError::UnsupportedOperation)
    ));
    assert!(matches!(
        registry.negotiate::<WrongShape>("fabric.query", 1, "list_runs", 1),
        Err(RegistryError::BindingMismatch)
    ));
    assert!(matches!(
        registry.negotiate::<ListRuns>("fabric.query", 1, "evidence", 1),
        Err(RegistryError::BindingMismatch)
    ));
}

#[test]
fn inventory_validates_identity_shape_and_bounds() {
    static DUPLICATE: [Descriptor; 2] = [
        QUERY_V1[0],
        Descriptor {
            shape: Shape::Bidi,
            ..QUERY_V1[0]
        },
    ];
    static INVALID_NAME: [Descriptor; 1] = [Descriptor {
        operation: "",
        ..QUERY_V1[0]
    }];
    static INVALID_VERSION: [Descriptor; 1] = [Descriptor {
        operation_version: 0,
        ..QUERY_V1[0]
    }];
    static TOO_MANY: [Descriptor; MAX_OPERATIONS + 1] = [QUERY_V1[0]; MAX_OPERATIONS + 1];
    assert!(matches!(
        Registry::new(&DUPLICATE),
        Err(RegistryError::DuplicateDescriptor)
    ));
    assert!(matches!(
        Registry::new(&INVALID_NAME),
        Err(RegistryError::InvalidDescriptor)
    ));
    assert!(matches!(
        Registry::new(&INVALID_VERSION),
        Err(RegistryError::InvalidDescriptor)
    ));
    assert!(matches!(
        Registry::new(&TOO_MANY),
        Err(RegistryError::TooManyOperations)
    ));
    let names: Vec<_> = QUERY_V1.iter().map(|d| d.operation).collect();
    assert_eq!(
        names,
        [
            "capabilities",
            "list_runs",
            "run_summary",
            "tool_summary",
            "service_summary",
            "activity",
            "evidence"
        ]
    );
    assert!(
        QUERY_V1.iter().all(|d| d.protocol_version == 1
            && d.operation_version == 1
            && d.shape == Shape::Unary)
    );
}

#[test]
fn normalized_outcomes_are_distinct_and_contain_no_payload() {
    let outcomes = [
        OperationError::Retry,
        OperationError::Reconcile,
        OperationError::Expired,
        OperationError::UnknownOutcome,
    ];
    for (i, outcome) in outcomes.iter().enumerate() {
        assert!(!outcomes[..i].contains(outcome));
    }
    assert_eq!(std::mem::size_of::<OperationError>(), 1);
}
