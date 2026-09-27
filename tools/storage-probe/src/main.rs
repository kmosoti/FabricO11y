//! Fixed S1 protocol runner; this is not an application query service.
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::log::{EventLog, same_record_contents};
use fabric_o11y::{Event, Payload, TenantId};
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Instant;
use storage_probe::{Mode, Query, Snapshot};

const EVENTS: u32 = 2048;
const BLOCK: usize = 64;

fn workload(name: &str, seed: u64) -> (Vec<Event>, i64) {
    let mut events: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed,
        events: EVENTS,
    })
    .collect();
    let base = events[0].event_time.0;
    for (index, event) in events.iter_mut().enumerate() {
        if name == "clustered_logs" {
            event.tenant = TenantId(index as u64 / 256 + 1);
        } else if name == "shuffled_logs" || name == "mixed" {
            event.tenant = TenantId((index as u64 * 17 + seed) % 1024 + 1);
        }
        if name == "shuffled_logs" {
            event.event_time.0 =
                base + ((index as u64 * 109 + seed) % u64::from(EVENTS)) as i64 * 1_000_000;
        }
        if name == "clustered_logs"
            || name == "shuffled_logs"
            || (name == "mixed" && index % 2 == 0)
        {
            let term = if index % 257 == 0 { "rare" } else { "normal" };
            event.payload = Payload::Log {
                body: format!(
                    "common service{} request{index} {term} {}",
                    index % 16,
                    "x".repeat(128)
                ),
            };
        }
    }
    (events, base)
}

fn queries(base: i64) -> Vec<Query> {
    (0..128)
        .map(|i| {
            let cycle = i / 8;
            let mut query = Query {
                start_ns: i64::MIN,
                end_ns: i64::MAX,
                tenant: None,
                token: None,
            };
            match i % 8 {
                1 | 6 => {
                    query.start_ns = base + ((cycle * 127) % 2048) as i64 * 1_000_000;
                    query.end_ns = query.start_ns + 31_000_000;
                    if i % 8 == 6 {
                        query.tenant = Some(cycle as u64 + 1);
                        query.token = Some("rare".into());
                    }
                }
                2 => query.tenant = Some(cycle as u64 + 1),
                3 => query.token = Some("rare".into()),
                4 => query.token = Some("absent".into()),
                5 => query.token = Some("common".into()),
                7 => {
                    query.start_ns = 1;
                    query.end_ns = 0;
                }
                _ => {}
            }
            query
        })
        .collect()
}

// Independent scalar evaluator: no block bounds, Bloom hash, or library predicate.
fn reference(events: &[Event], query: &Query) -> Vec<usize> {
    let mut positions = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if !(query.start_ns..=query.end_ns).contains(&event.event_time.0) {
            continue;
        }
        if let Some(tenant) = query.tenant {
            if tenant != event.tenant.0 {
                continue;
            }
        }
        if let Some(token) = &query.token {
            let Payload::Log { body } = &event.payload else {
                continue;
            };
            if !body.split_whitespace().any(|word| word == token) {
                continue;
            }
        }
        positions.push(index);
    }
    positions
}

fn peak_rss() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|s| s.starts_with("VmHWM:"))
                .and_then(|s| s.split_whitespace().nth(1))
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn run(output: &Path) -> io::Result<()> {
    fs::create_dir(output)?;
    let mut samples = BufWriter::new(File::create(output.join("queries.csv"))?);
    let mut builds = BufWriter::new(File::create(output.join("builds.csv"))?);
    writeln!(
        samples,
        "workload,seed,trial,query,case,mode,elapsed_ns,scanned_events,skipped_blocks,matches,expected_matches,equal"
    )?;
    writeln!(
        builds,
        "workload,seed,events,write_ns,replay_ns,build_ns,raw_bytes,summary_bytes,peak_rss_kib"
    )?;
    for name in ["gauge", "clustered_logs", "shuffled_logs", "mixed"] {
        for seed in [42, 43, 44] {
            let (source, base) = workload(name, seed);
            let path = output.join(format!("{name}-{seed}.log"));
            let mut log = EventLog::open(&path)?;
            let start = Instant::now();
            for event in &source {
                log.append(event)?;
            }
            let write_ns = start.elapsed().as_nanos();
            drop(log);
            let start = Instant::now();
            let mut log = EventLog::open(&path)?;
            let mut replayed = Vec::with_capacity(EVENTS as usize);
            log.replay(|event| {
                let expected = source
                    .get(replayed.len())
                    .ok_or_else(|| io::Error::other("extra replay row"))?;
                if !same_record_contents(expected, &event)? {
                    return Err(io::Error::other("replay mismatch"));
                }
                replayed.push(event);
                Ok(())
            })?;
            if replayed.len() != source.len() {
                return Err(io::Error::other("missing replay rows"));
            }
            let replay_ns = start.elapsed().as_nanos();
            drop(log);
            let corpus = queries(base);
            let expected: Vec<_> = corpus.iter().map(|q| reference(&source, q)).collect();
            drop(source);
            let start = Instant::now();
            let snapshot = Snapshot::new(replayed, NonZeroUsize::new(BLOCK).unwrap());
            let build_ns = start.elapsed().as_nanos();
            for (q, want) in corpus.iter().zip(&expected) {
                for mode in [Mode::Scan, Mode::Pruned] {
                    if snapshot.query(q, mode).positions != *want {
                        return Err(io::Error::other("warmup query mismatch"));
                    }
                }
            }
            for trial in 0..5 {
                for (index, (query, want)) in corpus.iter().zip(&expected).enumerate() {
                    let modes = if (trial + index) % 2 == 0 {
                        [Mode::Scan, Mode::Pruned]
                    } else {
                        [Mode::Pruned, Mode::Scan]
                    };
                    for mode in modes {
                        let start = Instant::now();
                        let result = black_box(snapshot.query(black_box(query), mode));
                        let elapsed = start.elapsed().as_nanos();
                        let equal = result.positions == *want;
                        if !equal {
                            return Err(io::Error::other("timed query mismatch"));
                        }
                        let label = match mode {
                            Mode::Scan => "scan",
                            Mode::Pruned => "pruned",
                        };
                        writeln!(
                            samples,
                            "{name},{seed},{trial},{index},{},{label},{elapsed},{},{},{},{},{}",
                            index % 8,
                            result.scanned_events,
                            result.skipped_blocks,
                            result.positions.len(),
                            want.len(),
                            u8::from(equal)
                        )?;
                    }
                }
            }
            writeln!(
                builds,
                "{name},{seed},{EVENTS},{write_ns},{replay_ns},{build_ns},{},{},{}",
                fs::metadata(&path)?.len(),
                snapshot.summary_bytes(),
                peak_rss()
            )?;
            eprintln!("completed {name} seed {seed}: {} replayed events", EVENTS);
        }
    }
    samples.flush()?;
    builds.flush()?;
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "run" {
        eprintln!("usage: storage-probe run FRESH_OUTPUT_DIRECTORY");
        return std::process::ExitCode::from(2);
    }
    match run(Path::new(&args[1])) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("S1 failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
