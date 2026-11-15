//! Deterministic synthetic workload generator run inside `agent.slice --collect`
//! to validate FabricO11y live memory, `/data/tmp` write velocity, and lineage attribution.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

struct Args {
    rss_mb: u64,
    disk_mb: u64,
    dir: PathBuf,
    hold_ms: u64,
}

fn parse_args() -> Args {
    let mut rss_mb = 512u64;
    let mut disk_mb = 200u64;
    let mut dir = PathBuf::from("/data/tmp/fabric-synth-load");
    let mut hold_ms = 10_000u64;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rss-mb" => {
                if let Some(v) = it.next() {
                    rss_mb = v.parse().unwrap_or(rss_mb);
                }
            }
            "--disk-mb" => {
                if let Some(v) = it.next() {
                    disk_mb = v.parse().unwrap_or(disk_mb);
                }
            }
            "--dir" => {
                if let Some(v) = it.next() {
                    dir = PathBuf::from(v);
                }
            }
            "--hold-ms" => {
                if let Some(v) = it.next() {
                    hold_ms = v.parse().unwrap_or(hold_ms);
                }
            }
            _ => {}
        }
    }

    Args {
        rss_mb,
        disk_mb,
        dir,
        hold_ms,
    }
}

fn main() {
    let args = parse_args();
    if args.dir.starts_with("/tmp") || args.dir.starts_with("/dev/shm") {
        eprintln!("refusing RAM-backed scratch dir: {}", args.dir.display());
        std::process::exit(2);
    }

    fs::create_dir_all(&args.dir).expect("create synthetic load directory");
    let payload_path = args.dir.join("payload.bin");
    let mut file = File::create(&payload_path).expect("create synthetic payload.bin");
    let chunk = vec![0xA5u8; 1024 * 1024];
    for _ in 0..args.disk_mb {
        file.write_all(&chunk).expect("write payload chunk");
    }
    file.sync_data().expect("sync synthetic payload.bin");

    // Fault real anonymous physical pages (one non-zero byte every 4 KiB page).
    let total_bytes = (args.rss_mb as usize) * 1024 * 1024;
    let mut mem = vec![0u8; total_bytes];
    for idx in (0..total_bytes).step_by(4096) {
        mem[idx] = ((idx >> 12) as u8) | 1;
    }
    std::hint::black_box(&mem);

    eprintln!(
        "synthetic_agent_load ready pid={} rss_mb={} disk_mb={} dir={}",
        std::process::id(),
        args.rss_mb,
        args.disk_mb,
        args.dir.display()
    );

    let deadline = Instant::now() + Duration::from_millis(args.hold_ms);
    while Instant::now() < deadline {
        std::hint::black_box(&mem[0]);
        thread::sleep(Duration::from_millis(200));
    }

    drop(file);
    let _ = fs::remove_dir_all(&args.dir);
}
