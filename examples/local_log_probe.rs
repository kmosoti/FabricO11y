//! Stage 6 local log baseline. See docs/experiments/benchmarks/local-log-stage6.md.

use fabric_o11y::Event;
use fabric_o11y::buffer::EventBuffer;
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::log::{EventLog, same_record_contents};
use std::fs::{self, OpenOptions};
use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

const CAPACITY: usize = 256;
const BATCH_SIZE: usize = 64;
const CSV_HEADER: &str = "phase,event_index,elapsed_ns,events,full_rejections,file_bytes,peak_rss_kib,alloc_calls,alloc_requested_bytes";

#[cfg(feature = "stage6-alloc-probe")]
mod alloc_probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

    struct CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    // Count successful alloc, alloc_zeroed, and realloc requests. For realloc,
    // count the new requested size. Deallocation is outside these two metrics.
    fn record(ptr: *mut u8, requested_bytes: usize) {
        if !ptr.is_null() && ENABLED.load(Ordering::SeqCst) {
            CALLS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(requested_bytes as u64, Ordering::Relaxed);
        }
    }

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            record(ptr, layout.size());
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            record(ptr, layout.size());
            ptr
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let replacement = unsafe { System.realloc(ptr, layout, new_size) };
            record(replacement, new_size);
            replacement
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) };
        }
    }

    pub struct Guard {
        counts: Option<(u64, u64)>,
    }

    impl Guard {
        pub fn start() -> Self {
            CALLS.store(0, Ordering::Relaxed);
            REQUESTED_BYTES.store(0, Ordering::Relaxed);
            ENABLED.store(true, Ordering::SeqCst);
            Self { counts: None }
        }

        pub fn finish_event(&mut self, index: usize, total: usize) {
            if index == total {
                self.stop();
            }
        }

        pub fn stop(&mut self) {
            if self.counts.is_none() {
                ENABLED.store(false, Ordering::SeqCst);
                self.counts = Some((
                    CALLS.load(Ordering::Relaxed),
                    REQUESTED_BYTES.load(Ordering::Relaxed),
                ));
            }
        }

        pub fn columns(&self) -> (String, String) {
            let (calls, bytes) = self.counts.expect("allocation counting has stopped");
            (calls.to_string(), bytes.to_string())
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // An append error exits via `?`; suppress counting before stderr.
            ENABLED.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(not(feature = "stage6-alloc-probe"))]
mod alloc_probe {
    pub struct Guard;

    impl Guard {
        pub fn start() -> Self {
            Self
        }

        pub fn finish_event(&mut self, _index: usize, _total: usize) {}

        pub fn stop(&mut self) {}

        pub fn columns(&self) -> (String, String) {
            (String::new(), String::new())
        }
    }
}

fn append_batch(
    log: &mut EventLog,
    batch: Vec<Event>,
    pipeline_start: Instant,
    append_ns: &mut Vec<u128>,
    last_append_ns: &mut u128,
    total_events: usize,
    allocations: &mut alloc_probe::Guard,
) -> io::Result<()> {
    for event in batch {
        // Append borrows this event. On an I/O error the whole trial aborts;
        // the reproducible seed is the independent source for a fresh trial.
        let append_start = Instant::now();
        log.append(&event)?;
        let append_end = Instant::now();
        allocations.finish_event(append_ns.len() + 1, total_events);
        append_ns.push(append_end.duration_since(append_start).as_nanos());
        *last_append_ns = append_end.duration_since(pipeline_start).as_nanos();
    }
    Ok(())
}

fn peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn write(path: &Path, config: WorkloadConfig) -> io::Result<()> {
    // EventLog::open creates a missing path, so create_new enforces a fresh
    // trial even when an existing directory entry is a dangling symlink.
    drop(OpenOptions::new().write(true).create_new(true).open(path)?);
    let mut log = EventLog::open(path)?;
    let mut buffer = EventBuffer::new(NonZeroUsize::new(CAPACITY).unwrap());
    let batch_size = NonZeroUsize::new(BATCH_SIZE).unwrap();
    let mut append_ns = Vec::with_capacity(config.events as usize);
    let mut full_rejections = 0_u64;
    let mut last_append_ns = 0_u128;
    let generator = EventGenerator::new(config);

    // This begins before the first generator step. For zero events, there is
    // no last append; the reported ingest duration is zero by convention.
    let mut allocations = alloc_probe::Guard::start();
    let pipeline_start = Instant::now();
    for event in generator {
        if let Err(event) = buffer.try_push(event) {
            full_rejections += 1;
            append_batch(
                &mut log,
                buffer.take_batch(batch_size),
                pipeline_start,
                &mut append_ns,
                &mut last_append_ns,
                config.events as usize,
                &mut allocations,
            )?;
            buffer
                .try_push(event)
                .expect("a drained batch made room for the rejected event");
        }
    }
    while !buffer.is_empty() {
        append_batch(
            &mut log,
            buffer.take_batch(batch_size),
            pipeline_start,
            &mut append_ns,
            &mut last_append_ns,
            config.events as usize,
            &mut allocations,
        )?;
    }
    if config.events == 0 {
        allocations.stop();
    }

    if append_ns.len() != config.events as usize {
        return Err(io::Error::other(
            "ingest count differs from requested count",
        ));
    }
    drop(log);
    let file_bytes = fs::metadata(path)?.len();
    let peak_rss = peak_rss_kib()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let (alloc_calls, alloc_requested_bytes) = allocations.columns();

    // Output is deliberately outside the timed pipeline interval.
    println!("{CSV_HEADER}");
    for (index, elapsed_ns) in append_ns.into_iter().enumerate() {
        println!("append,{},{elapsed_ns},,,,,,", index + 1);
    }
    println!(
        "ingest,,{last_append_ns},{},{full_rejections},{file_bytes},{peak_rss},{alloc_calls},{alloc_requested_bytes}",
        config.events
    );
    Ok(())
}

fn verify(path: &Path, config: WorkloadConfig) -> io::Result<()> {
    // EventLog::open can create a missing file, which would make a zero-event
    // verification appear to pass without a preceding write trial.
    fs::metadata(path)?;
    let open_start = Instant::now();
    let mut log = EventLog::open(path)?;
    let open_ns = open_start.elapsed().as_nanos();

    let mut expected = EventGenerator::new(config);
    let replay_start = Instant::now();
    let replayed = log.replay(|stored| {
        let Some(source_event) = expected.next() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "log contains more events than requested",
            ));
        };
        if same_record_contents(&source_event, &stored)? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("record {} differs from seeded generator", source_event.id.0),
            ))
        }
    })?;
    let replay_ns = replay_start.elapsed().as_nanos();
    if replayed != config.events as usize || expected.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("replayed {replayed} events; expected {}", config.events),
        ));
    }
    drop(log);
    let file_bytes = fs::metadata(path)?.len();

    println!("{CSV_HEADER}");
    println!("open,,{open_ns},{replayed},,{file_bytes},,,");
    println!("replay,,{replay_ns},{replayed},,{file_bytes},,,");
    Ok(())
}

fn parse_config(seed: &str, events: &str) -> Option<WorkloadConfig> {
    Some(WorkloadConfig {
        seed: seed.parse().ok()?,
        events: events.parse().ok()?,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let operation = match args.as_slice() {
        [mode, path, seed, events] if mode == "write" || mode == "verify" => {
            let Some(config) = parse_config(seed, events) else {
                eprintln!("usage: local_log_probe <write|verify> <PATH> <SEED:u64> <EVENTS:u32>");
                return ExitCode::from(2);
            };
            if mode == "write" {
                write(Path::new(path), config)
            } else {
                verify(Path::new(path), config)
            }
        }
        _ => {
            eprintln!("usage: local_log_probe <write|verify> <PATH> <SEED:u64> <EVENTS:u32>");
            return ExitCode::from(2);
        }
    };
    match operation {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("local log probe failed: {error}");
            ExitCode::FAILURE
        }
    }
}
