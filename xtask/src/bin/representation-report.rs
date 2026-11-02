//! Compare qualified samples by independent dimensions, never a combined score.
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn number(v: &Value, path: &str) -> f64 {
    v.pointer(path)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("missing metric {path}"))
}
fn median(rows: &[Value], metric: &str) -> f64 {
    let mut v: Vec<_> = rows.iter().map(|r| number(r, metric)).collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}
fn main() {
    let mut groups: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    for file in std::env::args().skip(1) {
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            if row["warmup"] == true {
                continue;
            }
            let key = json!([
                row["seed"],
                row["case"],
                row["batch"],
                row["custody"],
                row["allocation_run"]
            ])
            .to_string();
            groups
                .entry(key)
                .or_default()
                .entry(row["codec"].as_str().unwrap().into())
                .or_default()
                .push(row);
        }
    }
    let metrics = [
        "/wire_bytes_per_hop",
        "/encoded_buffer_capacity",
        "/dictionary_bytes",
        "/sender_encode_cpu_ns",
        "/sender_cpu_ns",
        "/stages/spindle/cpu_ns",
        "/stages/server/cpu_ns",
        "/total_ingest_cpu_ns",
        "/spindle_seal_ns",
        "/fabric_seal_ns",
        "/sender_peak_rss_bytes",
        "/stages/spindle/peak_rss_bytes",
        "/stages/server/peak_rss_bytes",
        "/stages/server/disk_bytes",
        "/stages/server/materialize_cpu_ns",
        "/stages/server/projection_cpu_ns",
        "/stages/server/recovery_ns",
    ];
    for (case, candidates) in groups {
        let baseline = candidates.get("json").expect("JSON baseline missing");
        assert_eq!(baseline.len(), 5);
        for (codec, rows) in &candidates {
            assert_eq!(rows.len(), 5);
            let allocations = rows[0]["allocation_run"] == true;
            let mut values = serde_json::Map::new();
            for metric in metrics {
                if allocations
                    && (metric.contains("cpu")
                        || metric.contains("seal_ns")
                        || metric.contains("recovery_ns"))
                {
                    continue;
                }
                let b = median(baseline, metric);
                let c = median(rows, metric);
                values.insert(
                    metric.into(),
                    json!({"median":c,"baseline_median":b,"ratio":if b>0.0{Some(c/b)}else{None}}),
                );
            }
            if allocations {
                for role in [
                    "/sender_allocations",
                    "/stages/spindle/allocations",
                    "/stages/server/allocations",
                ] {
                    for metric in ["allocated_bytes", "peak_live_bytes"] {
                        let path = format!("{role}/{metric}");
                        let b = median(baseline, &path);
                        let c = median(rows, &path);
                        values.insert(path, json!({"median":c,"baseline_median":b,"ratio":c/b}));
                    }
                }
            }
            let saved =
                median(baseline, "/wire_bytes_per_hop") - median(rows, "/wire_bytes_per_hop");
            let extra =
                median(rows, "/total_ingest_cpu_ns") - median(baseline, "/total_ingest_cpu_ns");
            println!(
                "{}",
                json!({"case":case,"codec":codec,"metrics":values,"cpu_transfer_break_even_mbps":if !allocations&&saved>0.0&&extra>0.0{Some(saved*8000.0/extra)}else{None},"break_even_scope":"one remote hop; equates serialized added aggregate CPU to saved wire transfer time; not pipelined throughput or energy"})
            );
        }
    }
}
