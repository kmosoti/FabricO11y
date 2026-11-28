//! Shared recorded scenario executed by the native and wasm harness.
use fabric_app_core::{Change, Effect, Event, Phase, State};
use fabric_query_contract::{Operation, QueryRequest};
use serde_json::json;

pub fn run() -> String {
    let request: QueryRequest = serde_json::from_str(include_str!("request.json")).unwrap();
    let first = include_bytes!("first.json").to_vec();
    let second = include_bytes!("second.json").to_vec();
    let mut state = State::default();
    let start = state
        .transition(Event::Start {
            request: Box::new(request.clone()),
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
    let mut detail = request.clone();
    detail.operation = Operation::RunSummary;
    detail.filter.run = Some(state.rows()[0].run.clone());
    detail.snapshot = state.coverages()[0].snapshot.clone();
    let detail_start = state
        .transition(Event::Start {
            request: Box::new(detail),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    let before_stale = serde_json::to_value(&state).unwrap();
    let stale_detail = state
        .transition(Event::Response {
            generation: 2,
            bytes: second.clone(),
        })
        .unwrap();
    assert_eq!(stale_detail.change, Change::Unchanged);
    assert_eq!(serde_json::to_value(&state).unwrap(), before_stale);
    state
        .transition(Event::Response {
            generation: 3,
            bytes: include_bytes!("detail.json").to_vec(),
        })
        .unwrap();
    let detail_result = json!({"rows": state.rows(), "coverages": state.coverages()});
    state
        .transition(Event::Start {
            request: Box::new(request.clone()),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    let cancelled = state.transition(Event::Cancel).unwrap();
    assert!(matches!(
        cancelled.effect,
        Some(Effect::Cancel { generation: 4 })
    ));
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 4,
                bytes: first.clone()
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    assert_eq!(state.phase(), Phase::Cancelled);
    state
        .transition(Event::Start {
            request: Box::new(request.clone()),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    state.transition(Event::Disconnected).unwrap();
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 5,
                bytes: second
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    let after_disconnect = state.phase();
    let reconnect = state
        .transition(Event::Start {
            request: Box::new(request.clone()),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    assert!(matches!(
        reconnect.effect,
        Some(Effect::Request { generation: 6, .. })
    ));
    let before_late = serde_json::to_vec(&state).unwrap();
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 4,
                bytes: first.clone()
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before_late);
    state
        .transition(Event::Response {
            generation: 6,
            bytes: first,
        })
        .unwrap();
    // Restarting a cancelled continuation is a fresh first page at its fixed snapshot.
    let mut paged = State::default();
    paged
        .transition(Event::Start {
            request: Box::new(request.clone()),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    paged
        .transition(Event::Response {
            generation: 1,
            bytes: include_bytes!("first.json").to_vec(),
        })
        .unwrap();
    let next = paged.transition(Event::Next).unwrap();
    let Some(Effect::Request {
        request: next_request,
        ..
    }) = next.effect
    else {
        panic!("missing page")
    };
    paged.transition(Event::Cancel).unwrap();
    let mut fresh = *next_request;
    fresh.cursor = None;
    assert_eq!(fresh.snapshot["alpha"], 3);
    paged
        .transition(Event::Start {
            request: Box::new(fresh),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    let before_old_page = serde_json::to_vec(&paged).unwrap();
    assert_eq!(
        paged
            .transition(Event::Response {
                generation: 2,
                bytes: include_bytes!("second.json").to_vec()
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
    assert_eq!(serde_json::to_vec(&paged).unwrap(), before_old_page);
    paged
        .transition(Event::Response {
            generation: 3,
            bytes: include_bytes!("first.json").to_vec(),
        })
        .unwrap();
    assert_eq!(paged.rows().len(), 1);
    json!({"completed":saved,"detail":detail_result,"detail_start":detail_start,"stale_detail":stale_detail.change,"after_cancel":cancelled.change,"after_disconnect":after_disconnect,"after_reconnect":state.phase(),"generation":state.generation(),"stale":stale.change}).to_string()
}
