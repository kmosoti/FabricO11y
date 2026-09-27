use fabric_o11y::alpha::node::{Attempt, Config, Node};
use fabric_o11y::alpha::sender::Delivery;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

extern "C" fn request_stop(_signal: libc::c_int) {
    // An atomic store is async-signal-safe; the loop checks it between cycles.
    STOP.store(true, Ordering::SeqCst);
}

/// SIGTERM and SIGINT end `run` after the current cycle, so a service stop never
/// interrupts an append. A kill during an append is still recoverable on reopen.
fn install_stop_handler() -> std::io::Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only performs an atomic store.
        let previous =
            unsafe { libc::signal(signal, request_stop as *const () as libc::sighandler_t) };
        if previous == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// One line per send attempt, so delivery progress is visible to operators.
fn delivery_line(attempt: &Attempt) -> String {
    let (status, through) = match &attempt.outcome {
        Delivery::Ack(t) => ("ack", Some(*t)),
        Delivery::Conflict(t) => ("conflict", Some(*t)),
        Delivery::Gap(t) => ("gap", Some(*t)),
        Delivery::Rejected(why) if why.starts_with("HTTP 401") => ("unauthorized", None),
        Delivery::Rejected(why) if why.starts_with("HTTP 403") => ("forbidden", None),
        Delivery::Rejected(why) if why.starts_with("HTTP 413") => ("too_large", None),
        Delivery::Rejected(_) => ("bad_request", None),
        Delivery::Retry(why) if why.starts_with("HTTP 503") => ("unavailable", None),
        Delivery::Retry(_) => ("no_response", None),
    };
    let through = through.map_or(String::new(), |t| format!(" committed_through={t}"));
    format!(
        "delivery sequence={} sha256={} status={status}{through}",
        attempt.sequence, attempt.sha256
    )
}

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
        // Installed before the spool replay, so a stop during startup also
        // ends with exit 0 instead of death by signal.
        if mode == "run" {
            install_stop_handler()?;
        }
        let config = Config::load(config_path)?;
        let interval = Duration::from_secs(config.interval_s);
        let mut node = Node::open(config)?;
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut backoff = MIN_BACKOFF;
        let mut last_error: Option<String> = None;
        loop {
            let started = Instant::now();
            let cycle = node.collect_once()?;
            println!(
                "batch={} metrics={} logs={} gaps={} spool_bytes={} log_backlog_bytes={} acked_through={}",
                cycle.batch_sequence,
                cycle.metric_points,
                cycle.log_records,
                cycle.gaps,
                cycle.spool_bytes,
                cycle.log_backlog_bytes,
                node.acked_through()
            );
            if mode == "collect" {
                break;
            }
            // Until the next interval boundary: deliver while batches are
            // pending, back off after a failed attempt, and honour a stop
            // request within 100 ms. Cycle time does not add drift.
            let deadline = started + interval;
            let mut retry_at = Instant::now();
            while !STOP.load(Ordering::SeqCst) && Instant::now() < deadline {
                let now = Instant::now();
                if now >= retry_at {
                    // Stdout is line buffered: each line is written before
                    // the ACK it reports is persisted.
                    let report = node.deliver(deadline.min(now + Duration::from_secs(1)), |a| {
                        println!("{}", delivery_line(a))
                    })?;
                    match report.error {
                        Some(error) => {
                            if last_error.as_deref() != Some(error.as_str()) {
                                eprintln!("fabric-node: delivery: {error}");
                            }
                            last_error = Some(error);
                            retry_at = Instant::now() + backoff;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                        None => {
                            if last_error.take().is_some() {
                                eprintln!("fabric-node: delivery resumed");
                            }
                            backoff = MIN_BACKOFF;
                            if !report.caught_up {
                                continue;
                            }
                            retry_at = deadline;
                        }
                    }
                }
                let now = Instant::now();
                let wake = retry_at.min(deadline);
                if wake > now {
                    std::thread::sleep((wake - now).min(Duration::from_millis(100)));
                }
            }
            if STOP.load(Ordering::SeqCst) {
                break;
            }
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
