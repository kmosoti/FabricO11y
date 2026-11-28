use fabric_app_core::{Change, Error, Event, MAX_PAGES, Phase, State};
use fabric_query_contract::{Operation, QueryRequest, RunKey};
use serde_json::{Value, json};
fn request() -> QueryRequest {
    serde_json::from_str(include_str!("../../fixtures/request.json")).unwrap()
}
fn page() -> Value {
    serde_json::from_str(include_str!("../../fixtures/first.json")).unwrap()
}
fn start() -> State {
    let mut state = State::default();
    state
        .transition(Event::Start {
            request: Box::new(request()),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    state
}
fn encoded(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn unchanged(state: &mut State, event: Event, error: Error) {
    let before = serde_json::to_vec(state).unwrap();
    assert_eq!(state.transition(event).unwrap_err(), error);
    assert_eq!(serde_json::to_vec(state).unwrap(), before);
}
#[test]
fn authority_scope_snapshot_and_decode_fail_without_mutation() {
    let mut state = start();
    for (field, value) in [
        ("canonical_authority", json!("other")),
        ("scope", json!(["other"])),
        ("snapshot", json!({"alpha":"4","other":"1"})),
        ("transform", json!("other")),
    ] {
        let mut response = page();
        response["coverage"][field] = value;
        unchanged(
            &mut state,
            Event::Response {
                generation: 1,
                bytes: encoded(&response),
            },
            Error::InvalidResponse,
        );
    }
    let mut foreign = page();
    foreign["coverage"]["projection"]["frontiers"]["other"] = json!("1");
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&foreign),
        },
        Error::InvalidResponse,
    );
    let mut bad_run = page();
    bad_run["rows"][0]["run"]["run_id"] = json!("00000000-0000-0000-0000-000000000000");
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&bad_run),
        },
        Error::InvalidResponse,
    );
    let mut response = page();
    response["rows"][0]["count"] = json!(17);
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&response),
        },
        Error::InvalidResponse,
    );
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: vec![b' '; 4097],
        },
        Error::ResponseTooLarge,
    );
    let mut too_many = page();
    let row = too_many["rows"][0].clone();
    too_many["rows"] = json!([row.clone(), row.clone(), row]);
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&too_many),
        },
        Error::RowLimit,
    );
    assert_eq!(state.phase(), Phase::Loading);
    assert_eq!(
        state
            .transition(Event::Response {
                generation: 99,
                bytes: vec![0; 300_000]
            })
            .unwrap()
            .change,
        Change::Unchanged
    );
}
#[test]
fn page_snapshot_and_duplicate_rows_are_rejected_without_mutation() {
    let mut state = start();
    state
        .transition(Event::Response {
            generation: 1,
            bytes: encoded(&page()),
        })
        .unwrap();
    state.transition(Event::Next).unwrap();
    let mut wrong = page();
    wrong["coverage"]["snapshot"]["alpha"] = json!("4");
    unchanged(
        &mut state,
        Event::Response {
            generation: 2,
            bytes: encoded(&wrong),
        },
        Error::InvalidResponse,
    );
    unchanged(
        &mut state,
        Event::Response {
            generation: 2,
            bytes: encoded(&page()),
        },
        Error::InvalidResponse,
    );
}
#[test]
fn page_count_and_generation_are_bounded() {
    let mut state = start();
    for index in 0..MAX_PAGES {
        let mut response = page();
        response["rows"][0]["run"]["run_id"] =
            json!(format!("00000000-0000-4000-8000-{:012}", index + 1));
        state
            .transition(Event::Response {
                generation: (index + 1) as u32,
                bytes: encoded(&response),
            })
            .unwrap();
        state.transition(Event::Next).unwrap();
    }
    let mut response = page();
    response["rows"][0]["run"]["run_id"] = json!("00000000-0000-4000-8000-000000000099");
    unchanged(
        &mut state,
        Event::Response {
            generation: (MAX_PAGES + 1) as u32,
            bytes: encoded(&response),
        },
        Error::RowLimit,
    );
    assert_eq!(state.coverages().len(), MAX_PAGES);
}
#[test]
fn invalid_authority_and_duplicate_scope_do_not_start() {
    let mut state = State::default();
    unchanged(
        &mut state,
        Event::Start {
            request: Box::new(request()),
            expected_authority: "other".into(),
        },
        Error::InvalidRequest,
    );
    let mut duplicate = request();
    duplicate.scope.push("alpha".into());
    unchanged(
        &mut state,
        Event::Start {
            request: Box::new(duplicate),
            expected_authority: "auth-1".into(),
        },
        Error::InvalidRequest,
    );
}

#[test]
fn run_summary_requires_a_matching_run_and_rejects_foreign_rows() {
    let mut state = State::default();
    let mut detail = request();
    detail.operation = Operation::RunSummary;
    unchanged(
        &mut state,
        Event::Start {
            request: Box::new(detail.clone()),
            expected_authority: "auth-1".into(),
        },
        Error::InvalidRequest,
    );
    detail.filter.run = Some(RunKey::Global {
        run_id: "00000000-0000-4000-8000-000000000002".into(),
    });
    state
        .transition(Event::Start {
            request: Box::new(detail),
            expected_authority: "auth-1".into(),
        })
        .unwrap();
    let mut response = page();
    response["coverage"]["complete"] = json!(true);
    response["coverage"]["truncated"] = json!(false);
    response["next_cursor"] = Value::Null;
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&response),
        },
        Error::InvalidResponse,
    );
    let row = response["rows"][0].clone();
    response["rows"] = json!([row.clone(), row]);
    unchanged(
        &mut state,
        Event::Response {
            generation: 1,
            bytes: encoded(&response),
        },
        Error::RowLimit,
    );
}
