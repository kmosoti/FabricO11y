//! Frozen workload generator. Seed 17 is development; 991 is held out.
use fabric::observation::Observation;
use serde_json::{Value, json};
pub fn records(seed: u64, case: &str, count: usize) -> Vec<Observation> {
    (0..count).map(|n| {
        let unique=format!("origin-{seed}-record-{n}");
        let data=match case {
            "metrics"=>json!({"kind":"metric","name":"cpu.time","unit":"ns","value":(seed+n as u64) as f64,"metric_type":"counter","reset":n==0}),
            "unique"=>json!({"kind":"tool_execution","tool":unique,"run_id":format!("run-{seed}-{n}"),"duration_ns":u64::MAX-n as u64,"status":"ok"}),
            "maximum"=>json!({"kind":"log","level":"info","message":"λ".repeat(16384)}),
            "structured"=>json!({"kind":"json_log","fields":{"integer":u64::MAX,"negative":i64::MIN,"float":-0.0,"nested":[null,true,{"value":unique}]}}),
            _=> match n%8 {
                0=>json!({"kind":"service_request","service":"api","operation":"GET","duration_ns":seed,"status":"ok","trace_id":null}),
                1=>json!({"kind":"agent_run","agent":"agent","run_id":unique,"duration_ns":seed,"status":"cancelled"}),
                2=>json!({"kind":"tool_execution","tool":"read","run_id":unique,"duration_ns":seed,"status":"error"}),
                3=>json!({"kind":"log","level":"warn","message":"short\nquoted \" text"}),
                4=>json!({"kind":"json_log","fields":{"sample":seed,"empty":[],"message":unique}}),
                5=>json!({"kind":"event","name":"ready","message":"event"}),
                6=>json!({"kind":"metric","name":"gauge","unit":"1","value":-0.0,"metric_type":"gauge","reset":false}),
                _=>json!({"kind":"gap","source":"file:app","reason":"rotation"}),
            }
        };
        let v:Value=json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"fixture","generation":format!("g-{seed}"),"sequence":n+1},"timestamp_ns":seed+n as u64,"resource":"host","data":data});
        let o:Observation=serde_json::from_value(v).unwrap();o.validate().unwrap();o
    }).collect()
}
pub const CASES: &[&str]=&["metrics","unique","maximum","structured","mixed"];
