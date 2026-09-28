use fabric_o11y::alpha::node::{Config, Node};
use std::fs;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("scratch directory exists");
    let cases = [
        ("zero_spool", 0_u64, 15_u64, 0_usize),
        ("oversized_spool", u64::MAX, 15, 0),
        ("zero_interval", 8192, 0, 0),
        ("too_many_logs", 8192, 15, 17),
    ];
    let mut any_accepted_or_panicked = false;
    for (label, spool_bytes, interval_s, log_count) in cases {
        let spool = root.join(format!("spool-{label}"));
        let logs = (0..log_count)
            .map(|index| root.join(format!("log-{label}-{index}")))
            .collect();
        let config = Config { spool, logs, interval_s, spool_bytes };
        let result = std::panic::catch_unwind(|| Node::open(config));
        let state = match result {
            Ok(Ok(node)) => { drop(node); any_accepted_or_panicked = true; "accepted" },
            Ok(Err(_)) => "rejected",
            Err(_) => { any_accepted_or_panicked = true; "panicked" },
        };
        println!("case={label} state={state}");
    }
    let loaded = root.join("loaded.conf");
    fs::write(&loaded, format!("spool_dir={}\nspool_bytes=0\n", root.join("file-config-spool").display()))
        .unwrap();
    let file_rejected = Config::load(&loaded).is_err();
    println!("file_config_zero_spool_rejected={file_rejected}");
    if any_accepted_or_panicked || !file_rejected {
        std::process::exit(1);
    }
}
