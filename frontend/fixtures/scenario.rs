//! Shared recorded scenario executed by the native and wasm harness.
use fabric_app_core::{Change, Effect, Event, Phase, State};
use fabric_query_contract::QueryRequest;
use serde_json::json;

pub fn run() -> String {
    let request: QueryRequest = serde_json::from_str(include_str!("request.json")).unwrap();
    let first = include_bytes!("first.json").to_vec();
    let second = include_bytes!("second.json").to_vec();
    let mut state = State::default();
    let start = state
        .transition(Event::Start {
            request: request.clone(),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    assert!(matches!(
        start.effect,
        Some(Effect::Request { generation: 1, .. })
    ));
    let stale = state
        .transition(Event::Response {
            generation: 9,
            bytes: first.clone(),
        })
        .unwrap();
    assert_eq!(stale.change, Change::Unchanged);
    state
        .transition(Event::Response {
            generation: 1,
            bytes: first.clone(),
        })
        .unwrap();
    state.transition(Event::Select(0)).unwrap();
    let next = state.transition(Event::Next).unwrap();
    match next.effect {
        Some(Effect::Request {
            generation: 2,
            request,
        }) => {
            assert_eq!(request.snapshot["alpha"], 3);
            assert_eq!(request.cursor.as_deref(), Some("opaque-token"));
        }
        _ => panic!("missing next request"),
    }
    state
        .transition(Event::Response {
            generation: 2,
            bytes: second.clone(),
        })
        .unwrap();
    let saved = json!({"phase":state.phase(),"rows":state.rows(),"selected":state.selected(),"coverages":state.coverages(),"next_cursor":state.next_cursor()});
    state
        .transition(Event::Start {
            request: request.clone(),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    let cancelled = state.transition(Event::Cancel).unwrap();
    assert!(matches!(
        cancelled.effect,
        Some(Effect::Cancel { generation: 3 })
    ));
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 3,
                bytes: first.clone()
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    assert_eq!(state.phase(), Phase::Cancelled);
    state
        .transition(Event::Start {
            request: request.clone(),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    state.transition(Event::Disconnected).unwrap();
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 4,
                bytes: second
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    let after_disconnect = state.phase();
    let reconnect = state
        .transition(Event::Start {
            request,
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    assert!(matches!(
        reconnect.effect,
        Some(Effect::Request { generation: 5, .. })
    ));
    state
        .transition(Event::Response {
            generation: 5,
            bytes: first,
        })
        .unwrap();
    json!({"completed":saved,"after_cancel":cancelled.change,"after_disconnect":after_disconnect,"after_reconnect":state.phase(),"generation":state.generation(),"stale":stale.change}).to_string()
}
