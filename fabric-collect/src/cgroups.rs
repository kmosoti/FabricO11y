//! Cgroup v2 walker (`app.slice`, `agent.slice`, `session.slice`, `fabrico11y.slice`, `system.slice`)
//! and `/proc/<pid>` attribution for RSS, swap, open FDs, unlinked FDs, and touched paths.

use fabric_core::{AgentSessionRecord, CgroupSample, ProcessSample, SliceSummary};
use rustix::process::getuid;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::self_guard::page_size_bytes;

const TARGET_SLICES: &[&str] = &[
    "app.slice",
    "agent.slice",
    "session.slice",
    "fabrico11y.slice",
];

const MAX_PIDS_PER_COLLECT: usize = 320;
const MAX_FDS_SCANNED_PER_PID: usize = 96;

/// Combined result from walking cgroups v2 and attributing processes & agent sessions.
#[derive(Debug, Clone, Default)]
pub struct CgroupWalkResult {
    pub slices: Vec<SliceSummary>,
    pub cgroups: Vec<CgroupSample>,
    pub processes: Vec<ProcessSample>,
    pub agents: Vec<AgentSessionRecord>,
}

/// Walks user and system cgroup v2 hierarchies with bounded depth and PID count.
#[must_use]
pub fn walk_cgroups_and_processes() -> CgroupWalkResult {
    let uid = getuid().as_raw();
    let user_service_root = PathBuf::from(format!(
        "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service"
    ));
    let system_slice_root = PathBuf::from("/sys/fs/cgroup/system.slice");

    let mut slices = Vec::new();
    let mut cgroups = Vec::new();
    let mut pid_origins: BTreeMap<u32, (String, Option<String>, String)> = BTreeMap::new();

    for &slice_name in TARGET_SLICES {
        let slice_dir = user_service_root.join(slice_name);
        if slice_dir.is_dir() {
            slices.push(read_slice_summary(slice_name, &slice_dir));
            collect_slice_children(slice_name, &slice_dir, &mut cgroups, &mut pid_origins);
        }
    }

    if system_slice_root.is_dir() {
        slices.push(read_slice_summary("system.slice", &system_slice_root));
        collect_slice_children(
            "system.slice",
            &system_slice_root,
            &mut cgroups,
            &mut pid_origins,
        );
    }

    // Read current process attributes for attributed PIDs (prioritizing agent.slice / fabrico11y.slice / app.slice).
    let mut processes = Vec::new();
    let mut ordered_pids: Vec<(u8, u32, String, Option<String>, String)> = pid_origins
        .into_iter()
        .map(|(pid, (slice, unit, cg_path))| {
            let prio = match slice.as_str() {
                "agent.slice" => 0,
                "fabrico11y.slice" => 1,
                "app.slice" => 2,
                "session.slice" => 3,
                _ => 4,
            };
            (prio, pid, slice, unit, cg_path)
        })
        .collect();
    ordered_pids.sort_by_key(|x| (x.0, x.1));

    for (_, pid, slice, unit, cg_path) in ordered_pids.into_iter().take(MAX_PIDS_PER_COLLECT) {
        if let Some(sample) = read_process_sample(pid, slice, unit, cg_path) {
            processes.push(sample);
        }
    }

    let agents = aggregate_agent_sessions(&processes);

    CgroupWalkResult {
        slices,
        cgroups,
        processes,
        agents,
    }
}

fn read_slice_summary(slice_name: &str, dir: &Path) -> SliceSummary {
    let mem_current = read_u64(dir.join("memory.current")).unwrap_or(0);
    let mem_high = read_limit(dir.join("memory.high"));
    let mem_max = read_limit(dir.join("memory.max"));
    let mem_peak = read_u64(dir.join("memory.peak"));
    let cpu_usage_usec = read_cpu_usage_usec(dir.join("cpu.stat"));
    let (io_read_bytes, io_write_bytes) = read_io_stat_bytes(dir.join("io.stat"));
    let pids_current = read_u64(dir.join("pids.current")).unwrap_or(0);
    let (memory_high_events, oom_events) = read_memory_events(dir.join("memory.events"));

    SliceSummary {
        slice_name: slice_name.to_owned(),
        cgroup_path: dir.to_string_lossy().into_owned(),
        memory_current_bytes: mem_current,
        memory_high_bytes: mem_high,
        memory_max_bytes: mem_max,
        memory_peak_bytes: mem_peak,
        cpu_usage_usec,
        io_read_bytes,
        io_write_bytes,
        pids_current,
        memory_high_events,
        oom_events,
    }
}

fn collect_slice_children(
    slice_name: &str,
    slice_dir: &Path,
    cgroups: &mut Vec<CgroupSample>,
    pid_origins: &mut BTreeMap<u32, (String, Option<String>, String)>,
) {
    // Also check direct slice PIDs.
    record_cgroup_pids(
        slice_dir,
        slice_name,
        None,
        &slice_dir.to_string_lossy(),
        pid_origins,
    );

    let mut stack = vec![slice_dir.to_path_buf()];
    let mut visited_dirs = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if visited_dirs >= 512 {
                return;
            }
            let path = entry.path();
            let Ok(meta) = path.symlink_metadata() else {
                continue;
            };
            if meta.is_symlink() || !meta.is_dir() {
                continue;
            }
            visited_dirs += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            let unit = if name.ends_with(".service") || name.ends_with(".scope") {
                Some(name.clone())
            } else {
                extract_unit_from_path(&path)
            };

            let cg_path_str = path.to_string_lossy().into_owned();
            let mem_current = read_u64(path.join("memory.current")).unwrap_or(0);
            let mem_peak = read_u64(path.join("memory.peak"));
            let mem_swap = read_u64(path.join("memory.swap.current")).unwrap_or(0);
            let cpu_usage_usec = read_cpu_usage_usec(path.join("cpu.stat"));
            let (io_read_bytes, io_write_bytes) = read_io_stat_bytes(path.join("io.stat"));
            let pids_current = read_u64(path.join("pids.current")).unwrap_or(0);
            let populated = read_cgroup_populated(path.join("cgroup.events"));

            cgroups.push(CgroupSample {
                path: cg_path_str.clone(),
                slice: slice_name.to_owned(),
                unit: unit.clone(),
                memory_current_bytes: mem_current,
                memory_peak_bytes: mem_peak,
                memory_swap_current_bytes: mem_swap,
                cpu_usage_usec,
                io_read_bytes,
                io_write_bytes,
                pids_current,
                populated,
            });

            record_cgroup_pids(&path, slice_name, unit.as_ref(), &cg_path_str, pid_origins);
            stack.push(path);
        }
    }
}

fn record_cgroup_pids(
    dir: &Path,
    slice_name: &str,
    unit: Option<&String>,
    cg_path: &str,
    pid_origins: &mut BTreeMap<u32, (String, Option<String>, String)>,
) {
    let Ok(raw) = fs::read_to_string(dir.join("cgroup.procs")) else {
        return;
    };
    for line in raw.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            pid_origins.insert(
                pid,
                (slice_name.to_owned(), unit.cloned(), cg_path.to_owned()),
            );
        }
    }
}

fn extract_unit_from_path(path: &Path) -> Option<String> {
    for comp in path.components().rev() {
        let s = comp.as_os_str().to_string_lossy();
        if s.ends_with(".service") || s.ends_with(".scope") {
            return Some(s.into_owned());
        }
    }
    None
}

fn read_process_sample(
    pid: u32,
    slice: String,
    unit: Option<String>,
    cgroup_path: String,
) -> Option<ProcessSample> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    if !proc_dir.is_dir() {
        return None;
    }

    let comm = fs::read_to_string(proc_dir.join("comm"))
        .map(|s| s.trim().to_owned())
        .ok()?;
    let cmdline_bytes = fs::read(proc_dir.join("cmdline")).unwrap_or_default();
    let ((argv0, argv1), cmdline) = parse_process_cmdline(&cmdline_bytes);

    let (ppid, cpu_time_ticks) = read_proc_stat(&proc_dir.join("stat"));
    let rss_bytes = read_proc_rss(&proc_dir.join("statm"));
    let swap_bytes = read_proc_swap(&proc_dir.join("status"));
    let cwd = fs::read_link(proc_dir.join("cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    let (open_fds, unlinked_fds, open_paths) = scan_proc_fds(&proc_dir.join("fd"));
    let agent_hint = detect_agent_hint(&slice, unit.as_deref(), &comm, &argv0, &argv1);

    Some(ProcessSample {
        pid,
        ppid,
        comm,
        cmdline,
        unit,
        slice,
        cgroup_path,
        rss_bytes,
        swap_bytes,
        cpu_time_ticks,
        open_fds,
        unlinked_fds,
        cwd,
        open_paths,
        agent_hint,
    })
}

fn parse_process_cmdline(
    bytes: &[u8],
) -> (
    (std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>),
    String,
) {
    let mut positions = bytes.split(|&byte| byte == 0);
    let argv0 = String::from_utf8_lossy(positions.next().unwrap_or_default());
    let argv1 = String::from_utf8_lossy(positions.next().unwrap_or_default());
    let mut display = String::new();
    let mut remaining = 240;
    for arg in bytes.split(|&byte| byte == 0).filter(|arg| !arg.is_empty()) {
        if remaining == 0 {
            break;
        }
        if !display.is_empty() {
            display.push(' ');
            remaining -= 1;
        }
        // At most four bytes can contribute each displayed Unicode character.
        // Decoding only this prefix keeps even invalid, very long arguments bounded.
        let prefix = &arg[..arg.len().min(remaining * 4)];
        for character in String::from_utf8_lossy(prefix).chars().take(remaining) {
            display.push(character);
            remaining -= 1;
        }
    }
    ((argv0, argv1), display)
}

const AGENTS: &[&str] = &[
    "synthetic_agent",
    "antigravity",
    "agy",
    "codex",
    "metron",
    "dsh",
    "fabric",
];
const BROWSER_UNITS: &[&str] = &[
    "com.google.chrome",
    "org.chromium.chromium",
    "gnome-chatgpt",
    "google-chrome",
    "chrome",
    "chromium",
    "org.mozilla.firefox",
    "firefox",
    "com.brave.browser",
    "brave",
    "vivaldi",
    "microsoft-edge",
    "org.gnome.epiphany",
];
const BROWSER_PROCESSES: &[&str] = &[
    "chrome",
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "google-chrome-unstable",
    "chromium",
    "chromium-browser",
    "chrome_crashpad",
    "crashpad_handler",
    "firefox",
    "firefox-bin",
    "brave",
    "brave-browser",
    "vivaldi-bin",
    "msedge",
    "epiphany",
    "webkitwebprocess",
    "webkitnetworkprocess",
    "web content",
    "isolated web co",
];

fn unit_has_name(unit: &str, name: &str) -> bool {
    unit.strip_prefix(name)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(['-', '.', '@']))
}

fn executable_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

fn known_agent(executable: &str) -> Option<&'static str> {
    let name = match executable {
        "synthetic_agent_load" => "synthetic_agent",
        _ => executable,
    };
    AGENTS.iter().copied().find(|&agent| agent == name)
}

fn detect_agent_hint(
    slice: &str,
    unit: Option<&str>,
    comm: &str,
    argv0: &str,
    argv1: &str,
) -> Option<String> {
    let unit = unit.unwrap_or_default().to_ascii_lowercase();
    let desktop_unit = unit
        .strip_prefix("app-flatpak-")
        .or_else(|| unit.strip_prefix("app-"));
    let command = comm.to_ascii_lowercase();
    let executable = executable_name(argv0).to_ascii_lowercase();
    if BROWSER_PROCESSES.contains(&executable.as_str())
        || BROWSER_PROCESSES.contains(&command.as_str())
        || desktop_unit
            .is_some_and(|unit| BROWSER_UNITS.iter().any(|name| unit_has_name(unit, name)))
    {
        return None;
    }

    if let Some(agent) = known_agent(&executable).or_else(|| known_agent(&command)) {
        return Some(agent.to_owned());
    }
    // Only a direct script invocation is evidence. Flags, their values, URLs, and later
    // arguments can describe arbitrary work unrelated to the process executable.
    if matches!(
        executable.as_str(),
        "node" | "nodejs" | "python" | "python3" | "bun" | "deno"
    ) && !argv1.starts_with('-')
        && !argv1.contains("://")
    {
        let script = executable_name(argv1).to_ascii_lowercase();
        let script = [".js", ".mjs", ".cjs", ".py"]
            .into_iter()
            .find_map(|extension| script.strip_suffix(extension))
            .unwrap_or(&script);
        if let Some(agent) = known_agent(script) {
            return Some(agent.to_owned());
        }
    }
    let unit = desktop_unit.unwrap_or(&unit);
    AGENTS
        .iter()
        .copied()
        .find(|name| unit_has_name(unit, name))
        .map(str::to_owned)
        .or_else(|| (slice == "agent.slice" && !comm.is_empty()).then(|| comm.to_owned()))
}

fn aggregate_agent_sessions(processes: &[ProcessSample]) -> Vec<AgentSessionRecord> {
    let mut grouped: BTreeMap<(String, String, Option<String>), AgentSessionRecord> =
        BTreeMap::new();
    for p in processes {
        let Some(agent_name) = &p.agent_hint else {
            continue;
        };
        let key = (agent_name.clone(), p.slice.clone(), p.unit.clone());
        let entry = grouped.entry(key).or_insert_with(|| AgentSessionRecord {
            session_id: p
                .unit
                .clone()
                .unwrap_or_else(|| format!("{agent_name}-{}", p.pid)),
            agent_name: agent_name.clone(),
            slice: p.slice.clone(),
            unit: p.unit.clone(),
            pids: Vec::new(),
            rss_bytes: 0,
            write_bytes: 0,
            touched_paths: Vec::new(),
        });
        entry.pids.push(p.pid);
        entry.rss_bytes = entry.rss_bytes.saturating_add(p.rss_bytes);
        for path in &p.open_paths {
            if !entry.touched_paths.contains(path) && entry.touched_paths.len() < 16 {
                entry.touched_paths.push(path.clone());
            }
        }
    }
    grouped.into_values().collect()
}

fn scan_proc_fds(fd_dir: &Path) -> (u32, u32, Vec<String>) {
    let Ok(entries) = fs::read_dir(fd_dir) else {
        return (0, 0, Vec::new());
    };
    let mut open_fds = 0u32;
    let mut unlinked_fds = 0u32;
    let mut paths = BTreeSet::new();
    for entry in entries.flatten() {
        open_fds = open_fds.saturating_add(1);
        if (open_fds as usize) > MAX_FDS_SCANNED_PER_PID {
            continue;
        }
        if let Ok(target) = fs::read_link(entry.path()) {
            let s = target.to_string_lossy();
            if s.ends_with(" (deleted)") {
                unlinked_fds = unlinked_fds.saturating_add(1);
            }
            if s.starts_with("/tmp/")
                || s.starts_with("/data/tmp/")
                || s.contains("/.cache/")
                || s.contains("/target/")
            {
                let clean = s.strip_suffix(" (deleted)").unwrap_or(&s).to_owned();
                paths.insert(clean);
            }
        }
    }
    (open_fds, unlinked_fds, paths.into_iter().take(16).collect())
}

fn read_proc_stat(path: &Path) -> (u32, u64) {
    let Ok(raw) = fs::read_to_string(path) else {
        return (0, 0);
    };
    let Some(rparen) = raw.rfind(')') else {
        return (0, 0);
    };
    let after = raw.get(rparen + 2..).unwrap_or("");
    let cols: Vec<&str> = after.split_whitespace().collect();
    // fields after comm start at state (index 0), ppid is index 1, utime is index 11, stime is index 12.
    let ppid = cols.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let utime = cols
        .get(11)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let stime = cols
        .get(12)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    (ppid, utime.saturating_add(stime))
}

fn read_proc_rss(path: &Path) -> u64 {
    let Ok(raw) = fs::read_to_string(path) else {
        return 0;
    };
    let pages = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    pages.saturating_mul(page_size_bytes())
}

fn read_proc_swap(path: &Path) -> u64 {
    let Ok(raw) = fs::read_to_string(path) else {
        return 0;
    };
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VmSwap:") {
            let kb = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            return kb.saturating_mul(1024);
        }
    }
    0
}

fn read_u64(path: PathBuf) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}

fn read_limit(path: PathBuf) -> Option<u64> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed == "max" {
        None
    } else {
        trimmed.parse::<u64>().ok()
    }
}

fn read_cpu_usage_usec(path: PathBuf) -> u64 {
    let Ok(raw) = fs::read_to_string(path) else {
        return 0;
    };
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("usage_usec ") {
            return rest.trim().parse::<u64>().unwrap_or(0);
        }
    }
    0
}

fn read_io_stat_bytes(path: PathBuf) -> (u64, u64) {
    let Ok(raw) = fs::read_to_string(path) else {
        return (0, 0);
    };
    let mut rbytes = 0u64;
    let mut wbytes = 0u64;
    for line in raw.lines() {
        for field in line.split_whitespace().skip(1) {
            if let Some(v) = field.strip_prefix("rbytes=") {
                rbytes = rbytes.saturating_add(v.parse::<u64>().unwrap_or(0));
            } else if let Some(v) = field.strip_prefix("wbytes=") {
                wbytes = wbytes.saturating_add(v.parse::<u64>().unwrap_or(0));
            }
        }
    }
    (rbytes, wbytes)
}

fn read_memory_events(path: PathBuf) -> (u64, u64) {
    let Ok(raw) = fs::read_to_string(path) else {
        return (0, 0);
    };
    let mut high = 0u64;
    let mut oom = 0u64;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("high ") {
            high = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("oom_kill ") {
            oom = oom.saturating_add(v.trim().parse::<u64>().unwrap_or(0));
        }
    }
    (high, oom)
}

fn read_cgroup_populated(path: PathBuf) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return true;
    };
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("populated ") {
            return v.trim() == "1";
        }
    }
    true
}

#[cfg(test)]
mod attribution_tests {
    use super::detect_agent_hint;

    #[test]
    fn argv_preserves_display_and_limits_attribution_to_identity_positions() {
        let bytes = b"/bin/node\0/work/main.js\0codex\0\0";
        let ((argv0, argv1), display) = super::parse_process_cmdline(bytes);
        assert_eq!(display, "/bin/node /work/main.js codex");
        assert_eq!(
            super::detect_agent_hint("app.slice", None, "node", &argv0, &argv1),
            None
        );
        let unicode = format!("/bin/worker\0{}\0", "é".repeat(130));
        let (_, display) = super::parse_process_cmdline(unicode.as_bytes());
        assert_eq!(display, format!("/bin/worker {}", "é".repeat(130)));
        let long = format!("/bin/worker\0{}\0", "x".repeat(300));
        let (_, display) = super::parse_process_cmdline(long.as_bytes());
        assert_eq!(display.chars().count(), 240);
    }

    #[test]
    fn argv_keeps_empty_positions_and_lossy_display_boundaries() {
        let mut bytes = vec![0; 100_000];
        bytes.extend_from_slice(b"codex\0");
        let ((argv0, argv1), display) = super::parse_process_cmdline(&bytes);
        assert_eq!((&*argv0, &*argv1, display.as_str()), ("", "", "codex"));
        assert_eq!(
            super::detect_agent_hint("app.slice", None, "worker", &argv0, &argv1),
            None
        );

        let mut bytes = vec![b'x'; 238];
        bytes.extend_from_slice(b"\0\xf0\x9f\x92\xa9\xfftail\0");
        let (_, display) = super::parse_process_cmdline(&bytes);
        assert_eq!(display, format!("{} 💩", "x".repeat(238)));
        let (_, display) = super::parse_process_cmdline(b"a\0\xf0\x9f\xffb\0");
        assert_eq!(display, "a ��b");
    }

    #[test]
    fn attribution_uses_executable_script_or_scope_identity() {
        for (slice, unit, comm, program, argument, expected) in [
            (
                "app.slice",
                Some("app-com.google.Chrome-codex.scope"),
                "ThreadPoolForeg",
                "/bin/chrome",
                "--app=https://example.com/codex",
                None,
            ),
            (
                "agent.slice",
                None,
                "chrome",
                "/bin/chrome",
                "https://example.com/codex",
                None,
            ),
            (
                "app.slice",
                Some("app-gnome-chatgpt-42.scope"),
                "helper",
                "/bin/helper",
                "codex",
                None,
            ),
            ("app.slice", None, "mycodex", "/bin/mycodex", "", None),
            (
                "app.slice",
                Some("app-chrome-42.scope"),
                "codex",
                "/bin/codex",
                "",
                None,
            ),
            (
                "app.slice",
                Some("mycodex.service"),
                "worker",
                "/bin/worker",
                "",
                None,
            ),
            ("app.slice", None, "worker", "/bin/worker", "", None),
            (
                "app.slice",
                None,
                "node",
                "/bin/node",
                "/work/mycodex.js",
                None,
            ),
            (
                "app.slice",
                None,
                "editor",
                "/bin/editor",
                "/var/work/codex",
                None,
            ),
            (
                "app.slice",
                None,
                "node",
                "/bin/node",
                "https://example.com/codex",
                None,
            ),
            (
                "app.slice",
                None,
                "node",
                "/bin/node",
                "--require=/var/work/codex",
                None,
            ),
            (
                "app.slice",
                None,
                "node",
                "/bin/node",
                "/var/work area/codex.mjs",
                Some("codex"),
            ),
            (
                "agent.slice",
                Some("codex-remote.service"),
                "worker",
                "/bin/worker",
                "",
                Some("codex"),
            ),
            (
                "agent.slice",
                Some("codex-remote.service"),
                "metron",
                "/bin/metron",
                "",
                Some("metron"),
            ),
            (
                "agent.slice",
                None,
                "custom-worker",
                "/bin/custom-worker",
                "",
                Some("custom-worker"),
            ),
            ("agent.slice", None, "", "", "", None),
        ] {
            assert_eq!(
                detect_agent_hint(slice, unit, comm, program, argument).as_deref(),
                expected,
                "{slice} {unit:?} {comm} {program} {argument}",
            );
        }
    }
}
