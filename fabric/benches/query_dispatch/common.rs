use fabric::query::{
    self, Budgets, ConsumedBudget, Coverage, Filter, Operation, ProjectionProgress, QueryRequest,
    QueryResponse,
};
use sha2::{Digest, Sha256};
use std::{hint::black_box, time::Instant};

fn request(operation: Operation) -> QueryRequest {
    QueryRequest {
        version: query::API_VERSION,
        query_id: "bench".into(),
        authority: None,
        scope: vec!["fixture".into()],
        snapshot: Default::default(),
        filter: Filter::default(),
        operation,
        budgets: Budgets::default(),
        cursor: None,
    }
}
fn handle(request: &QueryRequest, tag: u64) -> QueryResponse {
    let n = match request.operation {
        Operation::Activity { bucket_ns } => bucket_ns,
        _ => 0,
    };
    QueryResponse {
        version: request.version,
        rows: vec![serde_json::json!({"operation": tag, "bucket": n})],
        coverage: Coverage {
            canonical_authority: "fixture".into(),
            scope: vec!["fixture".into()],
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
trait Handler {
    fn call(&self, request: &QueryRequest) -> QueryResponse;
}
struct H<const N: u64>;
impl<const N: u64> Handler for H<N> {
    #[inline(never)]
    fn call(&self, request: &QueryRequest) -> QueryResponse {
        handle(request, N)
    }
}
#[derive(Clone, Copy)]
enum Choice {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}
impl Handler for Choice {
    #[inline(never)]
    fn call(&self, r: &QueryRequest) -> QueryResponse {
        match self {
            Self::A => H::<0>.call(r),
            Self::B => H::<1>.call(r),
            Self::C => H::<2>.call(r),
            Self::D => H::<3>.call(r),
            Self::E => H::<4>.call(r),
            Self::F => H::<5>.call(r),
            Self::G => H::<6>.call(r),
        }
    }
}
#[inline(never)]
fn static_dispatch(i: usize, r: &QueryRequest) -> QueryResponse {
    match i {
        0 => H::<0>.call(r),
        1 => H::<1>.call(r),
        2 => H::<2>.call(r),
        3 => H::<3>.call(r),
        4 => H::<4>.call(r),
        5 => H::<5>.call(r),
        6 => H::<6>.call(r),
        _ => unreachable!(),
    }
}
#[inline(never)]
fn dyn_dispatch(h: &dyn Handler, r: &QueryRequest) -> QueryResponse {
    h.call(r)
}
fn dispatch(
    mode: &str,
    i: usize,
    r: &QueryRequest,
    choices: &[Choice],
    dynamic: &[&dyn Handler],
) -> QueryResponse {
    match mode {
        "static" => static_dispatch(black_box(i), r),
        "enum" => choices[black_box(i)].call(r),
        "dyn" => dyn_dispatch(dynamic[black_box(i)], r),
        _ => panic!("invalid mode"),
    }
}
fn digest(responses: &[QueryResponse]) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(responses).unwrap())
    )
}
pub fn run(mode: &str) {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let corrupt = args.iter().any(|a| a == "--corrupt");
    let iterations: usize = std::env::var("FABRIC_DISPATCH_ITERATIONS")
        .unwrap_or_else(|_| "100000".into())
        .parse()
        .unwrap();
    assert!((1..=10_000_000).contains(&iterations));
    let requests = [
        Operation::Capabilities,
        Operation::ListRuns,
        Operation::RunSummary,
        Operation::ToolSummary,
        Operation::ServiceSummary,
        Operation::Activity { bucket_ns: 17 },
        Operation::Evidence,
    ]
    .into_iter()
    .map(request)
    .collect::<Vec<_>>();
    let choices = [
        Choice::A,
        Choice::B,
        Choice::C,
        Choice::D,
        Choice::E,
        Choice::F,
        Choice::G,
    ];
    let dynamic: [&dyn Handler; 7] = [
        &H::<0>, &H::<1>, &H::<2>, &H::<3>, &H::<4>, &H::<5>, &H::<6>,
    ];
    let expected: Vec<_> = requests
        .iter()
        .enumerate()
        .map(|(i, r)| handle(r, i as u64))
        .collect();
    let mut actual: Vec<_> = requests
        .iter()
        .enumerate()
        .map(|(i, r)| dispatch(mode, i, r, &choices, &dynamic))
        .collect();
    if corrupt {
        actual[5].rows[0]["operation"] = serde_json::json!(99);
    }
    let expected_hash = digest(&expected);
    let actual_hash = digest(&actual);
    assert_eq!(actual_hash, expected_hash, "dispatch outputs differ");
    let start = Instant::now();
    let mut checksum = 0u64;
    for n in 0..iterations {
        let i = n % requests.len();
        let output = dispatch(mode, i, black_box(&requests[i]), &choices, &dynamic);
        checksum = checksum.wrapping_add(black_box(output.rows[0]["operation"].as_u64().unwrap()));
    }
    let elapsed = start.elapsed().as_nanos();
    println!(
        "{}",
        serde_json::json!({"mode": mode, "iterations": iterations, "elapsed_ns": elapsed, "checksum": checksum, "output_sha256": actual_hash})
    );
}
