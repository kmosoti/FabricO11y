use fabric_o11y::alpha::node::{Config, inspect};
use std::process::ExitCode;

fn hex_id(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [mode, config_path] = args.as_slice() else {
        eprintln!("usage: fabricctl inspect <CONFIG_PATH>");
        return ExitCode::from(2);
    };
    if mode != "inspect" {
        eprintln!("usage: fabricctl inspect <CONFIG_PATH>");
        return ExitCode::from(2);
    }
    let result = Config::load(config_path).and_then(|config| inspect(&config));
    match result {
        Ok(report) => {
            println!(
                "node_id={} generation={} batches={} metric_points={} log_records={} gaps={} otlp_payload_bytes={} committed_bytes={} file_bytes={} coverage_unknown={} recovery_required={} interrupted_append={} next_sequence={} acked_through={} log_backlog_bytes={}",
                hex_id(&report.node_id),
                report.generation,
                report.batches,
                report.metric_points,
                report.log_records,
                report.gaps,
                report.otlp_payload_bytes,
                report.committed_bytes,
                report.file_bytes,
                report.coverage_unknown,
                report.recovery_required,
                report.interrupted_append,
                report.next_sequence,
                report.acked_through,
                report.log_backlog_bytes
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fabricctl: {error}");
            ExitCode::FAILURE
        }
    }
}
