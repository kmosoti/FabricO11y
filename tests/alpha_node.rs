use fabric_o11y::alpha::host::Paths;
use fabric_o11y::alpha::journal::{Batch, Journal};
use fabric_o11y::alpha::node::{Config, Node, inspect};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::{AggregationTemporality, metric, number_data_point};
use prost::Message;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("alpha-node-test-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn host_paths(root: &Path) -> Paths {
    Paths {
        proc_stat: root.join("stat"),
        proc_meminfo: root.join("meminfo"),
        proc_diskstats: root.join("diskstats"),
        proc_net_dev: root.join("netdev"),
        boot_id: root.join("boot_id"),
        hostname: root.join("hostname"),
        filesystem: root.to_owned(),
    }
}

fn write_host(root: &Path, boot: &str, boot_time: u64, read_sectors: u64) {
    fs::write(
        root.join("stat"),
        format!("cpu 10 0 5 20 0 0 0 0 0 0\nbtime {boot_time}\n"),
    )
    .unwrap();
    fs::write(
        root.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
    )
    .unwrap();
    fs::write(
        root.join("diskstats"),
        format!("8 0 sda 1 0 {read_sectors} 0 1 0 8 0\n"),
    )
    .unwrap();
    fs::write(root.join("netdev"), "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n").unwrap();
    fs::write(root.join("boot_id"), boot).unwrap();
    fs::write(root.join("hostname"), "fixture-host\n").unwrap();
}

fn config(root: &Path, bytes: u64) -> Config {
    Config {
        spool: root.join("spool"),
        logs: vec![root.join("selected.log")],
        interval_s: 15,
        spool_bytes: bytes,
    }
}

fn batches(config: &Config) -> Vec<Batch> {
    let mut result = Vec::new();
    Journal::inspect(&config.spool, config.spool_bytes - 4096, |batch| {
        result.push(batch);
        Ok(())
    })
    .unwrap();
    result
}

fn disk_read(batch: &Batch) -> (u64, i64) {
    let request = ExportMetricsServiceRequest::decode(batch.metrics.as_slice()).unwrap();
    let metric = request.resource_metrics[0].scope_metrics[0]
        .metrics
        .iter()
        .find(|m| m.name == "system.disk.read.bytes")
        .unwrap();
    let Some(metric::Data::Sum(sum)) = metric.data.as_ref() else {
        panic!("not cumulative Sum")
    };
    let point = &sum.data_points[0];
    let Some(number_data_point::Value::AsInt(value)) = point.value else {
        panic!("not integer bytes")
    };
    (point.start_time_unix_nano, value)
}

#[test]
fn restart_rotation_truncation_incomplete_and_oversize_are_explicit() {
    let scratch = Scratch::new();
    let boot = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    write_host(&scratch.0, boot, 1000, 4);
    fs::write(scratch.path("selected.log"), b"one\npartial").unwrap();
    let cfg = config(&scratch.0, 1024 * 1024);
    let paths = host_paths(&scratch.0);
    let mut node = Node::open_with_paths(cfg.clone(), paths.clone()).unwrap();
    let first = node.collect_once().unwrap();
    assert_eq!(
        (first.metric_points, first.log_records, first.gaps),
        (11, 1, 0)
    );
    drop(node);
    let mut node = Node::open_with_paths(cfg.clone(), paths).unwrap();
    OpenOptions::new()
        .append(true)
        .open(scratch.path("selected.log"))
        .unwrap()
        .write_all(b" completed\n")
        .unwrap();
    assert_eq!(node.collect_once().unwrap().log_records, 1);
    fs::rename(scratch.path("selected.log"), scratch.path("rotated.log")).unwrap();
    fs::write(scratch.path("selected.log"), b"new\n").unwrap();
    let rotated = node.collect_once().unwrap();
    assert_eq!((rotated.log_records, rotated.gaps), (1, 1));
    fs::write(scratch.path("selected.log"), b"x\n").unwrap();
    let truncated = node.collect_once().unwrap();
    assert_eq!((truncated.log_records, truncated.gaps), (1, 1));
    OpenOptions::new()
        .append(true)
        .open(scratch.path("selected.log"))
        .unwrap()
        .write_all(&[vec![b'X'; 5000], b"\n".to_vec()].concat())
        .unwrap();
    let oversized = node.collect_once().unwrap();
    assert_eq!((oversized.log_records, oversized.gaps), (0, 1));
    let report = inspect(&cfg).unwrap();
    assert_eq!((report.batches, report.log_records, report.gaps), (5, 4, 3));
    let stored = batches(&cfg);
    assert_eq!(stored[0].cursors[0].offset, 4);
    assert_eq!(stored[1].cursors[0].offset, 22);
    assert_eq!(stored[4].cursors[0].offset, 5003);
}

#[test]
fn counters_preserve_start_then_reset_on_decrease_and_boot_change() {
    let scratch = Scratch::new();
    let boot_a = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let boot_b = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    write_host(&scratch.0, boot_a, 1000, 4);
    fs::write(scratch.path("selected.log"), b"").unwrap();
    let cfg = config(&scratch.0, 1024 * 1024);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    node.collect_once().unwrap();
    write_host(&scratch.0, boot_a, 1000, 8);
    node.collect_once().unwrap();
    let first_two = batches(&cfg);
    let decoded = ExportMetricsServiceRequest::decode(first_two[0].metrics.as_slice()).unwrap();
    let group = &decoded.resource_metrics[0];
    assert!(
        group
            .resource
            .as_ref()
            .unwrap()
            .attributes
            .iter()
            .any(|a| a.key == "host.boot.id")
    );
    let metrics = &group.scope_metrics[0].metrics;
    assert!(metrics.iter().any(|m| m.name == "system.memory.available"
        && m.unit == "By"
        && matches!(&m.data, Some(metric::Data::Gauge(_)))));
    assert!(metrics.iter().any(|m| m.name == "system.cpu.time"
        && m.unit == "s"
        && matches!(&m.data, Some(metric::Data::Sum(s)) if s.is_monotonic
            && s.aggregation_temporality == AggregationTemporality::Cumulative as i32
            && s.data_points[0].time_unix_nano > s.data_points[0].start_time_unix_nano)));
    let (start_a, value_a) = disk_read(&first_two[0]);
    assert_eq!(value_a, 2048);
    assert_eq!(disk_read(&first_two[1]), (start_a, 4096));
    drop(node);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    write_host(&scratch.0, boot_a, 1000, 1);
    node.collect_once().unwrap();
    let after_reset = batches(&cfg);
    let (start_reset, value_reset) = disk_read(&after_reset[2]);
    assert_eq!(value_reset, 512);
    assert!(start_reset >= start_a);
    write_host(&scratch.0, boot_b, 2000, 2);
    node.collect_once().unwrap();
    let after_boot = batches(&cfg);
    let (start_boot, value_boot) = disk_read(&after_boot[3]);
    assert_eq!(value_boot, 1024);
    assert!(start_boot >= start_reset);
    assert!(after_boot[3].metrics != after_boot[2].metrics);
}

#[test]
fn oversized_skip_is_committed_and_resumed_after_node_restart() {
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    let mut bytes = vec![b'X'; 256 * 1024 + 10];
    bytes.extend_from_slice(b"\nnormal\n");
    fs::write(scratch.path("selected.log"), &bytes).unwrap();
    let cfg = config(&scratch.0, 1024 * 1024);
    let mut first = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    let cycle = first.collect_once().unwrap();
    assert_eq!((cycle.log_records, cycle.gaps), (0, 1));
    drop(first);
    let saved = batches(&cfg);
    assert_eq!(saved[0].cursors[0].offset, 256 * 1024);
    assert!(saved[0].cursors[0].skipping_oversize);
    let mut resumed = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    assert_eq!(resumed.collect_once().unwrap().log_records, 1);
    let saved = batches(&cfg);
    assert_eq!(saved[1].cursors[0].offset, bytes.len() as u64);
    assert!(!saved[1].cursors[0].skipping_oversize);
    assert_eq!(inspect(&cfg).unwrap().log_records, 1);
}

#[test]
fn config_rejects_unused_keys_and_out_of_profile_limits() {
    let scratch = Scratch::new();
    let path = scratch.path("node.conf");
    fs::write(
        &path,
        format!(
            "spool_dir={}\nplugin=anything\n",
            scratch.path("spool").display()
        ),
    )
    .unwrap();
    assert!(Config::load(&path).is_err());
    fs::write(
        &path,
        format!(
            "spool_dir={}\nmetric_interval_s=0\n",
            scratch.path("spool").display()
        ),
    )
    .unwrap();
    assert!(Config::load(&path).is_err());
    fs::write(
        &path,
        format!(
            "spool_dir={}\nspool_bytes=8192\n",
            scratch.path("spool").display()
        ),
    )
    .unwrap();
    assert_eq!(Config::load(&path).unwrap().spool_bytes, 8192);
}

#[test]
fn source_failure_and_full_spool_leave_visible_gap_or_unknown_coverage() {
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    fs::write(scratch.path("selected.log"), b"ok\n").unwrap();
    let cfg = config(&scratch.0, 1024 * 1024);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    fs::remove_file(scratch.path("meminfo")).unwrap();
    let cycle = node.collect_once().unwrap();
    assert_eq!(
        (cycle.metric_points, cycle.log_records, cycle.gaps),
        (0, 1, 1)
    );
    fs::remove_file(scratch.path("selected.log")).unwrap();
    fs::create_dir(scratch.path("selected.log")).unwrap();
    assert_eq!(node.collect_once().unwrap().gaps, 2);
    drop(node);

    let full = Scratch::new();
    write_host(&full.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    fs::write(
        full.path("selected.log"),
        [vec![b'a'; 4096], b"\n".to_vec()].concat(),
    )
    .unwrap();
    let small = config(&full.0, 8192);
    let mut limited = Node::open_with_paths(small.clone(), host_paths(&full.0)).unwrap();
    assert!(limited.collect_once().is_err());
    assert_eq!(inspect(&small).unwrap().batches, 0);
    assert!(inspect(&small).unwrap().coverage_unknown);
    drop(limited);
    assert!(
        Node::open_with_paths(small, host_paths(&full.0))
            .unwrap()
            .collect_once()
            .is_err()
    );
}

#[test]
fn denied_host_and_log_sources_report_gaps_then_recover() {
    if unsafe { libc::geteuid() } == 0 {
        return; // The permission check needs an unprivileged test process.
    }
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    fs::write(scratch.path("selected.log"), b"after access returns\n").unwrap();
    for name in ["meminfo", "selected.log"] {
        fs::set_permissions(scratch.path(name), fs::Permissions::from_mode(0o000)).unwrap();
    }
    let cfg = config(&scratch.0, 1024 * 1024);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    let denied = node.collect_once().unwrap();
    assert_eq!(
        (denied.metric_points, denied.log_records, denied.gaps),
        (0, 0, 2)
    );
    assert_eq!(batches(&cfg)[0].cursors.len(), 0);
    for name in ["meminfo", "selected.log"] {
        fs::set_permissions(scratch.path(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let restored = node.collect_once().unwrap();
    assert_eq!(
        (restored.metric_points, restored.log_records, restored.gaps),
        (11, 1, 0)
    );
    assert_eq!(inspect(&cfg).unwrap().gaps, 2);
}

#[test]
fn public_config_uses_the_file_loaded_limits_before_opening_spool() {
    let scratch = Scratch::new();
    let base = config(&scratch.0, 1024 * 1024);
    for bytes in [0, 4095, u64::MAX, 256 * 1024 * 1024 + 1] {
        let mut invalid = base.clone();
        invalid.spool_bytes = bytes;
        assert_eq!(
            Node::open(invalid.clone()).err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            inspect(&invalid).err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
    let mut invalid = base.clone();
    invalid.interval_s = 0;
    assert_eq!(
        Node::open(invalid).err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );
    let mut invalid = base.clone();
    invalid.logs = (0..17)
        .map(|n| scratch.path(&format!("source-{n}.log")))
        .collect();
    assert_eq!(
        Node::open(invalid).err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(
        !base.spool.exists(),
        "invalid public config must not create state"
    );

    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    fs::write(scratch.path("selected.log"), b"once\n").unwrap();
    let mut duplicate = base.clone();
    duplicate.logs.push(scratch.path("selected.log"));
    let mut node = Node::open_with_paths(duplicate, host_paths(&scratch.0)).unwrap();
    assert_eq!(node.collect_once().unwrap().log_records, 1);
}

#[test]
fn all_three_malformed_sources_commit_gaps_and_later_good_lines() {
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    let mut cfg = config(&scratch.0, 1024 * 1024);
    cfg.logs = (0..3)
        .map(|n| scratch.path(&format!("bad-{n}.log")))
        .collect();
    for path in &cfg.logs {
        fs::write(path, [b"\xff\n".repeat(8), b"good\n".to_vec()].concat()).unwrap();
    }
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    let first = node.collect_once().unwrap();
    assert_eq!(
        (first.metric_points, first.log_records, first.gaps),
        (11, 0, 24)
    );
    drop(node);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    let second = node.collect_once().unwrap();
    assert_eq!(
        (second.metric_points, second.log_records, second.gaps),
        (11, 3, 0)
    );
    let report = inspect(&cfg).unwrap();
    assert_eq!(
        (report.batches, report.log_records, report.gaps),
        (2, 3, 24)
    );
    assert!(!report.coverage_unknown);
    let mut bodies = Vec::new();
    for batch in batches(&cfg) {
        if batch.logs.is_empty() {
            continue;
        }
        let request = ExportLogsServiceRequest::decode(batch.logs.as_slice()).unwrap();
        for group in request.resource_logs {
            for scope in group.scope_logs {
                for record in scope.log_records {
                    let Some(any_value::Value::StringValue(body)) = record.body.unwrap().value
                    else {
                        panic!("nonstring body")
                    };
                    bodies.push(body);
                }
            }
        }
    }
    assert_eq!(bodies, vec!["good", "good", "good"]);
}

#[test]
fn maximum_source_gap_count_fits_one_bounded_batch() {
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    let mut cfg = config(&scratch.0, 1024 * 1024);
    cfg.logs = (0..16)
        .map(|n| scratch.path(&format!("bad-{n}.log")))
        .collect();
    for path in &cfg.logs {
        fs::write(path, [b"\xff\n".repeat(8), b"good\n".to_vec()].concat()).unwrap();
    }
    fs::remove_file(scratch.path("meminfo")).unwrap();
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    let first = node.collect_once().unwrap();
    assert_eq!(
        (first.metric_points, first.log_records, first.gaps),
        (0, 0, 129)
    );
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    let second = node.collect_once().unwrap();
    assert_eq!(
        (second.metric_points, second.log_records, second.gaps),
        (11, 16, 0)
    );
    assert!(!inspect(&cfg).unwrap().coverage_unknown);
}

#[test]
fn same_inode_rewrite_with_longer_new_file_reports_gap_and_reads_from_zero() {
    let scratch = Scratch::new();
    write_host(&scratch.0, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 1000, 4);
    let path = scratch.path("selected.log");
    let old = (0..10).map(|n| format!("old-{n:02}\n")).collect::<String>();
    fs::write(&path, old).unwrap();
    let cfg = config(&scratch.0, 1024 * 1024);
    let mut node = Node::open_with_paths(cfg.clone(), host_paths(&scratch.0)).unwrap();
    assert_eq!(node.collect_once().unwrap().log_records, 10);
    let newer = (0..20).map(|n| format!("new-{n:02}\n")).collect::<String>();
    fs::write(&path, newer).unwrap(); // same inode, length now beyond old offset
    let cycle = node.collect_once().unwrap();
    assert_eq!(
        (cycle.metric_points, cycle.log_records, cycle.gaps),
        (11, 20, 1)
    );
    assert_eq!(inspect(&cfg).unwrap().log_records, 30);
    let batch = batches(&cfg).pop().unwrap();
    let request = ExportLogsServiceRequest::decode(batch.logs.as_slice()).unwrap();
    let records = &request.resource_logs[0].scope_logs[0].log_records;
    let bodies: Vec<_> = records
        .iter()
        .map(
            |record| match record.body.as_ref().unwrap().value.as_ref().unwrap() {
                any_value::Value::StringValue(body) => body.as_str(),
                _ => panic!("nonstring body"),
            },
        )
        .collect();
    assert_eq!(bodies.first(), Some(&"new-00"));
    assert_eq!(bodies.last(), Some(&"new-19"));
}
