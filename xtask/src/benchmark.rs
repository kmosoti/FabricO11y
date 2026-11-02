//! Compare independent benchmark dimensions after native acceptance.
use super::evidence;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

fn median(rows: &[Value], field: &str) -> Result<f64, String> {
    let mut values = rows
        .iter()
        .map(|r| {
            r[field]
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("missing benchmark metric: {field}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    values.sort_by(f64::total_cmp);
    values
        .get(values.len() / 2)
        .copied()
        .ok_or_else(|| "empty measurement group".into())
}
fn load(path: &Path) -> Result<(Value, BTreeMap<String, Vec<Value>>), String> {
    let result: Value = evidence::read(&path.join("result.json"))?;
    if result["mode"] != "benchmark"
        || result["success"] != true
        || result["native_acceptance"] != true
    {
        return Err(format!(
            "{}: incomplete or unaccepted benchmark cannot be ranked",
            path.display()
        ));
    }
    let stdout = fs::read_to_string(path.join("stdout")).map_err(|e| e.to_string())?;
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for line in stdout.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row["event"] != "measurement" {
            continue;
        }
        let key = serde_json::json!([
            row["case"],
            row["text_bytes"],
            row["vector_len"],
            row["encoded_bytes"],
            row["slots"],
            row["max_record"],
            row["producers"],
            row["consumers"],
            row["records"],
            row["warmup_samples"],
            row["deadline_seconds_per_sample"]
        ])
        .to_string();
        groups.entry(key).or_default().push(row);
    }
    if groups.is_empty() {
        return Err("benchmark has no measurements".into());
    }
    Ok((result, groups))
}
pub fn compare(paths: &[&str]) -> Result<(), String> {
    if paths.len() < 2 {
        return Err("benchmark compare BASELINE_DIRECTORY CANDIDATE_DIRECTORY...".into());
    }
    let (baseline, groups) = load(Path::new(paths[0]))?;
    println!(
        "First directory is the baseline. Throughput ratios >1 and latency ratios <1 are favorable; no combined score."
    );
    for path in &paths[1..] {
        let (candidate, measurements) = load(Path::new(path))?;
        for field in [
            "driver_sha256",
            "contract_sha256",
            "compiler",
            "os",
            "hardware_topology",
            "cases_sha256",
            "build_configuration",
            "evaluator_policy_sha256",
        ] {
            if candidate[field] != baseline[field] {
                return Err(format!("incomparable benchmark {field}"));
            }
        }
        if groups.keys().ne(measurements.keys()) {
            return Err("incomparable workload cases".into());
        }
        for (key, rows) in &measurements {
            let reference = &groups[key];
            if rows.len() != reference.len() {
                return Err("incomparable repetition counts".into());
            }
            let throughput =
                median(rows, "records_per_second")? / median(reference, "records_per_second")?;
            let p99 = median(rows, "record_with_retries_p99_ns")?
                / median(reference, "record_with_retries_p99_ns")?;
            println!(
                "{}",
                serde_json::json!({"candidate":path,"case":rows[0]["case"],"throughput_ratio":throughput,
                "record_p99_ratio":p99,"accepted_call_p99_ratio":median(rows,"accepted_call_p99_ns")?/median(reference,"accepted_call_p99_ns")?,
                "setup_ns":median(rows,"setup_ns")?,"payload_bytes":median(rows,"payload_bytes")?,
                "collection_bytes":median(rows,"collection_bytes")?,"control_bytes":median(rows,"control_bytes")?,
                "attempts":median(rows,"attempts")?,"baseline_attempts":median(reference,"attempts")?})
            );
        }
    }
    Ok(())
}
