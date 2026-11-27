use fabric_ops::Shape;
use fabric_ops::registry::{
    Descriptor, MAX_OPERATIONS, Operation, OperationError, Registry, RegistryError,
};
use std::cell::Cell;

static FIXTURES: [Descriptor; 2] = [
    Descriptor {
        protocol: "fixture",
        protocol_version: 1,
        operation: "one",
        operation_version: 1,
        shape: Shape::Unary,
    },
    Descriptor {
        protocol: "fixture",
        protocol_version: 1,
        operation: "two",
        operation_version: 1,
        shape: Shape::Unary,
    },
];

struct QueryRequest;
struct QueryResponse;
struct ListRuns;
impl Operation for ListRuns {
    type Request = QueryRequest;
    type Response = QueryResponse;
    const DESCRIPTOR: Descriptor = FIXTURES[0];
}
struct WrongShape;
impl Operation for WrongShape {
    type Request = QueryRequest;
    type Response = QueryResponse;
    const DESCRIPTOR: Descriptor = Descriptor {
        shape: Shape::Bidi,
        ..FIXTURES[0]
    };
}

#[test]
fn operation_version_is_checked_before_the_typed_handler() {
    let registry = Registry::new(&FIXTURES).unwrap();
    let called = Cell::new(false);
    let result = registry.negotiate::<ListRuns>("fixture", 1, "one", 2);
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
        .negotiate::<ListRuns>("fixture", 1, "one", 1)
        .unwrap();
    let _: QueryResponse = token.call(QueryRequest, |_| {
        called.set(true);
        QueryResponse
    });
    assert!(called.get());
    assert!(matches!(
        registry.negotiate::<ListRuns>("fixture", 2, "one", 1),
        Err(RegistryError::UnsupportedProtocolVersion)
    ));
    assert!(matches!(
        registry.negotiate::<ListRuns>("fixture", 1, "not_an_operation", 1),
        Err(RegistryError::UnsupportedOperation)
    ));
    assert!(matches!(
        registry.negotiate::<WrongShape>("fixture", 1, "one", 1),
        Err(RegistryError::BindingMismatch)
    ));
    assert!(matches!(
        registry.negotiate::<ListRuns>("fixture", 1, "two", 1),
        Err(RegistryError::BindingMismatch)
    ));
}

#[test]
fn inventory_validates_identity_shape_and_bounds() {
    static DUPLICATE: [Descriptor; 2] = [
        FIXTURES[0],
        Descriptor {
            shape: Shape::Bidi,
            ..FIXTURES[0]
        },
    ];
    static INVALID_NAME: [Descriptor; 1] = [Descriptor {
        operation: "",
        ..FIXTURES[0]
    }];
    static INVALID_VERSION: [Descriptor; 1] = [Descriptor {
        operation_version: 0,
        ..FIXTURES[0]
    }];
    static TOO_MANY: [Descriptor; MAX_OPERATIONS + 1] = [FIXTURES[0]; MAX_OPERATIONS + 1];
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
}
