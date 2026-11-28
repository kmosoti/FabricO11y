use super::evidence;
use std::{
    fs,
    io::{Read, Write},
    os::fd::AsFd,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

fn counters_complete(counters: &str) -> bool {
    ["task-clock", "cycles", "instructions"]
        .iter()
        .all(|event| {
            counters
                .lines()
                .filter(|line| !line.starts_with('#'))
                .any(|line| {
                    let fields: Vec<_> = line.split(',').collect();
                    fields.len() >= 3
                        && fields[2].split(':').next() == Some(*event)
                        && fields[0]
                            .parse::<f64>()
                            .is_ok_and(|value| value.is_finite() && value >= 0.0)
                })
        })
}

fn benchmark_complete(stdout: &[u8]) -> bool {
    let records: Vec<serde_json::Value> = String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let expected = records
        .iter()
        .find(|r| r["event"] == "configuration")
        .and_then(|r| r["expected_samples"].as_u64())
        .unwrap_or(0);
    let measurements: Vec<_> = records
        .iter()
        .filter(|r| r["event"] == "measurement")
        .collect();
    expected > 0
        && measurements.len() as u64 == expected
        && measurements.iter().all(|r| {
            r["records"].as_u64().is_some_and(|n| n > 0)
                && r["elapsed_ns"].as_u64().is_some_and(|n| n > 0)
        })
}

pub fn cli(args: &[&str], root: &Path) -> Result<(), String> {
    if args.first() == Some(&"collection") {
        return collection(args, root);
    }
    let [mode @ ("permissions" | "benchmark"), directory] = args else {
        return Err("profile permissions|benchmark ABSOLUTE_ARTIFACT_DIRECTORY | profile collection ABSOLUTE_ARTIFACT_DIRECTORY ABSOLUTE_RUNNER".into());
    };
    let directory = Path::new(directory);
    if !directory.is_absolute() || directory.exists() {
        return Err("use a new absolute artifact directory".into());
    }
    let directory = evidence::artifact_path(directory, root, true)?;
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let capture = super::capture(root)?;
    let candidate = capture.lines().next().ok_or("empty source capture")?;
    let receipt = fs::read_to_string(root.join(".local/last-verification")).unwrap_or_default();
    let native_acceptance = receipt
        .lines()
        .any(|line| line == format!("candidate={candidate}"))
        && receipt.lines().any(|line| line == "result=success");
    if *mode == "benchmark" && !native_acceptance {
        // Keep refusal evidence bounded: neither receipt text nor capture paths
        // are copied into the result. Compare rejects unsuccessful results.
        if !matches!(candidate.len(), 40 | 64)
            || !candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid candidate identity in source capture".into());
        }
        evidence::write(
            &directory.join("result.json"),
            &serde_json::json!({
                "mode": mode,
                "candidate": candidate,
                "capture_sha256": evidence::hash_bytes(capture.as_bytes())?,
                "reason": "native_verification_missing_or_mismatched",
                "success": false,
                "refused": true,
                "executed": false,
                "native_acceptance": false,
            }),
        )?;
        return Err(
            "benchmark requires candidate capture/verify acceptance for the current source".into(),
        );
    }
    let version = super::output("perf", &["--version"], root)?;
    let budget: u64 = std::env::var("FABRIC_PROFILE_SECONDS")
        .map(|s| s.parse().map_err(|_| "invalid FABRIC_PROFILE_SECONDS"))
        .unwrap_or(Ok(if *mode == "benchmark" { 900 } else { 30 }))?;
    let build_jobs = super::fuzz::env_positive_u64("CARGO_BUILD_JOBS", 4)?;
    let mut executable_sha256 = None;
    let mut driver_sha256 = None;
    let mut cases_sha256 = None;
    let mut evaluator_policy_sha256 = None;
    let mut build_configuration = serde_json::Value::Null;
    let compiler = super::output("rustc", &["--version", "--verbose"], root)?;
    let mut command = Command::new("timeout");
    command.arg(budget.to_string()).arg("perf");
    command
        .args([
            "stat",
            "-x",
            ",",
            "-e",
            "task-clock,cycles,instructions",
            "-o",
        ])
        .arg(directory.join("perf.csv"))
        .arg("--");
    if *mode == "permissions" {
        command.arg("/usr/bin/true");
    } else {
        let build = Command::new("cargo")
            .args([
                "bench",
                "--package",
                "fabric",
                "--bench",
                "admission",
                "--no-run",
                "--message-format=json",
                "--locked",
            ])
            .env("CARGO_BUILD_JOBS", build_jobs.to_string())
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        fs::write(directory.join("build.stdout"), &build.stdout).map_err(|e| e.to_string())?;
        fs::write(directory.join("build.stderr"), &build.stderr).map_err(|e| e.to_string())?;
        if !build.status.success() {
            return Err(format!("benchmark compilation failed: {}", build.status));
        }
        let executable = String::from_utf8_lossy(&build.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|r| {
                if r["reason"] == "compiler-artifact" && r["target"]["name"] == "admission" {
                    r["executable"].as_str().map(std::path::PathBuf::from)
                } else {
                    None
                }
            })
            .ok_or("Cargo did not produce the admission benchmark executable")?;
        let frozen = directory.join("benchmark-bin");
        fs::copy(executable, &frozen).map_err(|e| e.to_string())?;
        executable_sha256 = Some(evidence::hash(&frozen)?);
        driver_sha256 = Some(evidence::hash(&root.join("fabric/benches/admission.rs"))?);
        if let Some(path) = std::env::var_os("FABRIC_BENCH_CASES_FILE") {
            let owned_cases = directory.join("cases.json");
            fs::copy(path, &owned_cases).map_err(|e| e.to_string())?;
            cases_sha256 = Some(evidence::hash(&owned_cases)?);
            command.env("FABRIC_BENCH_CASES_FILE", owned_cases);
        }
        let mut policy = Vec::new();
        for file in [
            "xtask/src/main.rs",
            "xtask/src/profile.rs",
            "xtask/src/benchmark.rs",
            "xtask/src/fuzz.rs",
            "xtask/src/proof.rs",
            "xtask/src/extraction.rs",
            "xtask/src/architecture.rs",
            "xtask/src/evidence.rs",
            "verification/architecture.toml",
            "proofs/ExtractedAdmission.lean",
            "fabric/src/admission/tests.rs",
        ] {
            policy.push((file, evidence::hash(&root.join(file))?));
        }
        evaluator_policy_sha256 = Some(evidence::hash_bytes(
            &serde_json::to_vec(&policy).map_err(|e| e.to_string())?,
        )?);
        let settings: std::collections::BTreeMap<_, _> = [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_TARGET",
            "CARGO_PROFILE_BENCH_OPT_LEVEL",
            "CARGO_PROFILE_BENCH_LTO",
            "CARGO_PROFILE_BENCH_OVERFLOW_CHECKS",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL",
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS",
            "FABRIC_BENCH_RECORDS",
            "FABRIC_BENCH_REPEATS",
            "FABRIC_BENCH_SECONDS",
        ]
        .into_iter()
        .map(|key| (key, std::env::var(key).ok()))
        .collect();
        build_configuration = serde_json::json!({"profile":"bench", "jobs": build_jobs,
            "environment_sha256":evidence::hash_bytes(&serde_json::to_vec(&settings).map_err(|e|e.to_string())?)?});
        command.arg(frozen);
    }
    let output = command
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let counters = fs::read_to_string(directory.join("perf.csv")).unwrap_or_default();
    let available = counters_complete(&counters);
    let measured = *mode != "benchmark" || benchmark_complete(&output.stdout);
    let complete = available && measured;
    let unchanged = super::capture(root)? == capture;
    fs::write(directory.join("stdout"), &output.stdout).map_err(|e| e.to_string())?;
    fs::write(directory.join("stderr"), &output.stderr).map_err(|e| e.to_string())?;
    evidence::write(
        &directory.join("result.json"),
        &serde_json::json!({
            "mode": mode, "perf": version, "repository_capture": capture,
        "exit": output.status.code(), "success": output.status.success() && complete && unchanged,
        "source_unchanged": unchanged,
        "all_counters_available": available, "measurements_complete": measured,
        "native_acceptance": native_acceptance, "driver_sha256":driver_sha256,
        "acceptance_receipt":receipt,
        "executable_sha256":executable_sha256, "cases_sha256":cases_sha256, "compiler":compiler,
        "evaluator_policy_sha256":evaluator_policy_sha256,
        "build_configuration":build_configuration,
        "contract_sha256":if *mode == "benchmark" { Some(evidence::hash(&root.join("verification/admission-contract.md"))?) } else { None },
            "measurement_boundary": "perf covers the benchmark process including warmup, fixtures, construction, retries and reporting; compilation excluded; stdout separates warmed operation samples and construction",
            "counters": "task-clock,cycles,instructions", "build_jobs": build_jobs,
            "time_budget_seconds": budget,
            "os": super::output("uname", &["-a"], root)?,
            "hardware": super::output("lscpu", &[], root)?,
            "hardware_topology": super::output("lscpu", &["-p=CPU,CORE,SOCKET,ONLINE"], root)?,
        }),
    )?;
    if output.status.success() && complete && unchanged {
        Ok(())
    } else {
        Err(format!(
            "perf/benchmark unavailable or failed ({}); inspect {}; host security policy was not changed",
            output.status,
            directory.display()
        ))
    }
}

const COLLECTION_COMMAND: &str = "systemd-run";
const MAX_ACCEPTANCE_RECEIPT_BYTES: u64 = 64 * 1024;
const MAX_COLLECTION_STREAM_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION_READ_PER_TICK: usize = 64 * 1024;
const MAX_COLLECTION_CLIENT_WAIT: Duration = Duration::from_secs(145);

struct BoundedStream {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
    eof: bool,
    read_error: Option<String>,
}

impl BoundedStream {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAX_COLLECTION_STREAM_BYTES.min(64 * 1024)),
            total_bytes: 0,
            truncated: false,
            eof: false,
            read_error: None,
        }
    }
}

fn nonblocking(fd: &impl AsFd) -> Result<(), String> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|error| error.to_string())?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| error.to_string())
}

fn retain_bytes(stream: &mut BoundedStream, input: &[u8], limit: usize) {
    let room = limit.saturating_sub(stream.bytes.len());
    stream
        .bytes
        .extend_from_slice(&input[..input.len().min(room)]);
    stream.total_bytes = stream.total_bytes.saturating_add(input.len() as u64);
    stream.truncated = stream.total_bytes > stream.bytes.len() as u64;
}

fn read_available(reader: &mut impl Read, stream: &mut BoundedStream) {
    if stream.eof || stream.read_error.is_some() {
        return;
    }
    let mut chunk = [0u8; 8192];
    let mut budget = MAX_COLLECTION_READ_PER_TICK;
    while budget > 0 {
        let limit = budget.min(chunk.len());
        match reader.read(&mut chunk[..limit]) {
            Ok(0) => {
                stream.eof = true;
                break;
            }
            Ok(count) => {
                retain_bytes(stream, &chunk[..count], MAX_COLLECTION_STREAM_BYTES);
                budget -= count;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                break;
            }
            Err(error) => {
                stream.read_error = Some(error.to_string());
                stream.eof = true;
                break;
            }
        }
    }
}

struct OwnedCommand(Child);

impl Drop for OwnedCommand {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct BoundedOutput {
    status: Option<ExitStatus>,
    stdout: BoundedStream,
    stderr: BoundedStream,
    timed_out: bool,
    run_error: Option<String>,
}

fn run_bounded(command: &mut Command) -> Result<BoundedOutput, String> {
    let mut child = OwnedCommand(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?,
    );
    let Some(mut stdout_reader) = child.0.stdout.take() else {
        return Err("collection command stdout pipe unavailable".into());
    };
    let Some(mut stderr_reader) = child.0.stderr.take() else {
        return Err("collection command stderr pipe unavailable".into());
    };
    nonblocking(&stdout_reader)?;
    nonblocking(&stderr_reader)?;
    let mut stdout = BoundedStream::new();
    let mut stderr = BoundedStream::new();
    let deadline = Instant::now() + MAX_COLLECTION_CLIENT_WAIT;
    let mut status: Option<ExitStatus> = None;
    let mut timed_out = false;
    let mut run_error = None;
    loop {
        read_available(&mut stdout_reader, &mut stdout);
        read_available(&mut stderr_reader, &mut stderr);
        if status.is_none() {
            match child.0.try_wait() {
                Ok(result) => status = result,
                Err(error) => {
                    drop(stdout_reader);
                    drop(stderr_reader);
                    let _ = child.0.kill();
                    status = child.0.wait().ok();
                    stdout.truncated |= !stdout.eof;
                    stderr.truncated |= !stderr.eof;
                    timed_out = true;
                    run_error = Some(error.to_string());
                    break;
                }
            }
        }
        if status.is_some() && stdout.eof && stderr.eof {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            stdout.truncated |= !stdout.eof;
            stderr.truncated |= !stderr.eof;
            drop(stdout_reader);
            drop(stderr_reader);
            let _ = child.0.kill();
            if status.is_none() {
                match child.0.wait() {
                    Ok(exit) => status = Some(exit),
                    Err(error) => run_error = Some(error.to_string()),
                }
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
        timed_out,
        run_error,
    })
}

fn disk_backed_directory(path: &Path) -> Result<PathBuf, String> {
    if ["/tmp", "/dev/shm", "/run/shm"]
        .iter()
        .any(|prefix| path.starts_with(Path::new(prefix)))
    {
        return Err("profile data path uses a forbidden temporary directory".into());
    }
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if ["/tmp", "/dev/shm", "/run/shm"]
        .iter()
        .any(|prefix| path.starts_with(Path::new(prefix)))
    {
        return Err("profile data directory resolves to a RAM-backed temporary path".into());
    }
    let output = Command::new("findmnt")
        .args(["--noheadings", "--output", "FSTYPE", "--target"])
        .arg(&path)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("cannot determine profile data filesystem type".into());
    }
    let filesystem = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if matches!(filesystem.as_str(), "tmpfs" | "ramfs" | "hugetlbfs") {
        return Err(format!(
            "profile data directory uses RAM-backed {filesystem}"
        ));
    }
    Ok(path)
}

fn collection_tmpdir() -> Result<PathBuf, String> {
    let data = disk_backed_directory(Path::new("/data"))?;
    let tmp = data.join("tmp");
    fs::create_dir_all(&tmp).map_err(|error| error.to_string())?;
    disk_backed_directory(&tmp)
}

fn write_collection_result(directory: &Path, value: &serde_json::Value) -> Result<(), String> {
    let destination = directory.join("result.json");
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    temporary
        .write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(destination)
        .map_err(|error| error.error.to_string())?;
    Ok(())
}

struct CollectionPlan {
    case: String,
    slots: u64,
    record_capacity_bytes: u64,
    producers: u64,
    warmups: u64,
    measured_repeats: u64,
}

fn collection_plan(
    root: &Path,
) -> Result<(CollectionPlan, serde_json::Value, serde_json::Value), String> {
    let benchmark: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("xtask/tests/fixtures/n02/benchmark-profile.toml"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|_| "refused: invalid N02 benchmark profile")?;
    let resources: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("xtask/tests/fixtures/n02/resource-profile.toml"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|_| "refused: invalid N02 resource profile")?;
    let workload = benchmark
        .get("workload")
        .and_then(toml::Value::as_table)
        .ok_or("refused: N02 workload table missing")?;
    let value = |key: &str| -> Result<u64, String> {
        workload
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| format!("refused: invalid N02 workload field {key}"))
    };
    let case = workload
        .get("case")
        .and_then(toml::Value::as_str)
        .ok_or("refused: N02 workload case missing")?
        .to_owned();
    let plan = CollectionPlan {
        case,
        slots: value("slots")?,
        record_capacity_bytes: value("record_capacity_bytes")?,
        producers: value("producers")?,
        warmups: value("warmups")?,
        measured_repeats: value("measured_repeats")?,
    };
    let fixture = xtask::collection_lab::RESOURCE_WORKLOADS
        .iter()
        .find(|workload| workload.case == plan.case)
        .ok_or("refused: N02 case has no executable fixture")?;
    if plan.case != "full-width"
        || plan.slots != fixture.slots as u64
        || plan.record_capacity_bytes != fixture.record_capacity_bytes as u64
        || plan.producers != fixture.producers as u64
        || plan.warmups != 1
        || plan.measured_repeats != 5
        || benchmark.get("version").and_then(toml::Value::as_integer) != Some(1)
        || benchmark
            .get("resource_profile")
            .and_then(toml::Value::as_str)
            != Some("resource-profile.toml")
        || benchmark
            .get("baseline")
            .and_then(|value| value.get("fixture"))
            .and_then(toml::Value::as_str)
            != Some("xtask/src/bin/collection-resources.rs")
    {
        return Err("refused: N02 workload policy drift".into());
    }
    let slice = resources
        .get("slice_global")
        .and_then(toml::Value::as_table)
        .ok_or("refused: N02 slice policy missing")?;
    let exact_slice = slice.get("name").and_then(toml::Value::as_str) == Some("fabrico11y.slice")
        && slice
            .get("memory_high_bytes")
            .and_then(toml::Value::as_integer)
            == Some(268_435_456)
        && slice
            .get("memory_max_bytes")
            .and_then(toml::Value::as_integer)
            == Some(536_870_912)
        && slice
            .get("cpu_quota_percent")
            .and_then(toml::Value::as_integer)
            == Some(15)
        && slice.get("io_weight").and_then(toml::Value::as_integer) == Some(30)
        && slice.get("tasks_max").and_then(toml::Value::as_integer) == Some(64);
    if !exact_slice {
        return Err("refused: N02 slice resource policy drift".into());
    }
    if resources.get("version").and_then(toml::Value::as_integer) != Some(1) {
        return Err("refused: N02 resource profile version drift".into());
    }
    Ok((
        plan,
        serde_json::to_value(benchmark).map_err(|e| e.to_string())?,
        serde_json::to_value(resources).map_err(|e| e.to_string())?,
    ))
}

fn read_bounded_receipt(path: &Path) -> Result<(Vec<u8>, bool), String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false));
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_ACCEPTANCE_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() as u64 > MAX_ACCEPTANCE_RECEIPT_BYTES;
    if truncated {
        bytes.truncate(MAX_ACCEPTANCE_RECEIPT_BYTES as usize);
    }
    Ok((bytes, truncated))
}

fn collection_output_complete(stdout: &[u8], plan: &CollectionPlan) -> bool {
    let Ok(report) = serde_json::from_slice::<serde_json::Value>(stdout) else {
        return false;
    };
    report["passed"] == true
        && reading_available(&report["harness_lifetime_peak_rss_bytes"])
        && reading_available(&report["visible_cgroup_lifetime_peak_bytes"])
        && report["workloads"].as_array().is_some_and(|workloads| {
            workloads.len() == 1
                && workloads[0]["case"] == plan.case
                && workloads[0]["slots"].as_u64() == Some(plan.slots)
                && workloads[0]["record_capacity"].as_u64() == Some(plan.record_capacity_bytes)
                && workloads[0]["producers"].as_u64() == Some(plan.producers)
                && [
                    "rss_before",
                    "rss_at_full",
                    "collector_rss_with_slow_clients",
                    "daemon_lifetime_peak_rss_bytes",
                ]
                .iter()
                .all(|key| reading_available(&workloads[0][*key]))
        })
}

fn reading_available(reading: &serde_json::Value) -> bool {
    reading["state"] == "available" && reading["bytes"].as_u64().is_some()
}

fn collection_disposition(
    exit_success: bool,
    stdout: &[u8],
    plan: &CollectionPlan,
) -> &'static str {
    if exit_success && collection_output_complete(stdout, plan) {
        "completed"
    } else if serde_json::from_slice::<serde_json::Value>(stdout)
        .is_ok_and(|report| report["refused"] == true || report["disposition"] == "refused")
    {
        "refused"
    } else {
        "failed"
    }
}

fn collection_sample_metrics(stdout: &[u8]) -> Option<std::collections::BTreeMap<String, u64>> {
    let report: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let workload = report["workloads"].as_array()?.first()?.as_object()?;
    let mut metrics = std::collections::BTreeMap::new();
    for (name, value) in workload {
        if let Some(value) = value.as_u64() {
            metrics.insert(format!("workload.{name}"), value);
        }
    }
    for name in [
        "harness_lifetime_peak_rss_bytes",
        "visible_cgroup_lifetime_peak_bytes",
    ] {
        if let Some(value) = report[name]["bytes"].as_u64() {
            metrics.insert(format!("runner.{name}"), value);
        }
    }
    Some(metrics)
}

fn collection_metric_summary(
    repetitions: &[serde_json::Value],
    directory: &Path,
) -> Result<serde_json::Value, String> {
    let mut samples: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
    for repetition in repetitions {
        if repetition["phase"] != "measured" || repetition["disposition"] != "completed" {
            continue;
        }
        let path = repetition["stdout"]
            .as_str()
            .ok_or("missing repetition stdout path")?;
        let output = fs::read(directory.join(path)).map_err(|e| e.to_string())?;
        if let Some(metrics) = collection_sample_metrics(&output) {
            for (name, value) in metrics {
                samples.entry(name).or_default().push(value);
            }
        }
    }
    let summary: serde_json::Map<String, serde_json::Value> = samples
        .into_iter()
        .map(|(name, values)| {
            let numeric: Vec<f64> = values.iter().map(|value| *value as f64).collect();
            let mean = numeric.iter().sum::<f64>() / numeric.len() as f64;
            let variance = if numeric.len() > 1 {
                Some(
                    numeric
                        .iter()
                        .map(|value| (value - mean).powi(2))
                        .sum::<f64>()
                        / (numeric.len() - 1) as f64,
                )
            } else {
                None
            };
            (
                name,
                serde_json::json!({
                    "samples": values,
                    "sample_count": numeric.len(),
                    "mean": mean,
                    "sample_variance": variance,
                }),
            )
        })
        .collect();
    Ok(serde_json::Value::Object(summary))
}

fn machine_profile(root: &Path) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "uname": super::output("uname", &["-a"], root)?,
        "lscpu": super::output("lscpu", &[], root)?,
        "cpu_topology": super::output("lscpu", &["-p=CPU,CORE,SOCKET,ONLINE"], root)?,
        "filesystem": super::output("findmnt", &["-T", "/data", "-o", "SOURCE,FSTYPE,OPTIONS"], root)?,
        "affinity": super::output("taskset", &["-pc", &std::process::id().to_string()], root)
            .unwrap_or_else(|_| "unavailable".into())
            .rsplit_once(": ").map(|(_, cpus)| cpus.to_owned()).unwrap_or_else(|| "unavailable".into()),
        "memory_total": fs::read_to_string("/proc/meminfo").ok()
            .and_then(|contents| contents.lines().find(|line| line.starts_with("MemTotal:" )).map(str::to_owned))
            .unwrap_or_else(|| "unavailable".into()),
        "power_policy": fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").unwrap_or_else(|_| "unavailable".into()),
        "cache_state": "one full-width warmup precedes five measured launches; cache state is not reset or controlled",
        "systemd": super::output(COLLECTION_COMMAND, &["--version"], root).unwrap_or_else(|_| "unavailable".into()),
    }))
}

fn collection(args: &[&str], root: &Path) -> Result<(), String> {
    let ["collection", directory, runner] = args else {
        return Err("profile collection ABSOLUTE_ARTIFACT_DIRECTORY ABSOLUTE_RUNNER".into());
    };
    let directory = Path::new(directory);
    if !directory.is_absolute() || directory.exists() {
        return Err("use a new absolute artifact directory".into());
    }
    let raw_parent = directory.parent().ok_or("artifact has no parent")?;
    let resolved = evidence::artifact_path(directory, root, false)?;
    let parent = disk_backed_directory(raw_parent)?;
    if resolved.parent() != Some(parent.as_path()) {
        return Err("artifact parent changed during validation".into());
    }
    let directory = parent.join(resolved.file_name().ok_or("artifact has no name")?);
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    write_collection_result(
        &directory,
        &serde_json::json!({
            "mode": "collection", "success": false,
            "disposition": "incomplete", "phase": "setup", "executed": false,
        }),
    )?;
    let capture = super::capture(root)?;
    let candidate = capture.lines().next().ok_or("empty source capture")?;
    let receipt_path = root.join(".local/last-verification");
    let (receipt_bytes, receipt_truncated) = read_bounded_receipt(&receipt_path)?;
    let receipt_digest = evidence::hash_bytes(&receipt_bytes)?;
    let receipt_text = std::str::from_utf8(&receipt_bytes).ok();
    let native_acceptance = !receipt_truncated
        && receipt_text.is_some_and(|receipt| {
            receipt
                .lines()
                .any(|line| line == format!("candidate={candidate}"))
                && receipt.lines().any(|line| line == "result=success")
        });
    if !native_acceptance {
        if !matches!(candidate.len(), 40 | 64)
            || !candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid candidate identity in source capture".into());
        }
        write_collection_result(
            &directory,
            &serde_json::json!({
                "mode": "collection", "candidate": candidate,
                "acceptance_receipt_sha256": receipt_digest,
                "acceptance_receipt_truncated": receipt_truncated,
                "native_acceptance": false, "success": false,
                "disposition": "refused", "reason": "native_verification_missing_or_mismatched",
                "executed": false,
            }),
        )?;
        return Err(
            "collection profile requires candidate capture/verify acceptance for current source"
                .into(),
        );
    }
    let (plan, benchmark_manifest, resource_manifest) = match collection_plan(root) {
        Ok(plan) => plan,
        Err(reason) => {
            write_collection_result(
                &directory,
                &serde_json::json!({
                    "mode": "collection", "candidate": candidate,
                    "acceptance_receipt_sha256": receipt_digest,
                    "native_acceptance": true, "success": false,
                    "disposition": "refused", "reason": reason,
                    "executed": false,
                }),
            )?;
            return Err(
                "collection profile refused because the checked-in N02 policy drifted".into(),
            );
        }
    };
    collection_tmpdir()?;
    let runner = Path::new(runner);
    if !runner.is_absolute() || !runner.is_file() {
        return Err("runner must be an existing absolute executable".into());
    }
    use std::os::unix::fs::PermissionsExt;
    if runner
        .metadata()
        .map_err(|e| e.to_string())?
        .permissions()
        .mode()
        & 0o111
        == 0
    {
        return Err("runner must be executable".into());
    }
    let runner = runner.canonicalize().map_err(|e| e.to_string())?;
    let runner_digest = evidence::hash(&runner)?;
    let lock_digest = evidence::hash(&root.join("Cargo.lock"))?;
    let fixture = root.join("xtask/src/bin/collection-resources.rs");
    let fixture_digest = evidence::hash(&fixture)?;
    let resource_profile = root.join("xtask/tests/fixtures/n02/resource-profile.toml");
    let benchmark_profile = root.join("xtask/tests/fixtures/n02/benchmark-profile.toml");
    let resource_profile_digest = evidence::hash(&resource_profile)?;
    let benchmark_profile_digest = evidence::hash(&benchmark_profile)?;
    let compiler = super::output("rustc", &["--version", "--verbose"], root)?;
    let toolchain =
        fs::read_to_string(root.join("rust-toolchain.toml")).map_err(|e| e.to_string())?;
    let machine = machine_profile(root)?;
    let machine_profile_digest =
        evidence::hash_bytes(&serde_json::to_vec(&machine).map_err(|e| e.to_string())?)?;
    let profile_digest = evidence::hash_bytes(
        &[
            resource_profile_digest.as_bytes(),
            benchmark_profile_digest.as_bytes(),
        ]
        .concat(),
    )?;
    let mut repetitions = Vec::new();
    write_collection_result(
        &directory,
        &serde_json::json!({
            "mode": "collection", "candidate": candidate,
            "acceptance_receipt_sha256": receipt_digest,
            "executable_sha256": runner_digest,
            "disposition": "incomplete", "phase": "running", "attempted": false,
            "repetitions": &repetitions,
        }),
    )?;
    for index in 0..plan.warmups + plan.measured_repeats {
        let phase = if index < plan.warmups {
            "warmup"
        } else {
            "measured"
        };
        let label = format!("{phase}-{:02}", if index == 0 { 0 } else { index });
        let unit = format!(
            "fabrico11y-collection-{}-{index}.service",
            std::process::id()
        );
        let mut command = Command::new("timeout");
        command
            .stdin(Stdio::null())
            .args([
                "--kill-after=5s",
                "135s",
                COLLECTION_COMMAND,
                "--user",
                "--wait",
                "--collect",
                "--pipe",
                "--quiet",
                "--slice=fabrico11y.slice",
                "--setenv=TMPDIR=/data/tmp",
                "--working-directory",
                root.to_str().ok_or("non-UTF8 source path")?,
                "--unit",
                unit.as_str(),
                "-p",
                "MemoryHigh=256M",
                "-p",
                "MemoryMax=512M",
                "-p",
                "CPUQuota=15%",
                "-p",
                "IOWeight=30",
                "-p",
                "TasksMax=64",
                "-p",
                "RuntimeMaxSec=120",
                "-p",
                "TimeoutStopSec=5",
                "--",
            ])
            .arg(&runner)
            .args(["--case", plan.case.as_str()])
            .current_dir(root);
        let record = match run_bounded(&mut command) {
            Ok(output) => {
                let stdout_limit_exceeded =
                    output.stdout.total_bytes > MAX_COLLECTION_STREAM_BYTES as u64;
                let stderr_limit_exceeded =
                    output.stderr.total_bytes > MAX_COLLECTION_STREAM_BYTES as u64;
                let over_limit = stdout_limit_exceeded || stderr_limit_exceeded;
                let exit_code = output.status.as_ref().and_then(|status| status.code());
                let command_success = output
                    .status
                    .as_ref()
                    .is_some_and(|status| status.success());
                let client_timed_out = output.timed_out || matches!(exit_code, Some(124 | 137));
                let reader_error =
                    output.stdout.read_error.is_some() || output.stderr.read_error.is_some();
                let disposition = if over_limit {
                    "over_limit"
                } else if client_timed_out {
                    "timed_out"
                } else if reader_error {
                    "failed"
                } else {
                    collection_disposition(command_success, &output.stdout.bytes, &plan)
                };
                fs::write(
                    directory.join(format!("{label}.stdout")),
                    &output.stdout.bytes,
                )
                .map_err(|e| e.to_string())?;
                fs::write(
                    directory.join(format!("{label}.stderr")),
                    &output.stderr.bytes,
                )
                .map_err(|e| e.to_string())?;
                serde_json::json!({
                    "phase": phase, "ordinal": index, "disposition": disposition,
                    "exit": exit_code,
                    "report_complete": !over_limit && !client_timed_out && !reader_error && collection_output_complete(&output.stdout.bytes, &plan),
                    "stdout": format!("{label}.stdout"), "stderr": format!("{label}.stderr"),
                    "stdout_retained_bytes": output.stdout.bytes.len(), "stdout_observed_bytes": output.stdout.total_bytes,
                    "stdout_truncated": output.stdout.truncated,
                    "stdout_limit_exceeded": stdout_limit_exceeded,
                    "stderr_retained_bytes": output.stderr.bytes.len(), "stderr_observed_bytes": output.stderr.total_bytes,
                    "stderr_truncated": output.stderr.truncated,
                    "stderr_limit_exceeded": stderr_limit_exceeded,
                    "client_timed_out": client_timed_out,
                    "stream_limit_bytes": MAX_COLLECTION_STREAM_BYTES,
                    "read_per_poll_per_stream_bytes": MAX_COLLECTION_READ_PER_TICK,
                    "reader_error": output.stdout.read_error.or(output.stderr.read_error),
                    "run_error": output.run_error,
                    "scope": "systemd transient user service with --collect; placement in fabrico11y.slice requested; timeout bounds systemd-run at 135s",
                })
            }
            Err(error) => {
                fs::write(directory.join(format!("{label}.stdout")), &[] as &[u8])
                    .map_err(|e| e.to_string())?;
                fs::write(directory.join(format!("{label}.stderr")), &error)
                    .map_err(|e| e.to_string())?;
                serde_json::json!({
                    "phase": phase, "ordinal": index, "disposition": "unavailable",
                    "reason": error.to_string(), "stdout": format!("{label}.stdout"), "stderr": format!("{label}.stderr"),
                    "scope": "systemd transient user service with --collect; placement in fabrico11y.slice requested; timeout bounds systemd-run at 135s",
                })
            }
        };
        repetitions.push(record);
        write_collection_result(
            &directory,
            &serde_json::json!({
                "mode": "collection", "candidate": candidate,
                "acceptance_receipt_sha256": receipt_digest,
                "executable_sha256": runner_digest,
            "disposition": "incomplete", "phase": "running", "attempted": true,
                "repetitions": &repetitions,
            }),
        )?;
    }
    let unchanged = super::capture(root)? == capture;
    let runner_unchanged = evidence::hash(&runner)? == runner_digest;
    let all_complete = repetitions.iter().all(|r| r["disposition"] == "completed");
    let metrics = collection_metric_summary(&repetitions, &directory)?;
    let measured_counts: std::collections::BTreeMap<_, _> = [
        "completed",
        "failed",
        "refused",
        "unavailable",
        "over_limit",
        "timed_out",
    ]
    .into_iter()
    .map(|disposition| {
        (
            disposition,
            repetitions
                .iter()
                .filter(|r| r["phase"] == "measured" && r["disposition"] == disposition)
                .count(),
        )
    })
    .collect();
    let success = native_acceptance && all_complete && unchanged && runner_unchanged;
    write_collection_result(
        &directory,
        &serde_json::json!({
            "mode": "collection", "case": plan.case,
            "success": success,
            "disposition": if success { "complete" } else { "incomplete" },
            "attempted": !repetitions.is_empty(),
            "native_acceptance": native_acceptance, "source_unchanged": unchanged, "runner_unchanged": runner_unchanged,
            "acceptance_receipt_truncated": receipt_truncated,
            "candidate_capture": capture,
            "acceptance_receipt_sha256": receipt_digest,
            "rustc": compiler, "toolchain": toolchain, "cargo_lock_sha256": lock_digest,
            "fixture_sha256": fixture_digest,
            "resource_profile_sha256": resource_profile_digest,
            "benchmark_profile_sha256": benchmark_profile_digest,
            "profile_sha256": profile_digest,
            "executable_sha256": runner_digest,
            "machine_profile_sha256": machine_profile_digest, "machine_profile": machine,
            "benchmark_manifest": benchmark_manifest, "resource_manifest": resource_manifest,
            "workload": { "case": plan.case, "slots": plan.slots, "record_capacity_bytes": plan.record_capacity_bytes, "producers": plan.producers, "warmups": plan.warmups, "measured_repetitions": plan.measured_repeats },
            "measured_disposition_counts": measured_counts,
            "requested_service_settings": { "slice": "fabrico11y.slice", "TMPDIR": "/data/tmp", "MemoryHigh": "256M", "MemoryMax": "512M", "CPUQuota": "15%", "IOWeight": 30, "TasksMax": 64, "RuntimeMaxSec": 120, "TimeoutStopSec": 5, "client_timeout_seconds": 135, "client_kill_after_seconds": 5, "client_loop_max_seconds": MAX_COLLECTION_CLIENT_WAIT.as_secs(), "transient_service_collect": true, "working_directory": root },
            "ambient_parent_slice_limits": "not independently inspected; transient service placement in fabrico11y.slice is requested",
            "over_limit_handling": "streams retain at most 1 MiB each; additional bytes are drained and discarded until the 120-second service limit or 135-second client timeout; over_limit never completes",
            "repetitions": repetitions,
            "repeat_metrics": metrics,
            "unsupported_metrics": ["peak-fds-by-process", "cpu-time-ns", "wakeups", "cgroup-file-cache-bytes", "bytes-written", "scratch-peak-bytes", "observer-overhead"],
            "note": "Runner executable is supplied by caller and its digest is recorded; build/source correspondence is not independently inferred from the binary digest. Failed, refused and unavailable repetition stdout/stderr are retained. Repeat metrics summarize measured completed reports only; warmup is excluded.",
        }),
    )?;
    if success {
        Ok(())
    } else {
        Err(format!(
            "collection profile incomplete; inspect {}",
            directory.display()
        ))
    }
}

#[cfg(test)]
mod collection_tests {
    use std::path::Path;

    fn test_tmpdir() -> tempfile::TempDir {
        let tmp = super::collection_tmpdir().unwrap();
        tempfile::Builder::new()
            .prefix("fabrico11y-profile-test-")
            .tempdir_in(tmp)
            .unwrap()
    }

    fn plan() -> super::CollectionPlan {
        super::CollectionPlan {
            case: "full-width".into(),
            slots: 8,
            record_capacity_bytes: 4096,
            producers: 4,
            warmups: 1,
            measured_repeats: 5,
        }
    }

    #[test]
    fn accepts_only_one_complete_full_width_report() {
        let valid = br#"{"passed":true,"harness_lifetime_peak_rss_bytes":{"state":"available","bytes":1},"visible_cgroup_lifetime_peak_bytes":{"state":"available","bytes":1},"workloads":[{"case":"full-width","slots":8,"record_capacity":4096,"producers":4,"rss_before":{"state":"available","bytes":1},"rss_at_full":{"state":"available","bytes":1},"collector_rss_with_slow_clients":{"state":"available","bytes":1},"daemon_lifetime_peak_rss_bytes":{"state":"available","bytes":1} }]}"#;
        assert!(super::collection_output_complete(valid, &plan()));
        let wrong_case = br#"{"passed":true,"workloads":[{"case":"sequence-width","slots":8,"record_capacity":4096,"producers":4}]}"#;
        let multiple = br#"{"passed":true,"workloads":[{"case":"full-width","slots":8,"record_capacity":4096,"producers":4},{"case":"short-record"}]}"#;
        assert!(!super::collection_output_complete(wrong_case, &plan()));
        assert!(!super::collection_output_complete(multiple, &plan()));
        assert!(!super::collection_output_complete(b"not-json", &plan()));
        assert_eq!(
            super::collection_disposition(false, br#"{"refused":true}"#, &plan()),
            "refused"
        );
        assert_eq!(super::collection_disposition(false, b"", &plan()), "failed");
    }

    #[test]
    fn n02_plan_matches_executable_case_and_repeat_contract() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let (plan, _, _) = super::collection_plan(root).expect("valid N02 policy");
        assert_eq!(plan.case, "full-width");
        assert_eq!(
            (plan.slots, plan.record_capacity_bytes, plan.producers),
            (8, 4096, 4)
        );
        assert_eq!((plan.warmups, plan.measured_repeats), (1, 5));
    }

    #[test]
    fn repeat_summary_uses_only_successful_measured_reports() {
        let directory = test_tmpdir();
        let workload = |value| {
            serde_json::json!({"passed": true, "workloads": [{"calls": value}]}).to_string()
        };
        std::fs::write(directory.path().join("warmup.json"), workload(999)).unwrap();
        std::fs::write(directory.path().join("measured.json"), workload(7)).unwrap();
        std::fs::write(directory.path().join("failed.json"), workload(123)).unwrap();
        let repetitions = [
            serde_json::json!({"phase":"warmup","disposition":"completed","stdout":"warmup.json"}),
            serde_json::json!({"phase":"measured","disposition":"completed","stdout":"measured.json"}),
            serde_json::json!({"phase":"measured","disposition":"refused","stdout":"failed.json"}),
        ];
        let summary = super::collection_metric_summary(&repetitions, directory.path()).unwrap();
        assert_eq!(summary["workload.calls"]["samples"], serde_json::json!([7]));
        assert_eq!(
            summary["workload.calls"]["sample_variance"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn bounded_stream_discards_but_drains_excess() {
        let mut output = super::BoundedStream::new();
        super::retain_bytes(&mut output, b"abcd", 4);
        super::retain_bytes(&mut output, b"efgh", 4);
        assert_eq!(output.bytes, b"abcd");
        assert_eq!(output.total_bytes, 8);
        assert!(output.truncated);
    }

    #[test]
    fn rejects_ram_tmp_and_symlink_aliases() {
        assert!(super::disk_backed_directory(Path::new("/tmp")).is_err());
        let directory = test_tmpdir();
        let alias = directory.path().join("tmp-alias");
        std::os::unix::fs::symlink("/tmp", &alias).unwrap();
        assert!(super::disk_backed_directory(&alias).is_err());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn incomplete_or_unavailable_counters_cannot_pass() {
        let good = "0.88,msec,task-clock:u\n381030,,cycles:u\n161077,,instructions:u\n";
        assert!(super::counters_complete(good));
        for bad in [
            good.replace("381030", "<not counted>"),
            good.replace("161077", "NaN"),
            good.replace("cycles:u", "cycles-other:u"),
            String::new(),
        ] {
            assert!(!super::counters_complete(&bad));
        }
    }
    #[test]
    fn partial_benchmark_output_cannot_pass() {
        let configuration = "{\"event\":\"configuration\",\"expected_samples\":2}\n";
        let sample = "{\"event\":\"measurement\",\"records\":1,\"elapsed_ns\":10}\n";
        assert!(!super::benchmark_complete(b""));
        assert!(!super::benchmark_complete(
            format!("{configuration}{sample}").as_bytes()
        ));
        assert!(super::benchmark_complete(
            format!("{configuration}{sample}{sample}").as_bytes()
        ));
    }
}
