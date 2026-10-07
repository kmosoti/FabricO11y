//! O8 private native logs/metrics node; production CLI remains serial.
use fabric_o11y::spindle::{
    host::Paths,
    runtime::{Config, Spindle},
    spool::Spool,
};
use serde_json::json;
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        6,
        "CONFIG serial|overlap SECONDS HOST_FIXTURE STOP_FILE"
    );
    let cfg = Config::load(&args[1])?;
    let overlap = match args[2].as_str() {
        "serial" => false,
        "overlap" => true,
        _ => panic!("serial|overlap"),
    };
    let seconds: u64 = args[3].parse().unwrap();
    assert!((1..=60).contains(&seconds));
    let host = PathBuf::from(&args[4]);
    let paths = Paths {
        proc_stat: host.join("stat"),
        proc_meminfo: host.join("meminfo"),
        proc_diskstats: host.join("diskstats"),
        proc_net_dev: host.join("netdev"),
        boot_id: host.join("boot_id"),
        hostname: host.join("hostname"),
        filesystem: host.clone(),
    };
    let mut node = Spindle::open_with_paths(cfg.clone(), paths)?;
    assert!(cfg.traces_listen.is_none());
    let stop_path = PathBuf::from(&args[5]);
    let stop = AtomicBool::new(false);
    let began = Instant::now();
    let end = began + Duration::from_secs(seconds);
    let mut metrics = Instant::now();
    let mut next_config = Instant::now();
    let mut last = 0u64;
    let mut recorded = 0u64;
    let mut out = io::BufWriter::new(io::stdout());
    let mut ledger = fs::File::create(cfg.spool.parent().unwrap().join("producer.jsonl"))?;
    while Instant::now() < end && !stop_path.exists() {
        if Instant::now() >= next_config {
            let poll = node.poll_config()?;
            if let Some(error) = poll.error {
                writeln!(out, "{}", json!({"event":"config_error","error":error}))?;
            }
            next_config = Instant::now() + Duration::from_secs(5);
        }
        let want_metrics = Instant::now() >= metrics;
        let cycle_start = Instant::now();
        let mut attempt = |a: &fabric_o11y::spindle::runtime::Attempt| {
            println!(
                "{}",
                json!({"event":"attempt","unix_ns":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
                "sequence":a.sequence,"sha256":a.sha256,"outcome":format!("{:?}",a.outcome),"elapsed_us":a.elapsed_us})
            );
        };
        if last <= node.acked_through() && !node.paused() {
            let cycle = if want_metrics {
                Some(node.collect_once()?)
            } else {
                node.collect_logs()?
            };
            if let Some(cycle) = cycle {
                last = cycle.batch_sequence;
                writeln!(
                    out,
                    "{}",
                    json!({"event":"commit","sequence":last,"unix_ns":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),"logs":cycle.log_records,"metrics":cycle.metric_points,"backlog":cycle.log_backlog_bytes})
                )?;
                if want_metrics {
                    metrics = Instant::now() + Duration::from_secs(node.interval_s());
                }
            }
        }
        if last > node.acked_through() {
            if overlap {
                let overlap_metrics = Instant::now() >= metrics;
                let r = node.deliver_with_one_prepared(
                    Instant::now() + Duration::from_secs(1),
                    overlap_metrics,
                    &stop,
                    &mut attempt,
                )?;
                if let Some(cycle) = r.prepared {
                    last = cycle.batch_sequence;
                    writeln!(
                        out,
                        "{}",
                        json!({"event":"commit","sequence":last,"unix_ns":r.prepared_unix_ns.expect("post-Spool clock sample"),"logs":cycle.log_records,"metrics":cycle.metric_points,"backlog":cycle.log_backlog_bytes})
                    )?;
                    if overlap_metrics {
                        metrics = Instant::now() + Duration::from_secs(node.interval_s());
                    }
                }
                if let Some(error) = r.collection_error {
                    writeln!(out, "{}", json!({"event":"collection_error","error":error}))?;
                }
                if let Some(error) = r.delivery.error {
                    writeln!(out, "{}", json!({"event":"delivery_error","error":error}))?;
                }
            } else {
                let r = node.deliver(Instant::now() + Duration::from_secs(1), &mut attempt)?;
                if let Some(error) = r.error {
                    writeln!(out, "{}", json!({"event":"delivery_error","error":error}))?;
                }
            }
        }
        Spool::inspect(&cfg.spool, cfg.spool_bytes - 4096, |batch| {
            if batch.sequence > recorded {
                use prost::Message;
                let bytes = batch.encode_to_vec();
                writeln!(
                    ledger,
                    "{}",
                    json!({"label":"o8-node","hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>(),"sequence":batch.sequence})
                )?;
                recorded = batch.sequence;
            }
            Ok(())
        })?;
        writeln!(
            out,
            "{}",
            json!({"event":"cycle","elapsed_ns":cycle_start.elapsed().as_nanos(),"acked":node.acked_through(),"last":last})
        )?;
        out.flush()?;
        ledger.flush()?;
        std::thread::sleep(Duration::from_millis(5));
    }
    // Stop prepares no new data; outstanding exact bytes are retained until ACK.
    let drained = node.deliver(Instant::now() + Duration::from_secs(10), |_| {})?;
    writeln!(
        out,
        "{}",
        json!({"event":"finished","acked":drained.acked_through,"last":last,"caught_up":drained.caught_up,"wall_ns":began.elapsed().as_nanos(),"recorded_batches":recorded})
    )?;
    out.flush()
}
