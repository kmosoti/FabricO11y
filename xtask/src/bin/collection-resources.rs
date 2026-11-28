//! Outage saturation and recovery against the real SDK and Spindle process.
use fabric::{
    observation::{Data, Level},
    sdk::{Admission, Client, Config},
    spindle::{self, custody::Store},
};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};
use xtask::collection_lab::{RESOURCE_WORKLOADS, config, daemon, rss_bytes};

fn parse_args(
    args: &[String],
) -> Result<
    (
        Vec<xtask::collection_lab::ResourceWorkload>,
        Option<PathBuf>,
    ),
    String,
> {
    let mut case = None;
    let mut binary = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                return Err("usage: collection-resources [absolute-spindle-path] [--case short-record|full-width|sequence-width]".into());
            }
            "--case" => {
                i += 1;
                let value = args.get(i).ok_or("--case requires a workload name")?;
                if case.replace(value.as_str()).is_some() {
                    return Err("--case may be specified only once".into());
                }
            }
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            value => {
                if binary.replace(value).is_some() {
                    return Err("at most one absolute spindle path is accepted".into());
                }
            }
        }
        i += 1;
    }
    let selected: Vec<_> = match case {
        None => RESOURCE_WORKLOADS.to_vec(),
        Some(name) => RESOURCE_WORKLOADS
            .iter()
            .copied()
            .filter(|workload| workload.case == name)
            .collect(),
    };
    if selected.is_empty() {
        return Err("unknown workload; choose short-record, full-width, or sequence-width".into());
    }
    Ok((selected, binary.map(PathBuf::from)))
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("serve") {
        let cfg: spindle::Config =
            serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
        spindle::Daemon::open(cfg).unwrap().run().unwrap();
        return;
    }
    if args[1..].iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "usage: collection-resources [absolute-spindle-path] [--case short-record|full-width|sequence-width]"
        );
        return;
    }
    let (workloads, binary_arg) = parse_args(&args[1..]).unwrap_or_else(|error| panic!("{error}"));
    let binary = binary_arg.unwrap_or_else(|| std::env::current_exe().unwrap());
    assert!(binary.is_absolute() && binary.is_file());
    let mut reports = Vec::new();
    // Fixed coverage cases, not performance-selection holdouts.
    for workload in workloads {
        let slots = workload.slots;
        let width = workload.record_capacity_bytes;
        let producers = workload.producers;
        let case = workload.case;
        let root = xtask::evidence::non_tmp_tempdir().unwrap();
        let mut cfg = config(root.path());
        cfg.client_timeout_ms = 1000;
        let before = rss_bytes();
        let setup = Instant::now();
        let mut sdk = Config::local(cfg.socket.clone(), "resource-program".into()).unwrap();
        sdk.slots = slots;
        sdk.max_record = width;
        sdk.delivery_timeout = Duration::from_millis(100);
        let empty = fabric::observation::Observation {
            version: 2,
            registry: fabric::observation::REGISTRY.into(),
            identity: fabric::observation::Identity {
                producer: sdk.producer.clone(),
                generation: sdk.generation.clone(),
                sequence: slots as u64,
            },
            resource: sdk.resource.clone(),
            timestamp_ns: 1,
            data: Data::Log {
                level: Level::Info,
                message: String::new(),
            },
        };
        let overhead = serde_json::to_vec(&empty).unwrap().len();
        let payload_len = if case == "short-record" {
            width / 2
        } else {
            width - overhead
        };
        let client = Arc::new(Client::new(sdk).unwrap());
        let setup_ns = setup.elapsed().as_nanos();
        let barrier = Arc::new(Barrier::new(producers));
        let start = Instant::now();
        let workers: Vec<_> = (0..producers)
            .map(|_| {
                let c = client.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut timing = Vec::new();
                    let mut accepted = Vec::new();
                    let end = Instant::now() + Duration::from_secs(3);
                    let mut contended = 0;
                    loop {
                        assert!(timing.len() < 100_000, "admission attempt budget");
                        assert!(Instant::now() < end, "admission progress deadline");
                        // Caller allocation is outside the measured SDK call.
                        let data = Data::Log {
                            level: Level::Info,
                            message: "x".repeat(payload_len),
                        };
                        let t = Instant::now();
                        let result = c.try_record(data, 1);
                        timing.push((
                            t.elapsed().as_nanos(),
                            matches!(result, Admission::Accepted { .. }),
                        ));
                        match result {
                            Admission::Accepted { sequence } => accepted.push(sequence),
                            Admission::Contended => {
                                contended += 1;
                                std::thread::yield_now();
                            }
                            Admission::Full => break,
                            r => panic!("unexpected admission: {r:?}"),
                        }
                    }
                    (timing, accepted, contended)
                })
            })
            .collect();
        let mut timing = Vec::new();
        let mut accepted = Vec::new();
        let mut contended = 0;
        for w in workers {
            let (t, a, c) = w.join().unwrap();
            timing.extend(t);
            accepted.extend(a);
            contended += c;
        }
        accepted.sort();
        assert_eq!(accepted, (1..=slots as u64).collect::<Vec<_>>());
        let status = client.status();
        assert_eq!(status.charged_slots, slots);
        assert_eq!(status.confirmed, 0);
        assert_eq!(status.reserved_payload_bytes, slots * width);
        assert!(
            client
                .flush(Instant::now() + Duration::from_millis(30))
                .is_err()
        );
        assert_eq!(
            client.status().charged_slots,
            slots,
            "outage released capacity"
        );
        let outage_ns = start.elapsed().as_nanos();
        let rss = rss_bytes();
        let mut child = daemon(&binary, &cfg, root.path());
        let flush = Instant::now();
        let receipt = client
            .flush(Instant::now() + Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let flush_ns = flush.elapsed().as_nanos();
        // Incomplete maximum-sized frames must remain within configured connection buffers.
        use std::io::Write;
        let mut slow = Vec::new();
        for _ in 0..cfg.max_connections - 1 {
            let mut stream = std::os::unix::net::UnixStream::connect(&cfg.socket).unwrap();
            stream
                .write_all(&(fabric::observation::MAX_FRAME as u32).to_be_bytes())
                .unwrap();
            stream.write_all(b"{").unwrap();
            slow.push(stream);
        }
        let end = Instant::now() + Duration::from_millis(500);
        let collector = loop {
            let reply = spindle::transport::request(
                &cfg.socket,
                &spindle::Request::Status,
                Instant::now() + Duration::from_millis(200),
            )
            .unwrap();
            if let spindle::Reply::Status { status } = reply {
                assert!(status.active_connections <= cfg.max_connections);
                assert!(
                    status.queue_bytes
                        <= cfg.max_connections
                            * (fabric::observation::MAX_FRAME + spindle::MAX_REPLY + 4)
                );
                if status.active_connections >= slow.len()
                    && status.queue_bytes >= slow.len() * fabric::observation::MAX_FRAME
                {
                    break status;
                }
            }
            assert!(Instant::now() < end, "slow connection observation deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(collector.queue_bytes >= slow.len() * fabric::observation::MAX_FRAME);
        drop(slow);

        assert_eq!(receipt.identity.sequence, slots as u64);
        assert_eq!(client.status().charged_slots, 0);
        drop(client);
        let daemon_peak_rss = memory_peak::process_rss(child.0.id());
        assert!(matches!(
            spindle::transport::request(
                &cfg.socket,
                &spindle::Request::Stop,
                Instant::now() + Duration::from_secs(2)
            ),
            Ok(spindle::Reply::Stopped)
        ));
        // kill/wait also handles the short graceful-shutdown window before reopening.
        child.kill_wait();
        let store = Store::open(&cfg.storage, cfg.quota_bytes).unwrap();
        assert_eq!(store.count().unwrap(), slots as u64);
        let records = store.read(0, slots).unwrap();
        let mut payload_bytes = 0;
        for (index, (_, record)) in records.iter().enumerate() {
            assert_eq!(record.identity.sequence, index as u64 + 1);
            assert!(
                matches!(&record.data,Data::Log{message,..} if message==&"x".repeat(payload_len))
            );
            payload_bytes += serde_json::to_vec(record).unwrap().len();
        }
        if case == "full-width" {
            assert_eq!(payload_bytes, slots * width);
        }
        timing.sort();
        let admitted_timing: Vec<_> = timing
            .iter()
            .filter(|(_, accepted)| *accepted)
            .map(|(n, _)| *n)
            .collect();
        reports.push(serde_json::json!({"case":case,"slots":slots,"record_capacity":width,
            "producers":producers,"reserved_payload_bytes":slots*width,"retained_encoded_bytes":payload_bytes,
            "unused_payload_bytes":slots*width-payload_bytes,"rss_before":before,"rss_at_full":rss,
            "setup_ns":setup_ns,"outage_phase_ns":outage_ns,"recovery_flush_ns":flush_ns,
            "all_call_p50_ns":timing[(timing.len()*50).div_ceil(100)-1].0,
            "all_call_p99_ns":timing[(timing.len()*99).div_ceil(100)-1].0,
            "accepted_call_p50_ns":admitted_timing[(admitted_timing.len()*50).div_ceil(100)-1],
            "accepted_call_p99_ns":admitted_timing[(admitted_timing.len()*99).div_ceil(100)-1],
            "calls":timing.len(),"contended":contended,"collector_queue_bytes_with_slow_clients":collector.queue_bytes,
            "collector_rss_with_slow_clients":collector.process_resident_bytes,
            "daemon_lifetime_peak_rss_bytes":daemon_peak_rss,
            "storage_bytes":store.disk_bytes().unwrap()}));
    }
    let harness_peak_rss = memory_peak::process_rss(std::process::id());
    let cgroup_peak = memory_peak::visible_cgroup();
    println!(
        "{}",
        serde_json::json!({"passed":true,"workloads":reports,
        "harness_lifetime_peak_rss_bytes":harness_peak_rss,
        "visible_cgroup_lifetime_peak_bytes":cgroup_peak,
        "scope":"One run per configuration; SDK calls include input drop but exclude input construction; all admission outcomes included. RSS snapshots include harness and allocator, not a hard bound or isolated delta. Daemon peaks are per-child lifetime, sampled before stop. Harness peak is process-lifetime. Cgroup memory.peak is read from normalized self membership only after validating a cgroup2 mount rooted at / on /sys/fs/cgroup and confirming this PID in cgroup.procs before and after; it is cumulative for that cgroup and descendants since creation, not per workload or reset. Proc RSS accounting can be approximate. Warm cache uncontrolled; no latency optimality claim."})
    );
}

#[cfg(test)]
mod selector_tests {
    use super::*;

    #[test]
    fn defaults_to_all_existing_workloads() {
        let (selected, binary) = parse_args(&[]).unwrap();
        assert_eq!(selected, RESOURCE_WORKLOADS.to_vec());
        assert_eq!(binary, None);
    }

    #[test]
    fn selects_exactly_one_workload_and_accepts_binary_path() {
        let args = vec!["--case".into(), "full-width".into(), "/opt/spindle".into()];
        let (selected, binary) = parse_args(&args).unwrap();
        assert_eq!(selected, vec![RESOURCE_WORKLOADS[1]]);
        assert_eq!(binary, Some(PathBuf::from("/opt/spindle")));
    }

    #[test]
    fn rejects_unknown_or_repeated_selection() {
        assert!(parse_args(&["--case".into(), "other".into()]).is_err());
        assert!(
            parse_args(&[
                "--case".into(),
                "full-width".into(),
                "--case".into(),
                "short-record".into()
            ])
            .is_err()
        );
    }
}

mod memory_peak {
    use serde::Serialize;
    use std::{
        fs::File,
        io::{self, Read},
        path::{Path, PathBuf},
    };

    const STATUS_MAX: u64 = 64 * 1024;
    const MEMBERSHIP_MAX: u64 = 8192;
    const PEAK_MAX: u64 = 64;
    const MOUNTINFO_MAX: u64 = 128 * 1024;
    const MOUNTINFO_MAX_LINES: usize = 2048;
    const CGROUP_PROCS_MAX: u64 = 1024 * 1024;
    const CGROUP_PROCS_MAX_ENTRIES: usize = 65_536;
    const CGROUP_COMPONENT_MAX: usize = 255;

    #[derive(Debug, PartialEq, Eq, Serialize)]
    #[serde(tag = "state", content = "bytes", rename_all = "snake_case")]
    pub(super) enum Reading {
        Available(u64),
        Missing,
        Invalid,
        Unavailable,
    }

    fn read_bounded(path: &Path, limit: u64) -> Result<String, Reading> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                Reading::Missing
            } else {
                Reading::Unavailable
            }
        })?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Reading::Unavailable)?;
        if bytes.len() as u64 > limit {
            return Err(Reading::Invalid);
        }
        String::from_utf8(bytes).map_err(|_| Reading::Invalid)
    }

    fn rss_from_status(status: &str) -> Reading {
        let mut found = None;
        for line in status.lines() {
            if let Some(value) = line.strip_prefix("VmHWM:") {
                if found.is_some() {
                    return Reading::Invalid;
                }
                let mut fields = value.split_whitespace();
                let value = fields
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .and_then(|v| v.checked_mul(1024));
                if fields.next() != Some("kB") || fields.next().is_some() || value.is_none() {
                    return Reading::Invalid;
                }
                found = value;
            }
        }
        found.map_or(Reading::Missing, Reading::Available)
    }

    fn cgroup_path(membership: &str) -> Result<String, Reading> {
        if !membership.is_empty() && !membership.ends_with('\n') {
            return Err(Reading::Invalid);
        }
        let mut found = None;
        for line in membership.lines() {
            if let Some(value) = line.strip_prefix("0::")
                && found.replace(value).is_some()
            {
                return Err(Reading::Invalid);
            }
        }
        let value = found.ok_or(Reading::Missing)?;
        if value.is_empty()
            || value.len() > MEMBERSHIP_MAX as usize
            || !value.starts_with('/')
            || value.contains('\0')
            || value.chars().any(char::is_control)
        {
            return Err(Reading::Invalid);
        }
        if value == "/" {
            return Ok(value.to_owned());
        }
        let tail = value.strip_prefix('/').ok_or(Reading::Invalid)?;
        if tail.is_empty() || tail.ends_with('/') {
            return Err(Reading::Invalid);
        }
        for component in tail.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.len() > CGROUP_COMPONENT_MAX
            {
                return Err(Reading::Invalid);
            }
        }
        Ok(value.to_owned())
    }

    fn peak_value(text: &str) -> Reading {
        let value = text.trim();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Reading::Invalid;
        }
        value.parse().map_or(Reading::Invalid, Reading::Available)
    }

    pub(super) fn process_rss(pid: u32) -> Reading {
        let path = PathBuf::from(format!("/proc/{pid}/status"));
        match read_bounded(&path, STATUS_MAX) {
            Ok(s) => rss_from_status(&s),
            Err(e) => e,
        }
    }

    pub(super) fn visible_cgroup() -> Reading {
        visible_cgroup_from(
            Path::new("/proc"),
            Path::new("/sys/fs/cgroup"),
            std::process::id(),
        )
    }

    fn visible_cgroup_from(proc_root: &Path, cgroup_root: &Path, pid: u32) -> Reading {
        if validate_cgroup2_mount(proc_root).is_err() {
            return Reading::Unavailable;
        }
        let membership = match read_process_cgroup_path(proc_root) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let cgroup_dir = match resolve_cgroup_dir(cgroup_root, &membership) {
            Ok(path) => path,
            Err(error) => return error,
        };
        if verify_process_membership(&cgroup_dir, pid).is_err() {
            return Reading::Unavailable;
        }

        let peak = match read_cgroup_bounded(&cgroup_dir.join("memory.peak"), PEAK_MAX) {
            Ok(value) => peak_value(&value),
            Err(error) => error,
        };
        let membership_after = match read_process_cgroup_path(proc_root) {
            Ok(path) => path,
            Err(_) => return Reading::Unavailable,
        };
        if membership_after != membership
            || verify_process_membership(&cgroup_dir, pid).is_err()
            || validate_cgroup2_mount(proc_root).is_err()
        {
            return Reading::Unavailable;
        }
        peak
    }

    fn read_cgroup_bounded(path: &Path, limit: u64) -> Result<String, Reading> {
        read_bounded(path, limit).map_err(|error| match error {
            Reading::Invalid => Reading::Unavailable,
            other => other,
        })
    }

    fn read_process_cgroup_path(proc_root: &Path) -> Result<String, Reading> {
        let membership = read_cgroup_bounded(&proc_root.join("self/cgroup"), MEMBERSHIP_MAX)?;
        cgroup_path(&membership)
    }

    fn validate_cgroup2_mount(proc_root: &Path) -> Result<(), Reading> {
        let mountinfo = read_cgroup_bounded(&proc_root.join("self/mountinfo"), MOUNTINFO_MAX)?;
        if cgroup2_mount_is_compatible(&mountinfo) {
            Ok(())
        } else {
            Err(Reading::Unavailable)
        }
    }

    fn cgroup2_mount_is_compatible(mountinfo: &str) -> bool {
        if !mountinfo.is_empty() && !mountinfo.ends_with('\n') {
            return false;
        }
        let mut cgroup2_mounts = Vec::new();
        let mut expected_mounts = Vec::new();
        for (index, line) in mountinfo.lines().enumerate() {
            if index + 1 > MOUNTINFO_MAX_LINES {
                return false;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            let separators: Vec<_> = fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| (*field == "-").then_some(index))
                .collect();
            if separators.len() != 1
                || separators[0] < 6
                || fields.len() < separators[0] + 4
                || fields.len() < 5
            {
                return false;
            }
            let mount = (fields[3], fields[4], fields[separators[0] + 1]);
            if mount.1 == "/sys/fs/cgroup" {
                expected_mounts.push(mount);
            }
            if mount.2 == "cgroup2" {
                cgroup2_mounts.push(mount);
            }
        }
        if cgroup2_mounts.len() != 1 || expected_mounts.len() != 1 {
            return false;
        }
        let (root, mountpoint, filesystem) = cgroup2_mounts[0];
        root == "/"
            && mountpoint == "/sys/fs/cgroup"
            && filesystem == "cgroup2"
            && expected_mounts[0] == cgroup2_mounts[0]
    }

    fn resolve_cgroup_dir(root: &Path, membership: &str) -> Result<PathBuf, Reading> {
        let root_metadata = std::fs::symlink_metadata(root).map_err(|_| Reading::Unavailable)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(Reading::Unavailable);
        }
        let mut result = root.to_path_buf();
        if membership != "/" {
            let relative = membership.strip_prefix('/').ok_or(Reading::Invalid)?;
            for component in relative.split('/') {
                if component.is_empty() || component == "." || component == ".." {
                    return Err(Reading::Invalid);
                }
                result.push(component);
                let metadata =
                    std::fs::symlink_metadata(&result).map_err(|_| Reading::Unavailable)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(Reading::Unavailable);
                }
            }
        }
        Ok(result)
    }

    fn verify_process_membership(cgroup_dir: &Path, pid: u32) -> Result<(), Reading> {
        let cgroup_type = read_cgroup_bounded(&cgroup_dir.join("cgroup.type"), 64)?;
        if cgroup_type != "domain\n" {
            return Err(Reading::Unavailable);
        }
        let processes = read_cgroup_bounded(&cgroup_dir.join("cgroup.procs"), CGROUP_PROCS_MAX)?;
        if !processes.is_empty() && !processes.ends_with('\n') {
            return Err(Reading::Invalid);
        }
        let mut found = false;
        for (index, line) in processes.lines().enumerate() {
            let line_number = index + 1;
            if line_number > CGROUP_PROCS_MAX_ENTRIES {
                return Err(Reading::Unavailable);
            }
            let process_id = line
                .parse::<u32>()
                .ok()
                .filter(|process_id| *process_id != 0)
                .ok_or(Reading::Invalid)?;
            found |= process_id == pid;
        }
        if found {
            Ok(())
        } else {
            Err(Reading::Unavailable)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn status_requires_single_valid_checked_kib() {
            assert_eq!(
                rss_from_status("Name: x\nVmHWM: 12 kB\n"),
                Reading::Available(12288)
            );
            assert_eq!(rss_from_status("Name: x\n"), Reading::Missing);
            for s in [
                "VmHWM: 1 MB",
                "VmHWM: nope kB",
                "VmHWM: 18014398509481984 kB",
                "VmHWM: 1 kB\nVmHWM: 2 kB",
                "VmHWM: 1 kB extra",
            ] {
                assert_eq!(rss_from_status(s), Reading::Invalid);
            }
        }
        #[test]
        fn membership_rejects_escape_root_and_duplicate() {
            assert_eq!(
                cgroup_path("0::/user.slice/run.scope\n"),
                Ok("/user.slice/run.scope".to_owned())
            );
            assert_eq!(cgroup_path("2:cpu:/x\n"), Err(Reading::Missing));
            assert_eq!(cgroup_path("0::/\n"), Ok("/".to_owned()));
            for s in ["0::/../x", "0::/x/./y", "0::/x//y", "0::x", "0::/x\n0::/y"] {
                assert_eq!(cgroup_path(s), Err(Reading::Invalid));
            }
        }
        #[test]
        fn peak_requires_one_unsigned_bounded_value() {
            assert_eq!(peak_value("4096\n"), Reading::Available(4096));
            for s in ["", "max", "1 2", "-1", "18446744073709551616"] {
                assert_eq!(peak_value(s), Reading::Invalid);
            }
        }

        #[test]
        fn visible_cgroup_peak_requires_root_mount_and_matching_pid() {
            let root = xtask::evidence::non_tmp_tempdir().unwrap();
            let proc_root = root.path().join("proc");
            let cgroup_root = root.path().join("sys/fs/cgroup");
            let proc_self = proc_root.join("self");
            let cgroup_dir = cgroup_root.join("resource-test");
            std::fs::create_dir_all(&proc_self).unwrap();
            std::fs::create_dir_all(&cgroup_dir).unwrap();
            std::fs::write(proc_self.join("cgroup"), "0::/resource-test\n").unwrap();
            std::fs::write(
                proc_self.join("mountinfo"),
                "35 25 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n",
            )
            .unwrap();
            std::fs::write(cgroup_dir.join("cgroup.type"), "domain\n").unwrap();
            std::fs::write(
                cgroup_dir.join("cgroup.procs"),
                format!("{}\n", std::process::id()),
            )
            .unwrap();
            std::fs::write(cgroup_dir.join("memory.peak"), "4096\n").unwrap();

            assert_eq!(
                visible_cgroup_from(&proc_root, &cgroup_root, std::process::id()),
                Reading::Available(4096)
            );
            std::fs::write(
                proc_self.join("mountinfo"),
                "35 25 0:29 /host-slice /sys/fs/cgroup rw - cgroup2 cgroup rw\n",
            )
            .unwrap();
            assert_eq!(
                visible_cgroup_from(&proc_root, &cgroup_root, std::process::id()),
                Reading::Unavailable
            );

            std::fs::write(
                proc_self.join("mountinfo"),
                "35 25 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n",
            )
            .unwrap();
            std::fs::write(cgroup_dir.join("cgroup.procs"), "4294967295\n").unwrap();
            assert_eq!(
                visible_cgroup_from(&proc_root, &cgroup_root, std::process::id()),
                Reading::Unavailable
            );
        }
    }
}
