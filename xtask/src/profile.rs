use super::evidence;
use std::{fs, path::Path, process::Command};

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
    let [mode @ ("permissions" | "benchmark"), directory] = args else {
        return Err("profile permissions|benchmark ABSOLUTE_ARTIFACT_DIRECTORY".into());
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
        return Err(
            "benchmark requires candidate capture/verify acceptance for the current source".into(),
        );
    }
    let version = super::output("perf", &["--version"], root)?;
    let budget: u64 = std::env::var("FABRIC_PROFILE_SECONDS")
        .map(|s| s.parse().map_err(|_| "invalid FABRIC_PROFILE_SECONDS"))
        .unwrap_or(Ok(if *mode == "benchmark" { 900 } else { 30 }))?;
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
            .env("CARGO_BUILD_JOBS", "2")
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
        build_configuration = serde_json::json!({"profile":"bench", "jobs":2,
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
            "counters": "task-clock,cycles,instructions", "build_jobs": 2,
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
