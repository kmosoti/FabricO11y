//! High-churn filesystem watcher (`rustix::fs::inotify` + bounded `O_NOFOLLOW` scanner)
//! across `/tmp`, `/data/tmp`, `~/.cache`, and active workspace `target/` paths.

use chrono::Utc;
use fabric_core::{FsChurnEntry, FsEventRecord, MountUsage, ProcessSample};
use rustix::fs::inotify;
use rustix::io::Errno;
use std::collections::BTreeMap;
use std::fs;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::host::resolve_mount_point;

const MAX_SCAN_ENTRIES_PER_ROOT: usize = 256;
const MAX_SCAN_DEPTH: usize = 2;

/// Non-blocking `inotify` watcher + previous-size tracker for churn delta calculation.
#[derive(Debug)]
pub struct FsChurnTracker {
    inotify_fd: Option<OwnedFd>,
    wd_to_root: BTreeMap<i32, PathBuf>,
    prev_sizes: BTreeMap<String, u64>,
    watch_roots: Vec<PathBuf>,
}

impl Default for FsChurnTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FsChurnTracker {
    #[must_use]
    pub fn new() -> Self {
        let mut roots = Vec::new();
        for fixed in ["/tmp", "/data/tmp"] {
            let p = PathBuf::from(fixed);
            if p.is_dir() {
                roots.push(p);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home_dir = PathBuf::from(home);
            let cache_dir = home_dir.join(".cache");
            if cache_dir.is_dir() {
                roots.push(cache_dir);
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let t = cwd.join("target");
            if t.is_dir() {
                roots.push(t);
            }
        }
        let target_env = std::env::var_os("CARGO_BUILD_TARGET_DIR").map(PathBuf::from);
        if let Some(t) = target_env
            && t.is_dir()
            && !roots.contains(&t)
        {
            roots.push(t);
        }

        let mut inotify_fd = None;
        let mut wd_to_root = BTreeMap::new();
        if let Ok(fd) =
            inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)
        {
            let flags = inotify::WatchFlags::CREATE
                | inotify::WatchFlags::CLOSE_WRITE
                | inotify::WatchFlags::MOVED_TO
                | inotify::WatchFlags::DELETE
                | inotify::WatchFlags::MODIFY;
            for root in &roots {
                if let Ok(wd) = inotify::add_watch(&fd, root, flags) {
                    wd_to_root.insert(wd, root.clone());
                }
            }
            inotify_fd = Some(fd);
        }

        Self {
            inotify_fd,
            wd_to_root,
            prev_sizes: BTreeMap::new(),
            watch_roots: roots,
        }
    }

    /// Drains non-blocking `inotify` events and performs bounded directory attribution scan.
    pub fn sample(
        &mut self,
        mounts: &[MountUsage],
        processes: &[ProcessSample],
    ) -> (Vec<FsChurnEntry>, Vec<FsEventRecord>) {
        let now_ms = Utc::now().timestamp_millis();
        let mut events = self.drain_inotify_events(now_ms, mounts, processes);
        let mut entries: BTreeMap<String, FsChurnEntry> = BTreeMap::new();

        for root in &self.watch_roots {
            let Ok(read_dir) = fs::read_dir(root) else {
                continue;
            };
            let mut root_count = 0usize;
            for child in read_dir.flatten() {
                if root_count >= MAX_SCAN_ENTRIES_PER_ROOT {
                    break;
                }
                let child_path = child.path();
                let Ok(meta) = child_path.symlink_metadata() else {
                    continue;
                };
                if meta.is_symlink() {
                    continue;
                }
                root_count += 1;
                let (size_bytes, file_count, mtime_ms) = if meta.is_dir() {
                    bounded_dir_usage(&child_path, MAX_SCAN_DEPTH, MAX_SCAN_ENTRIES_PER_ROOT)
                } else {
                    (meta.len(), 1, modified_ms(&meta))
                };
                let path_str = child_path.to_string_lossy().into_owned();
                let prev = self.prev_sizes.get(&path_str).copied().unwrap_or(0);
                let delta = (size_bytes as i64).saturating_sub(prev as i64);
                self.prev_sizes.insert(path_str.clone(), size_bytes);

                let mount_point = resolve_mount_point(&child_path, mounts);
                let owner = match_owner_process(&path_str, processes);
                let unlinked_open = owner.map(|p| u64::from(p.unlinked_fds)).unwrap_or(0);

                // Keep entries >= 1 MiB or actively changing.
                if size_bytes >= 1024 * 1024 || delta != 0 || owner.is_some() {
                    let entry = FsChurnEntry {
                        path: path_str.clone(),
                        mount_point: mount_point.clone(),
                        size_bytes,
                        recent_delta_bytes: delta,
                        file_count,
                        unlinked_open_fds: unlinked_open,
                        mtime_unix_ms: mtime_ms,
                        owner_pid: owner.map(|p| p.pid),
                        owner_comm: owner.map(|p| p.comm.clone()),
                        owner_unit: owner.and_then(|p| p.unit.clone()),
                        owner_slice: owner.map(|p| p.slice.clone()),
                        owner_agent: owner.and_then(|p| p.agent_hint.clone()),
                    };
                    if delta != 0 {
                        events.push(FsEventRecord {
                            timestamp_ms: now_ms,
                            path: entry.path.clone(),
                            mount_point: entry.mount_point.clone(),
                            event_kind: if delta > 0 {
                                "grow".into()
                            } else {
                                "shrink".into()
                            },
                            size_bytes: entry.size_bytes,
                            delta_bytes: entry.recent_delta_bytes,
                            owner_pid: entry.owner_pid,
                            owner_unit: entry.owner_unit.clone().unwrap_or_default(),
                            owner_slice: entry.owner_slice.clone().unwrap_or_default(),
                        });
                    }
                    entries.insert(path_str, entry);
                }
            }
        }

        // Also ensure any open path held by an attributed process appears even if below 1 MiB.
        for proc_sample in processes {
            for open_path in &proc_sample.open_paths {
                let p = Path::new(open_path);
                let size_bytes = p
                    .symlink_metadata()
                    .ok()
                    .filter(|m| !m.is_symlink() && m.is_file())
                    .map_or(0, |m| m.len());
                let mount_point = resolve_mount_point(p, mounts);
                entries
                    .entry(open_path.clone())
                    .or_insert_with(|| FsChurnEntry {
                        path: open_path.clone(),
                        mount_point,
                        size_bytes,
                        recent_delta_bytes: size_bytes as i64,
                        file_count: 1,
                        unlinked_open_fds: u64::from(proc_sample.unlinked_fds),
                        mtime_unix_ms: now_ms,
                        owner_pid: Some(proc_sample.pid),
                        owner_comm: Some(proc_sample.comm.clone()),
                        owner_unit: proc_sample.unit.clone(),
                        owner_slice: Some(proc_sample.slice.clone()),
                        owner_agent: proc_sample.agent_hint.clone(),
                    });
            }
        }

        let mut list: Vec<FsChurnEntry> = entries.into_values().collect();
        list.sort_by(|a, b| {
            b.size_bytes
                .cmp(&a.size_bytes)
                .then_with(|| a.path.cmp(&b.path))
        });
        list.truncate(200);
        (list, events)
    }

    fn drain_inotify_events(
        &self,
        now_ms: i64,
        mounts: &[MountUsage],
        processes: &[ProcessSample],
    ) -> Vec<FsEventRecord> {
        let Some(fd) = &self.inotify_fd else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut buf = [MaybeUninit::uninit(); 1024];
        let mut reader = inotify::Reader::new(fd, &mut buf);
        for _ in 0..128 {
            match reader.next() {
                Ok(ev) => {
                    let Some(root) = self.wd_to_root.get(&ev.wd()) else {
                        continue;
                    };
                    let full = match ev.file_name() {
                        Some(name) => root.join(name.to_string_lossy().as_ref()),
                        None => root.clone(),
                    };
                    let path_str = full.to_string_lossy().into_owned();
                    let size_bytes = full
                        .symlink_metadata()
                        .ok()
                        .filter(|m| !m.is_symlink() && m.is_file())
                        .map_or(0, |m| m.len());
                    let mount_point = resolve_mount_point(&full, mounts);
                    let owner = match_owner_process(&path_str, processes);
                    events.push(FsEventRecord {
                        timestamp_ms: now_ms,
                        path: path_str,
                        mount_point,
                        event_kind: format!("inotify:{:?}", ev.events()),
                        size_bytes,
                        delta_bytes: 0,
                        owner_pid: owner.map(|p| p.pid),
                        owner_unit: owner.and_then(|p| p.unit.clone()).unwrap_or_default(),
                        owner_slice: owner.map(|p| p.slice.clone()).unwrap_or_default(),
                    });
                }
                Err(Errno::WOULDBLOCK) => break,
                Err(_) => break,
            }
        }
        events
    }
}

fn match_owner_process<'a>(
    candidate_path: &str,
    processes: &'a [ProcessSample],
) -> Option<&'a ProcessSample> {
    let prefix = format!("{candidate_path}/");
    // Prefer processes with an explicit matching open FD inside or matching candidate_path.
    if let Some(p) = processes.iter().find(|p| {
        p.open_paths
            .iter()
            .any(|op| op == candidate_path || op.starts_with(&prefix))
    }) {
        return Some(p);
    }
    // Fallback to matching CWD prefix for workspace target/ churn.
    processes.iter().find(|p| {
        p.cwd
            .as_deref()
            .is_some_and(|c| candidate_path == c || candidate_path.starts_with(&format!("{c}/")))
    })
}

fn bounded_dir_usage(root: &Path, max_depth: usize, max_entries: usize) -> (u64, u64, i64) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut latest_mtime = 0i64;
    let mut visited = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if visited >= max_entries {
                return (bytes, files, latest_mtime);
            }
            visited += 1;
            let p = entry.path();
            let Ok(meta) = p.symlink_metadata() else {
                continue;
            };
            if meta.is_symlink() {
                continue;
            }
            latest_mtime = latest_mtime.max(modified_ms(&meta));
            if meta.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
            } else if meta.is_file() {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    (bytes, files, latest_mtime)
}

fn modified_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}
