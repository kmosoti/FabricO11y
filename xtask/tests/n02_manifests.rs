//! Test-only prototype for #299; no production loader or benchmark runner.
use fabric::{observation, spindle};
use serde::Deserialize;

const RESOURCE: &str = include_str!("fixtures/n02/resource-profile.toml");
const BENCHMARK: &str = include_str!("fixtures/n02/benchmark-profile.toml");
const SLICE: &str = include_str!("../../config/fabrico11y.slice");
const TOOLCHAIN: &str = include_str!("../../rust-toolchain.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceProfile {
    version: u32,
    collector_process: Collector,
    slice_global: Slice,
    spindle_connection: Connection,
    spindle_fixture_process: Spindle,
    coverage: Coverage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Collector {
    rss_target_exclusive_bytes: usize,
    rss_shed_ceiling_bytes: usize,
    fd_target_exclusive: usize,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Slice {
    name: String,
    memory_high_bytes: usize,
    memory_max_bytes: usize,
    cpu_quota_percent: usize,
    io_weight: usize,
    tasks_max: usize,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    request_max_bytes: usize,
    reply_max_bytes: usize,
    frame_header_bytes: usize,
    parser_depth_max: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spindle {
    max_connections: usize,
    transport_buffers_max_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    not_qualified: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkProfile {
    version: u32,
    resource_profile: String,
    baseline: Baseline,
    machine: Machine,
    workload: Workload,
    thresholds: Thresholds,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    source_commit: String,
    rustc_commit: String,
    rustc_release: String,
    target: String,
    build_profile: String,
    fixture: String,
    required_run_identity: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Machine {
    arch: String,
    os: String,
    cgroup_version: u8,
    match_fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Workload {
    case: String,
    slots: usize,
    record_capacity_bytes: usize,
    producers: usize,
    warmups: usize,
    measured_repeats: usize,
    report: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Thresholds {
    sdk_reserved_payload_max_bytes: usize,
    accepted_records_lost_max: usize,
}

fn validate_envelopes(r: &ResourceProfile, b: &BenchmarkProfile) -> Result<(), &'static str> {
    if r.version != 1 || b.version != 1 || b.resource_profile != "resource-profile.toml" {
        return Err("manifest version or resource-profile reference");
    }
    let collector = &r.collector_process;
    let slice = &r.slice_global;
    if !(0 < collector.rss_target_exclusive_bytes
        && collector.rss_target_exclusive_bytes < collector.rss_shed_ceiling_bytes
        && collector.rss_shed_ceiling_bytes < slice.memory_high_bytes
        && slice.memory_high_bytes < slice.memory_max_bytes)
    {
        return Err("collector and slice memory threshold ordering");
    }
    let c = &r.spindle_connection;
    let spindle = &r.spindle_fixture_process;
    let transport = c
        .request_max_bytes
        .checked_add(c.reply_max_bytes)
        .and_then(|n| n.checked_add(c.frame_header_bytes))
        .and_then(|n| n.checked_mul(spindle.max_connections));
    if spindle.max_connections == 0 || transport != Some(spindle.transport_buffers_max_bytes) {
        return Err("aggregate transport buffer limit");
    }
    let work = &b.workload;
    if work.slots == 0
        || !(256..=c.request_max_bytes).contains(&work.record_capacity_bytes)
        || work.producers == 0
        || work.warmups == 0
        || work.measured_repeats < 2
        || work.slots.checked_mul(work.record_capacity_bytes)
            != Some(b.thresholds.sdk_reserved_payload_max_bytes)
        || b.thresholds.accepted_records_lost_max != 0
    {
        return Err("workload or predeclared correctness threshold");
    }
    Ok(())
}

fn manifests() -> (ResourceProfile, BenchmarkProfile) {
    (
        toml::from_str(RESOURCE).unwrap(),
        toml::from_str(BENCHMARK).unwrap(),
    )
}

#[test]
fn declaration_matches_existing_resource_and_workload_inputs() {
    let (r, b) = manifests();
    assert_eq!(validate_envelopes(&r, &b), Ok(()));
    let collector = &r.collector_process;
    assert_eq!(collector.source, "AGENTS.md");
    assert_eq!(collector.rss_target_exclusive_bytes, 100 * 1024 * 1024);
    assert_eq!(collector.rss_shed_ceiling_bytes, 200 * 1024 * 1024);
    assert_eq!(collector.fd_target_exclusive, 64);
    assert_eq!(r.slice_global.name, "fabrico11y.slice");
    assert_eq!(r.slice_global.source, "config/fabrico11y.slice");
    let mib = |n: usize| {
        assert_eq!(n % (1024 * 1024), 0);
        n / (1024 * 1024)
    };
    for expected in [
        format!("MemoryHigh={}M", mib(r.slice_global.memory_high_bytes)),
        format!("MemoryMax={}M", mib(r.slice_global.memory_max_bytes)),
        format!("CPUQuota={}%", r.slice_global.cpu_quota_percent),
        format!("IOWeight={}", r.slice_global.io_weight),
        format!("TasksMax={}", r.slice_global.tasks_max),
    ] {
        assert!(SLICE.lines().any(|line| line == expected), "{expected}");
    }
    assert_eq!(
        r.spindle_connection.request_max_bytes,
        observation::MAX_FRAME
    );
    assert_eq!(r.spindle_connection.reply_max_bytes, spindle::MAX_REPLY);
    assert_eq!(r.spindle_connection.frame_header_bytes, size_of::<u32>());
    assert_eq!(
        r.spindle_connection.parser_depth_max,
        observation::MAX_DEPTH
    );
    let root = xtask::evidence::non_tmp_tempdir().unwrap();
    let fixture = xtask::collection_lab::config(root.path());
    assert_eq!(
        r.spindle_fixture_process.max_connections,
        fixture.max_connections
    );
    assert_eq!(
        r.spindle_fixture_process.transport_buffers_max_bytes,
        8 * (65_536 + 73_728 + 4),
        "fixture aggregate must equal its declared per-connection envelope"
    );

    assert_eq!(b.baseline.fixture, "xtask/src/bin/collection-resources.rs");
    assert_eq!(
        (
            b.workload.case.as_str(),
            b.workload.slots,
            b.workload.record_capacity_bytes,
            b.workload.producers
        ),
        ("full-width", 8, 4096, 4),
    );
    for commit in [&b.baseline.source_commit, &b.baseline.rustc_commit] {
        assert_eq!(commit.len(), 40);
        assert!(commit.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    let pin = format!("channel = \"{}\"", b.baseline.rustc_release);
    assert!(TOOLCHAIN.lines().any(|line| line == pin));
    assert_eq!(b.baseline.target, "x86_64-unknown-linux-gnu");
    assert_eq!(b.baseline.build_profile, "release");
    assert_eq!(
        (
            b.machine.arch.as_str(),
            b.machine.os.as_str(),
            b.machine.cgroup_version
        ),
        ("x86_64", "linux", 2)
    );
    assert_eq!(b.baseline.required_run_identity.len(), 6);
    assert_eq!(b.machine.match_fields.len(), 8);
    assert_eq!(b.workload.report.len(), 11);
    assert_eq!(r.coverage.not_qualified.len(), 9);
}

#[test]
fn rejects_scope_inversion_and_inconsistent_aggregate() {
    let (mut r, b) = manifests();
    r.slice_global.memory_high_bytes = r.collector_process.rss_shed_ceiling_bytes;
    assert_eq!(
        validate_envelopes(&r, &b),
        Err("collector and slice memory threshold ordering")
    );
    let (mut r, b) = manifests();
    r.spindle_fixture_process.transport_buffers_max_bytes -= 1;
    assert_eq!(
        validate_envelopes(&r, &b),
        Err("aggregate transport buffer limit")
    );
    r.spindle_connection.request_max_bytes = usize::MAX;
    assert_eq!(
        validate_envelopes(&r, &b),
        Err("aggregate transport buffer limit")
    );
}

#[test]
fn rejects_workload_that_does_not_fit_its_declared_budget() {
    let (r, mut b) = manifests();
    b.workload.slots += 1;
    assert_eq!(
        validate_envelopes(&r, &b),
        Err("workload or predeclared correctness threshold")
    );
}

#[test]
fn rejects_run_observations_inside_a_declaration() {
    let contaminated = BENCHMARK.replacen(
        "[thresholds]",
        "[observed]\npeak_rss_bytes = 1\n\n[thresholds]",
        1,
    );
    let err = toml::from_str::<BenchmarkProfile>(&contaminated)
        .err()
        .unwrap();
    assert!(err.to_string().contains("unknown field `observed`"));
    let contaminated = BENCHMARK.replacen("[baseline]", "[baseline]\nreceipt_passed = true", 1);
    let err = toml::from_str::<BenchmarkProfile>(&contaminated)
        .err()
        .unwrap();
    assert!(err.to_string().contains("unknown field `receipt_passed`"));
}

#[test]
fn declared_parser_boundary_matches_runtime_refusal() {
    let (r, _) = manifests();
    let c = r.spindle_connection;
    let at_depth = format!(
        "{}0{}",
        "[".repeat(c.parser_depth_max),
        "]".repeat(c.parser_depth_max)
    );
    assert!(observation::check_json(at_depth.as_bytes(), c.request_max_bytes).is_ok());
    let past_depth = format!("[{at_depth}]");
    assert_eq!(
        observation::check_json(past_depth.as_bytes(), c.request_max_bytes),
        Err("nesting limit".into())
    );
    let oversized = vec![b' '; c.request_max_bytes + 1];
    assert_eq!(
        observation::check_json(&oversized, c.request_max_bytes),
        Err("frame too large".into())
    );
}
