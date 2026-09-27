use fabric_o11y::alpha::node::{Config, Node};
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [mode, config_path] = args.as_slice() else {
        eprintln!("usage: fabric-node <collect|run> <CONFIG_PATH>");
        return ExitCode::from(2);
    };
    if mode != "collect" && mode != "run" {
        eprintln!("usage: fabric-node <collect|run> <CONFIG_PATH>");
        return ExitCode::from(2);
    }
    let result = (|| {
        let config = Config::load(config_path)?;
        let interval = config.interval_s;
        let mut node = Node::open(config)?;
        loop {
            let cycle = node.collect_once()?;
            println!(
                "batch={} metrics={} logs={} gaps={} spool_bytes={}",
                cycle.batch_sequence,
                cycle.metric_points,
                cycle.log_records,
                cycle.gaps,
                cycle.spool_bytes
            );
            if mode == "collect" {
                break;
            }
            std::thread::sleep(Duration::from_secs(interval));
        }
        Ok::<(), std::io::Error>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fabric-node: {error}");
            ExitCode::FAILURE
        }
    }
}
