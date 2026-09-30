//! Replays every committed corpus and regression input of each fuzz target
//! on stable Rust, so a fixed crash stays fixed without a nightly toolchain.

use std::path::Path;

fn replay(target: &str, run: fn(&[u8])) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz");
    let mut count = 0;
    for kind in ["corpus", "regressions"] {
        let Ok(entries) = std::fs::read_dir(root.join(kind).join(target)) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            run(&std::fs::read(&path).unwrap());
            count += 1;
        }
    }
    assert!(count > 0, "no committed inputs for {target}");
}

#[test]
fn batch_identify_corpus() {
    replay("batch_identify", fabric_fuzz_targets::batch_identify);
}

#[test]
fn query_request_corpus() {
    replay("query_request", fabric_fuzz_targets::query_request);
}

#[test]
fn frame_recovery_corpus() {
    replay("frame_recovery", fabric_fuzz_targets::frame_recovery);
}
