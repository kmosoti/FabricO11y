use fabric_adapter_linux::operational_log::OperationalLog;
use fabric_o11y::spindle::runtime::{Attempt, Config, Spindle};
use fabric_o11y::spindle::sender::Delivery;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
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
const DIAGNOSTIC_POLL: Duration = Duration::from_secs(15);

struct Diagnostics {
    log: OperationalLog,
    next_sample: Instant,
    failed: bool,
}

impl Diagnostics {
    fn report(&mut self, result: std::io::Result<()>) {
        let failed = result.is_err();
        if failed != self.failed {
            eprintln!(
                "fabric-node: diagnostics {}",
                if failed { "unavailable" } else { "resumed" }
            );
            self.failed = failed;
        }
    }

    fn event(&mut self, event: &str) {
        let result = self.log.event(event);
        self.report(result);
    }

    fn sample_due(&mut self) {
        let now = Instant::now();
        if now >= self.next_sample {
            self.next_sample = now + DIAGNOSTIC_POLL;
            let result = self.log.sample();
            self.report(result);
        }
    }
}

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

// Timing output is opt-in and never written to the node's collected diagnostic
// file. The bounded runtime buffer reports loss explicitly instead of inventing
// timestamps or retaining an unbounded stream during an outage.
fn timing_lines(node: &mut Spindle) {
    let (events, dropped) = node.take_timing_events();
    for event in events {
        let identity: String = event.node_id.iter().map(|b| format!("{b:02x}")).collect();
        let wall = event
            .stamp
            .unix_ns
            .map_or_else(|| "unmeasured".into(), |n| n.to_string());
        let boot = event
            .stamp
            .boot_monotonic_ns
            .map_or_else(|| "unmeasured".into(), |n| n.to_string());
        println!(
            "timing process_id={} node_id={identity} generation={} sequence={} stage={} unix_ns={wall} boot_monotonic_ns={boot} monotonic_before_ns={} monotonic_after_ns={}",
            std::process::id(),
            event.generation,
            event.sequence,
            event.stage,
            event.stamp.monotonic_before_ns,
            event.stamp.monotonic_after_ns
        );
    }
    if dropped != 0 {
        println!("timing dropped_events={dropped}");
    }
}

fn main() -> ExitCode {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let timing_enabled = args.last().is_some_and(|a| a == "--timing-events");
    if timing_enabled {
        args.pop();
    }
    let (mode, config_path, server_log) = match args.as_slice() {
        [mode, config] if mode == "collect" || mode == "run" => (mode, config, None),
        [mode, config, flag, path] if mode == "run" && flag == "--server-log" => {
            (mode, config, Some(PathBuf::from(path)))
        }
        _ => {
            eprintln!(
                "usage: fabric-node <collect|run> <CONFIG_PATH> [--server-log ABS_PATH (run only)] [--timing-events]"
            );
            return ExitCode::from(2);
        }
    };
    let mut diagnostics = None;
    let managed = server_log.is_some();
    let result = (|| {
        // Installed before the spool replay, so a stop during startup also
        // ends with exit 0 instead of death by signal.
        if mode == "run" {
            install_stop_handler()?;
        }
        let config = Config::load(config_path)?;
        let traces_listen = config.traces_listen;
        std::fs::create_dir_all(&config.spool)?;
        let log = OperationalLog::open(&config.spool.join("diagnostics"), "spindle")?;
        let mut local_logs = vec![log.path().to_path_buf()];
        diagnostics = Some(Diagnostics {
            log,
            next_sample: Instant::now(),
            failed: false,
        });
        let diagnostics = diagnostics.as_mut().expect("diagnostics opened");
        diagnostics.event("starting");
        diagnostics.sample_due();
        if let Some(path) = server_log {
            if !path.is_absolute() || path.as_os_str().len() > 240 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid --server-log absolute path",
                ));
            }
            // A companion path must already name a regular readable source.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&path)?;
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--server-log must name a regular file",
                ));
            }
            local_logs.push(path);
        }
        let mut node = Spindle::open_with_local_logs(config, local_logs)?;
        if timing_enabled {
            node.enable_timing_events();
        }
        // A companion is launched only after its local listener is ready.
        // Refuse invalid TLS/authentication at startup instead of leaving an
        // apparently healthy server with a permanently disconnected collector.
        // Ordinary edge nodes still retain/retry through unavailable servers.
        if managed && let Some(error) = node.poll_config()?.error {
            return Err(std::io::Error::other(format!(
                "dedicated Spindle startup configuration handshake failed: {error}"
            )));
        }
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
        diagnostics.event("started");
        loop {
            diagnostics.sample_due();
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
                    if poll.error.is_some() && last_config_error.is_none() {
                        diagnostics.event("configuration_error");
                    } else if poll.error.is_none() && last_config_error.is_some() {
                        diagnostics.event("configuration_resumed");
                    }
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
            timing_lines(&mut node);
            if mode == "collect" {
                break;
            }
            // Until the next poll: deliver while batches are pending, back off
            // after a failed attempt, and honour a stop request within 100 ms.
            let deadline = if catch_up {
                // Deliver until caught up (bounded), then read the backlog again.
                Instant::now() + Duration::from_secs(2)
            } else {
                next_metrics
                    .min(next_logs)
                    .min(next_config)
                    .min(diagnostics.next_sample)
            };
            while !STOP.load(Ordering::SeqCst) && Instant::now() < deadline {
                diagnostics.sample_due();
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
                    timing_lines(&mut node);
                    match report.error {
                        Some(error) => {
                            delivered = false;
                            if last_error.is_none() {
                                diagnostics.event("delivery_error");
                            }
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
                                diagnostics.event("delivery_resumed");
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
    if let Some(diagnostics) = &mut diagnostics {
        if result.is_err() {
            diagnostics.event("error");
        }
        // This final marker is collected on a later start, if retained.
        diagnostics.event("stopping");
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fabric-node: {error}");
            ExitCode::FAILURE
        }
    }
}
