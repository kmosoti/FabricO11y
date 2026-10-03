use fabric_o11y::spindle::runtime::{Attempt, Config, Spindle};
use fabric_o11y::spindle::sender::Delivery;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// How often `run` reads configured logs between metric samples.
const LOG_POLL: Duration = Duration::from_secs(1);
/// How often `run` asks the server for configuration (ADR-0014).
const CONFIG_POLL: Duration = Duration::from_secs(5);

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
        "delivery sequence={} sha256={} status={status}{through} elapsed_us={}",
        attempt.sequence, attempt.sha256, attempt.elapsed_us
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
        let traces_listen = config.traces_listen;
        let mut node = Spindle::open(config)?;
        // The loopback trace endpoint (ADR-0025), in `run` mode only.
        let traces = match traces_listen {
            Some(addr) if mode == "run" => {
                let (bound, rx) = fabric_o11y::spindle::otlp::start(addr)?;
                println!("traces listening={bound}");
                Some(rx)
            }
            _ => None,
        };
        let mut carried = None;
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut backoff = MIN_BACKOFF;
        let mut last_error: Option<String> = None;
        // Whether the last delivery attempt left nothing unacknowledged. Catch-up
        // passes wait for it, so collection never outruns what the server accepts.
        let mut delivered = true;
        let mut retry_at = Instant::now();
        let mut next_metrics = Instant::now();
        let mut next_logs = Instant::now() + LOG_POLL;
        let mut next_config = Instant::now();
        let mut last_config_error: Option<String> = None;
        loop {
            if let Some(rx) = &traces
                && fabric_o11y::spindle::otlp::drain(&mut node, rx, &mut carried)
                && last_error.is_none()
            {
                retry_at = Instant::now();
            }
            let now = Instant::now();
            if mode == "run" && now >= next_config {
                next_config = now + CONFIG_POLL;
                let poll = node.poll_config()?;
                if poll.changed {
                    // Confirm at once so the server sees the applied revision
                    // without waiting a full poll interval.
                    next_config = now;
                    println!(
                        "config revision={} paused={} interval_s={}",
                        node.applied_revision(),
                        node.paused(),
                        node.interval_s()
                    );
                }
                if poll.error != last_config_error {
                    if let Some(error) = &poll.error {
                        eprintln!("fabric-node: configuration: {error}");
                    }
                    last_config_error = poll.error;
                }
            }
            // Metrics on their interval boundary; logs every LOG_POLL so a
            // line is committed and sent within about a second. Nothing is
            // collected while the server has paused this node.
            let cycle = if node.paused() {
                None
            } else if now >= next_metrics {
                let interval = Duration::from_secs(node.interval_s());
                while next_metrics <= now {
                    next_metrics += interval;
                }
                next_logs = now + LOG_POLL;
                Some(node.collect_once()?)
            } else if now >= next_logs {
                next_logs = now + LOG_POLL;
                node.collect_logs()?
            } else {
                None
            };
            // A pass that left unread log bytes is followed by another as soon as
            // delivery has caught up, so collection keeps pace with delivery rather
            // than with one pass per second (ADR-0025).
            let catch_up = delivered && cycle.as_ref().is_some_and(|c| c.log_backlog_bytes > 0);
            if catch_up {
                next_logs = Instant::now();
                // Deliver at once: a retry time left from an idle turn would
                // otherwise hold this pass until that turn's deadline.
                if last_error.is_none() {
                    retry_at = Instant::now();
                }
            }
            if let Some(cycle) = cycle {
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
            }
            if mode == "collect" {
                break;
            }
            // Until the next poll: deliver while batches are pending, back off
            // after a failed attempt, and honour a stop request within 100 ms.
            let deadline = if catch_up {
                // Deliver until caught up (bounded), then read the backlog again.
                Instant::now() + Duration::from_secs(2)
            } else {
                next_metrics.min(next_logs).min(next_config)
            };
            while !STOP.load(Ordering::SeqCst) && Instant::now() < deadline {
                if let Some(rx) = &traces
                    && fabric_o11y::spindle::otlp::drain(&mut node, rx, &mut carried)
                    && last_error.is_none()
                {
                    // A trace Batch was committed: send it now, not at the next poll.
                    retry_at = Instant::now();
                }
                let now = Instant::now();
                // With the trace endpoint on, deliver in short slices so a waiting
                // exporter is answered within about 200 ms of its commit turn.
                let slice = if traces.is_some() {
                    deadline.min(now + Duration::from_millis(200))
                } else {
                    deadline
                };
                if now >= retry_at {
                    // Stdout is line buffered: each line is written before
                    // the ACK it reports is persisted.
                    let report = node.deliver(slice, |a| println!("{}", delivery_line(a)))?;
                    match report.error {
                        Some(error) => {
                            delivered = false;
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
                            delivered = report.caught_up;
                            if !report.caught_up {
                                continue;
                            }
                            if catch_up {
                                break;
                            }
                            // Nothing pending until the next poll commits.
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
