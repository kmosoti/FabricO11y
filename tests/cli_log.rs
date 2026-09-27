use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::log::EventLog;
use fabric_o11y::{Event, Payload};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "fabric-o11y-cli-log-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create temporary directory: {error}"),
            }
        }
        panic!("could not allocate a unique temporary directory");
    }

    fn log_path(&self) -> PathBuf {
        self.0.join("events.log")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, seed: u64, events: u32) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fabric_o11y"))
        .arg("write")
        .arg(path)
        .arg(seed.to_string())
        .arg(events.to_string())
        .output()
        .expect("write command should start")
}

fn replay(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fabric_o11y"))
        .arg("replay")
        .arg(path)
        .output()
        .expect("replay command should start")
}

fn read_events(path: &Path) -> Vec<Event> {
    let mut log = EventLog::open(path).unwrap();
    let mut actual = Vec::new();
    let count = log
        .replay(|event| {
            actual.push(event);
            Ok(())
        })
        .unwrap();
    assert_eq!(count, actual.len());
    actual
}

fn assert_committed(output: &Output, ids: &[u64]) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("committed event "))
        .collect();
    let expected: Vec<String> = ids
        .iter()
        .map(|id| format!("committed event {id}"))
        .collect();
    assert_eq!(lines, expected);
}

#[test]
fn write_then_restart_resumes_only_the_missing_suffix() {
    let temp = TempDir::new();
    let path = temp.log_path();

    let first_config = WorkloadConfig {
        seed: 17,
        events: 2,
    };
    let first_expected: Vec<Event> = EventGenerator::new(first_config).collect();
    assert_committed(
        &write(&path, first_config.seed, first_config.events),
        &[1, 2],
    );
    assert_eq!(read_events(&path), first_expected);

    let first_replay = replay(&path);
    assert!(first_replay.status.success());
    assert!(!first_replay.stdout.is_empty());
    assert_eq!(replay(&path).stdout, first_replay.stdout);

    let resumed_config = WorkloadConfig {
        seed: 17,
        events: 3,
    };
    let expected: Vec<Event> = EventGenerator::new(resumed_config).collect();
    assert_committed(
        &write(&path, resumed_config.seed, resumed_config.events),
        &[3],
    );
    assert_eq!(read_events(&path), expected);

    let second_replay = replay(&path);
    assert!(second_replay.status.success());
    assert_ne!(second_replay.stdout, first_replay.stdout);
    assert_eq!(replay(&path).stdout, second_replay.stdout);

    let before_rerun = fs::read(&path).unwrap();
    assert_committed(
        &write(&path, resumed_config.seed, resumed_config.events),
        &[],
    );
    assert_eq!(fs::read(&path).unwrap(), before_rerun);

    let different_seed = (18..100)
        .find(|seed| {
            EventGenerator::new(WorkloadConfig {
                seed: *seed,
                events: 2,
            })
            .collect::<Vec<_>>()
                != first_expected
        })
        .unwrap();
    for output in [write(&path, different_seed, 3), write(&path, 17, 2)] {
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("committed event "));
        assert_eq!(fs::read(&path).unwrap(), before_rerun);
    }
}

#[test]
fn resume_rejects_a_record_with_different_float_bits() {
    let temp = TempDir::new();
    let path = temp.log_path();
    let config = WorkloadConfig {
        seed: 98,
        events: 1,
    };
    let mut event = EventGenerator::new(config).next().unwrap();
    let Payload::Gauge { value, .. } = &mut event.payload else {
        panic!("the synthetic workload should produce a gauge");
    };
    assert_eq!(value.to_bits(), 0.0_f64.to_bits());
    *value = -0.0;
    let mut log = EventLog::open(&path).unwrap();
    log.append(&event).unwrap();
    drop(log);
    let before = fs::read(&path).unwrap();

    let output = write(&path, config.seed, config.events);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a prefix"));
    assert_eq!(fs::read(&path).unwrap(), before);
}
