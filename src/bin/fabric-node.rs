use fabric_o11y::alpha::node::{Config, Node};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_signal: libc::c_int) {
    // An atomic store is async-signal-safe; the loop checks it between cycles.
    STOP.store(true, Ordering::SeqCst);
}

/// SIGTERM and SIGINT end `run` after the current cycle, so a service stop never
/// interrupts an append. A kill during an append is still recoverable on reopen.
fn install_stop_handler() -> std::io::Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only performs an atomic store.
        let previous = unsafe { libc::signal(signal, request_stop as *const () as libc::sighandler_t) };
        if previous == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
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
        let config = Config::load(config_path)?;
        let interval = Duration::from_secs(config.interval_s);
        let mut node = Node::open(config)?;
        if mode == "run" {
            install_stop_handler()?;
        }
        loop {
            let started = Instant::now();
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
            // Sleep to the next interval boundary in short slices so a stop
            // request is honoured promptly and cycle time does not add drift.
            let deadline = started + interval;
            while !STOP.load(Ordering::SeqCst) {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                std::thread::sleep((deadline - now).min(Duration::from_millis(100)));
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
