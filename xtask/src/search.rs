//! Small, model-independent evaluation interface. No model orchestration.
use super::evidence;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub task: String,
    pub strategy: String,
    pub trial: u32,
    pub family: String,
    pub rationale: String,
    pub parent: Option<String>,
    #[serde(default)]
    pub parent_source_sha256: Option<String>,
    pub source: PathBuf,
    pub algorithm: String,
    pub representation: String,
    pub ownership: String,
    pub data_flow: String,
    pub falsification: String,
}
#[derive(Serialize, Deserialize)]
struct Candidate {
    proposal: Proposal,
    source_sha256: String,
    contract_sha256: String,
    repository_capture: String,
    status: String,
}
#[derive(Serialize, Deserialize)]
struct Measurement {
    case: String,
    n: usize,
    parameter: usize,
    repeats: usize,
    elapsed_ns: u128,
    compute_ns: u128,
    query_count: usize,
    peak_rss_kib: u64,
}
#[derive(Serialize, Deserialize)]
struct Evaluation {
    task: String,
    source_sha256: String,
    contract_sha256: String,
    evaluator_sha256: String,
    rustc: String,
    split: String,
    status: String,
    error: Option<String>,
    measurements: Vec<Measurement>,
}
fn task_ok(task: &str) -> Result<(), String> {
    if ["membership", "topk", "intervals"].contains(&task) {
        Ok(())
    } else {
        Err("unknown task".into())
    }
}
fn contract(root: &Path) -> PathBuf {
    root.join("verification/search-contract.md")
}
fn register(manifest: &Path, dir: &Path, root: &Path) -> Result<(), String> {
    let mut proposal: Proposal = evidence::read(manifest)?;
    task_ok(&proposal.task)?;
    if proposal.parent.is_some()
        && !proposal
            .parent_source_sha256
            .as_ref()
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("parent requires a concrete parent_source_sha256".into());
    }
    if !["serial", "fresh", "retained", "baseline"].contains(&proposal.strategy.as_str()) {
        return Err("unknown strategy".into());
    }
    for field in [
        &proposal.family,
        &proposal.rationale,
        &proposal.algorithm,
        &proposal.representation,
        &proposal.ownership,
        &proposal.data_flow,
        &proposal.falsification,
    ] {
        if field.trim().is_empty() {
            return Err("structural choices and falsification experiment required".into());
        }
    }
    let dir = evidence::artifact_path(dir, root, true)?;
    let source = fs::read(&proposal.source).map_err(|e| e.to_string())?;
    if source.len() > 64 * 1024 {
        return Err("candidate exceeds 64 KiB".into());
    }
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("candidate.rs"), source).map_err(|e| e.to_string())?;
    proposal.source = PathBuf::from("candidate.rs");
    let record = Candidate {
        proposal,
        source_sha256: evidence::hash(&dir.join("candidate.rs"))?,
        contract_sha256: evidence::hash(&contract(root))?,
        repository_capture: super::capture(root)?,
        status: "proposed".into(),
    };
    evidence::write(&dir.join("candidate.json"), &record)
}
fn sandbox(work: &Path) -> Command {
    let mut command = Command::new("timeout");
    command
        .args([
            "30",
            "bwrap",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--setenv",
            "PATH",
            "/usr/bin",
            "--bind",
        ])
        .arg(work)
        .arg("/work")
        .args(["--chdir", "/work"]);
    command
}
fn run_child(mut command: Command, work: &Path, input: &str) -> Result<(), String> {
    fs::write(work.join("input"), input).map_err(|e| e.to_string())?;
    let stdout = fs::File::create(work.join("stdout")).map_err(|e| e.to_string())?;
    let stderr = fs::File::create(work.join("stderr")).map_err(|e| e.to_string())?;
    let status = command
        .stdin(fs::File::open(work.join("input")).map_err(|e| e.to_string())?)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "candidate command failed ({status}): {}",
            fs::read_to_string(work.join("stderr"))
                .unwrap_or_default()
                .chars()
                .take(3000)
                .collect::<String>()
        ));
    }
    Ok(())
}
fn reference(task: &str, data: &[u64], queries: &[u64], k: usize) -> Vec<u64> {
    match task {
        "membership" => {
            let set: std::collections::BTreeSet<_> = data.iter().copied().collect();
            queries.iter().map(|q| u64::from(set.contains(q))).collect()
        }
        "topk" => {
            let mut v = data.to_vec();
            v.sort_unstable_by(|a, b| b.cmp(a));
            v.truncate(k);
            v
        }
        "intervals" => {
            let mut pairs: Vec<_> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| (p[0], p[1]))
                .collect();
            pairs.sort_unstable();
            let mut out: Vec<(u64, u64)> = Vec::new();
            for (lo, hi) in pairs {
                if let Some(last) = out.last_mut()
                    && lo <= last.1
                {
                    last.1 = last.1.max(hi);
                    continue;
                }
                out.push((lo, hi));
            }
            out.into_iter().flat_map(|(a, b)| [a, b]).collect()
        }
        _ => unreachable!(),
    }
}
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn evaluate(dir: &Path, out: &Path, split: &str, root: &Path) -> Result<(), String> {
    if !["development", "heldout"].contains(&split) {
        return Err("unknown split".into());
    }
    let candidate: Candidate = evidence::read(&dir.join("candidate.json"))?;
    if evidence::hash(&dir.join("candidate.rs"))? != candidate.source_sha256
        || evidence::hash(&contract(root))? != candidate.contract_sha256
    {
        return Err("stale candidate or contract".into());
    }
    if out.exists() {
        return Err("preserve existing evaluation".into());
    }
    let mut result = Evaluation {
        task: candidate.proposal.task.clone(),
        source_sha256: candidate.source_sha256,
        contract_sha256: candidate.contract_sha256,
        evaluator_sha256: evidence::hash(&std::env::current_exe().map_err(|e| e.to_string())?)?,
        rustc: super::output("rustc", &["--version", "--verbose"], root)?,
        split: split.into(),
        status: "failed".into(),
        error: None,
        measurements: Vec::new(),
    };
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let outcome = (|| {
        fs::copy(dir.join("candidate.rs"), work.path().join("candidate.rs"))
            .map_err(|e| e.to_string())?;
        fs::write(
            work.path().join("main.rs"),
            include_str!("search_runner.txt"),
        )
        .map_err(|e| e.to_string())?;
        let sysroot = super::output("rustc", &["--print", "sysroot"], root)?;
        let mut build = sandbox(work.path());
        build.args([
            "--ro-bind",
            &sysroot,
            "/toolchain",
            "--",
            "/usr/bin/prlimit",
            "--fsize=16777216",
            "--cpu=25",
            "--",
            "/toolchain/bin/rustc",
            "--edition=2024",
            "-C",
            "opt-level=3",
            "-C",
            "overflow-checks=yes",
            "main.rs",
            "-o",
            "candidate",
        ]);
        run_child(build, work.path(), "")?;
        let mut seed = if split == "development" {
            0x1234abcdu64
        } else {
            0xfedc987654u64
        };
        for (case, n) in [
            ("empty", 0),
            ("small", 8),
            ("medium", 256),
            ("large", 4096),
            ("duplicates", 512),
            ("boundaries", 8),
            ("disjoint", 256),
            ("ordered", 512),
            ("reversed", 512),
        ] {
            let mut data: Vec<_> = (0..n)
                .map(|_| next(&mut seed) % if case == "duplicates" { 7 } else { 65536 })
                .collect();
            if case == "boundaries" {
                data = vec![0, u64::MAX, 1, 1, u64::MAX - 1, u64::MAX, 3, 4];
            }
            if candidate.proposal.task == "intervals" {
                for pair in data.as_chunks_mut::<2>().0 {
                    pair.sort_unstable();
                }
            }
            if case == "disjoint" {
                data = (0..n as u64).map(|x| x * 2).collect();
            }
            if case == "ordered" || case == "reversed" {
                data.sort_unstable();
                if case == "reversed" {
                    if candidate.proposal.task == "intervals" {
                        let pairs: Vec<_> = data.as_chunks::<2>().0.iter().rev().copied().collect();
                        data = pairs.into_iter().flatten().collect();
                    } else {
                        data.reverse();
                    }
                }
            }
            for k in [0, 1, n / 2, n, n + 1] {
                let queries: Vec<_> = (0..k)
                    .map(|i| {
                        if i % 2 == 0 && n > 0 {
                            data[i % n]
                        } else {
                            next(&mut seed)
                        }
                    })
                    .collect();
                let expected = reference(&candidate.proposal.task, &data, &queries, k);
                let repeats = if n >= 256 { 32 } else { 4 };
                let input = format!(
                    "{} {} {} {}\n{}\n{}\n",
                    n,
                    queries.len(),
                    k,
                    repeats,
                    data.iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                    queries
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                let mut execute = sandbox(work.path());
                execute.args([
                    "--",
                    "/usr/bin/prlimit",
                    "--as=536870912",
                    "--cpu=5",
                    "--fsize=1048576",
                    "--",
                    "/usr/bin/time",
                    "-o",
                    "/work/resources",
                    "-f",
                    "%M",
                    "/work/candidate",
                ]);
                let start = Instant::now();
                run_child(execute, work.path(), &input)?;
                let elapsed_ns = start.elapsed().as_nanos();
                let actual: Vec<u64> = fs::read_to_string(work.path().join("stdout"))
                    .map_err(|e| e.to_string())?
                    .split_whitespace()
                    .map(|s| s.parse().map_err(|_| "invalid output".to_owned()))
                    .collect::<Result<_, _>>()?;
                if actual != expected {
                    return Err(format!("incorrect result: {case}, n={n}, k={k}"));
                }
                result.measurements.push(Measurement {
                    case: case.into(),
                    n,
                    parameter: k,
                    repeats,
                    elapsed_ns,
                    query_count: queries.len(),
                    peak_rss_kib: fs::read_to_string(work.path().join("resources"))
                        .map_err(|e| e.to_string())?
                        .trim()
                        .parse()
                        .map_err(|_| "invalid RSS measurement")?,
                    compute_ns: fs::read_to_string(work.path().join("stderr"))
                        .map_err(|e| e.to_string())?
                        .trim()
                        .strip_prefix("compute_ns=")
                        .ok_or("missing compute timer")?
                        .parse()
                        .map_err(|_| "invalid compute timer")?,
                });
            }
        }
        Ok(())
    })();
    match &outcome {
        Ok(()) => result.status = "tested".into(),
        Err(e) => result.error = Some(e.clone()),
    }
    evidence::write(out, &result)?;
    outcome
}
pub fn cli(args: &[&str], root: &Path) -> Result<(), String> {
    match args {
        ["status", dir, status @ ("evaluated" | "rejected" | "retained" | "selected"), evaluation] => {
            let dir = Path::new(dir);
            let candidate: Candidate = evidence::read(&dir.join("candidate.json"))?;
            let result: Evaluation = evidence::read(Path::new(evaluation))?;
            if candidate.source_sha256 != evidence::hash(&dir.join("candidate.rs"))?
                || candidate.source_sha256 != result.source_sha256
                || candidate.contract_sha256 != result.contract_sha256
                || candidate.contract_sha256 != evidence::hash(&contract(root))?
                || candidate.proposal.task != result.task {
                return Err("status evidence does not identify this candidate and contract".into());
            }
            if ["retained", "selected"].contains(status) && (result.status != "tested" || result.error.is_some()) {
                return Err("only correct candidates may be retained or selected".into());
            }
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
            evidence::write(&dir.join(format!("status-{stamp}.json")), &serde_json::json!({
                "status": status, "source_sha256": candidate.source_sha256,
                "evaluation_sha256": evidence::hash(Path::new(evaluation))?,
                "contract_sha256": result.contract_sha256,
            }))
        }
        ["register", manifest, dir] => register(Path::new(manifest), Path::new(dir), root),
        ["evaluate", dir, out, split] => evaluate(Path::new(dir), Path::new(out), split, root),
        ["compare", paths @ ..] if !paths.is_empty() => {
            let mut identity = None;
            let mut records = Vec::new();
            for path in paths {
                let record: Evaluation = evidence::read(Path::new(path))?;
                let key = (record.task.clone(), record.contract_sha256.clone(), record.evaluator_sha256.clone(), record.rustc.clone(), record.split.clone());
                if let Some(previous) = &identity && previous != &key { return Err("incomparable evaluation inputs".into()); } identity = Some(key);
                if record.status != "tested" || record.error.is_some() || record.measurements.is_empty() {
                    return Err(format!("{path}: incorrect or incomplete candidates cannot be ranked"));
                }
                records.push((*path, record));
            }
            let baseline = &records[0].1;
            println!("First result is the explicit comparison baseline. Ratios below 1 are faster; no aggregate score.");
            for (path, record) in &records {
                if record.measurements.len() != baseline.measurements.len() { return Err("different workload counts".into()); }
                for (m, b) in record.measurements.iter().zip(&baseline.measurements) {
                    if (&m.case, m.n, m.parameter, m.repeats, m.query_count) != (&b.case, b.n, b.parameter, b.repeats, b.query_count) || b.compute_ns == 0 {
                        return Err("different workloads or invalid baseline timer".into());
                    }
                    println!("{path}\t{}\tn={}\tk={}\tq={}\tcompute_ratio={:.4}\telapsed_ns={}\tpeak_rss_kib={}", m.case, m.n, m.parameter, m.query_count, m.compute_ns as f64 / b.compute_ns as f64, m.elapsed_ns, m.peak_rss_kib);
                }
            }
            Ok(())
        }
        _ => Err("search register MANIFEST DIRECTORY | evaluate DIRECTORY OUTPUT development|heldout | compare OUTPUT...".into()),
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn failed_results_cannot_enter_performance_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("failed.json");
        let record = super::Evaluation {
            task: "membership".into(),
            source_sha256: "source".into(),
            contract_sha256: "contract".into(),
            evaluator_sha256: "evaluator".into(),
            rustc: "compiler".into(),
            split: "development".into(),
            status: "failed".into(),
            error: Some("wrong answer".into()),
            measurements: vec![],
        };
        super::evidence::write(&file, &record).unwrap();
        let error = super::cli(&["compare", file.to_str().unwrap()], dir.path()).unwrap_err();
        assert!(error.contains("cannot be ranked"));
    }

    #[test]
    fn stale_source_is_rejected_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("candidate.rs"), "changed source").unwrap();
        let proposal = super::Proposal {
            task: "membership".into(),
            strategy: "serial".into(),
            trial: 0,
            family: "linear".into(),
            rationale: "small inputs".into(),
            parent: None,
            parent_source_sha256: None,
            source: "candidate.rs".into(),
            algorithm: "scan".into(),
            representation: "slice".into(),
            ownership: "borrow".into(),
            data_flow: "query then scan".into(),
            falsification: "large query batch".into(),
        };
        super::evidence::write(
            &dir.path().join("candidate.json"),
            &super::Candidate {
                proposal,
                source_sha256: "stale".into(),
                contract_sha256: "contract".into(),
                repository_capture: "capture".into(),
                status: "proposed".into(),
            },
        )
        .unwrap();
        let error = super::evaluate(
            dir.path(),
            &dir.path().join("out.json"),
            "development",
            dir.path(),
        )
        .unwrap_err();
        assert_eq!(error, "stale candidate or contract");
        assert!(!dir.path().join("out.json").exists());
    }

    #[test]
    fn independent_examples() {
        assert_eq!(
            super::reference("membership", &[9, 1, 9], &[1, 2, 9], 0),
            [1, 0, 1]
        );
        assert_eq!(super::reference("topk", &[2, 9, 9, 1], &[], 3), [9, 9, 2]);
        assert_eq!(
            super::reference("intervals", &[8, 9, 1, 4, 3, 8], &[], 0),
            [1, 9]
        );
    }
}
