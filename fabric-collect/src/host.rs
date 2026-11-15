//! Kernel PSI, memory/swap/zram0, mount utilization (`statvfs`), and optional GPU sysfs readers.

use fabric_core::{GpuDeviceStats, MemorySnapshot, MountUsage, PsiMetrics, PsiResource, ZramStats};
use rustix::fs::statvfs;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Reads host hostname cleanly from `/etc/hostname` or `/proc/sys/kernel/hostname`.
#[must_use]
pub fn read_hostname() -> String {
    for p in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(raw) = fs::read_to_string(p) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    "localhost".into()
}

/// Reads `/proc/pressure/{cpu,memory,io}` cleanly (returns zeros if PSI is disabled).
#[must_use]
pub fn read_psi_metrics() -> PsiMetrics {
    let (cpu_some, cpu_full) = read_psi_file("/proc/pressure/cpu");
    let (memory_some, memory_full) = read_psi_file("/proc/pressure/memory");
    let (io_some, io_full) = read_psi_file("/proc/pressure/io");
    PsiMetrics {
        cpu_some,
        cpu_full,
        memory_some,
        memory_full,
        io_some,
        io_full,
    }
}

fn read_psi_file(path: &str) -> (PsiResource, PsiResource) {
    let Ok(text) = fs::read_to_string(path) else {
        return (PsiResource::default(), PsiResource::default());
    };
    let mut some = PsiResource::default();
    let mut full = PsiResource::default();
    for line in text.lines() {
        let parsed = parse_psi_line(line);
        if line.starts_with("some ") {
            some = parsed;
        } else if line.starts_with("full ") {
            full = parsed;
        }
    }
    (some, full)
}

fn parse_psi_line(line: &str) -> PsiResource {
    let mut res = PsiResource::default();
    for part in line.split_whitespace().skip(1) {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "avg10" => res.avg10 = v.parse().unwrap_or(0.0),
                "avg60" => res.avg60 = v.parse().unwrap_or(0.0),
                "avg300" => res.avg300 = v.parse().unwrap_or(0.0),
                "total" => res.total_usec = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    res
}

/// Reads `/proc/meminfo` and `/sys/block/zram0/mm_stat`.
#[must_use]
pub fn read_memory_snapshot() -> MemorySnapshot {
    let mut mem_total = 0u64;
    let mut mem_avail = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else {
                continue;
            };
            let Some(val_kb) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
                continue;
            };
            let bytes = val_kb.saturating_mul(1024);
            match key {
                "MemTotal:" => mem_total = bytes,
                "MemAvailable:" => mem_avail = bytes,
                "SwapTotal:" => swap_total = bytes,
                "SwapFree:" => swap_free = bytes,
                _ => {}
            }
        }
    }

    let zram = read_zram0_stats();

    MemorySnapshot {
        mem_total_bytes: mem_total,
        mem_available_bytes: mem_avail,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_total.saturating_sub(swap_free),
        zram,
    }
}

fn read_zram0_stats() -> Option<ZramStats> {
    let raw = fs::read_to_string("/sys/block/zram0/mm_stat").ok()?;
    let nums: Vec<u64> = raw
        .split_whitespace()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let orig = nums[0];
    let compr = nums[1];
    let mem_used_total = nums[2];
    let mem_limit = nums[3];
    let same_pages = nums.get(5).copied().unwrap_or(0);
    let pages_compacted = nums.get(6).copied().unwrap_or(0);
    let compression_ratio = if compr > 0 {
        (orig as f64) / (compr as f64)
    } else {
        1.0
    };
    Some(ZramStats {
        device: "zram0".into(),
        orig_data_size_bytes: orig,
        compr_data_size_bytes: compr,
        mem_used_total_bytes: mem_used_total,
        mem_limit_bytes: mem_limit,
        compression_ratio,
        same_pages,
        pages_compacted,
    })
}

/// Samples key mount points via `rustix::fs::statvfs` enriched with `/proc/mounts` fstype.
#[must_use]
pub fn read_mount_usages() -> Vec<MountUsage> {
    let mut fs_types = std::collections::BTreeMap::<String, String>::new();
    if let Ok(mounts_txt) = fs::read_to_string("/proc/mounts") {
        for line in mounts_txt.lines() {
            let mut cols = line.split_whitespace();
            let _dev = cols.next();
            let Some(mp) = cols.next() else {
                continue;
            };
            let Some(fstype) = cols.next() else {
                continue;
            };
            fs_types.insert(mp.to_owned(), fstype.to_owned());
        }
    }

    let targets = ["/", "/home", "/data", "/tmp"];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for mp in targets {
        if !Path::new(mp).exists() || !seen.insert(mp) {
            continue;
        }
        let Ok(st) = statvfs(mp) else {
            continue;
        };
        let frsize = if st.f_frsize > 0 {
            st.f_frsize
        } else {
            st.f_bsize
        };
        let total_bytes = st.f_blocks.saturating_mul(frsize);
        let available_bytes = st.f_bavail.saturating_mul(frsize);
        let free_bytes = st.f_bfree.saturating_mul(frsize);
        let used_bytes = total_bytes.saturating_sub(free_bytes);
        let used_pct = if total_bytes > 0 {
            ((used_bytes as f64) / (total_bytes as f64)) * 100.0
        } else {
            0.0
        };
        out.push(MountUsage {
            mount_point: mp.to_owned(),
            fs_type: fs_types
                .get(mp)
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            total_bytes,
            available_bytes,
            used_bytes,
            used_pct,
        });
    }
    out
}

/// Reads optional DRM/NVIDIA sysfs telemetry without spawning subprocesses.
#[must_use]
pub fn read_gpu_stats() -> Vec<GpuDeviceStats> {
    let mut gpus = Vec::new();
    let drm = Path::new("/sys/class/drm");
    if let Ok(entries) = fs::read_dir(drm) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = entry.path().join("device");
            if !dev.exists() {
                continue;
            }
            let vendor = fs::read_to_string(dev.join("vendor"))
                .map(|s| s.trim().to_owned())
                .unwrap_or_else(|_| "unknown".into());
            let vram_used_bytes = fs::read_to_string(dev.join("mem_info_vram_used"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            let vram_total_bytes = fs::read_to_string(dev.join("mem_info_vram_total"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            let busy_pct = fs::read_to_string(dev.join("gpu_busy_percent"))
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok());

            gpus.push(GpuDeviceStats {
                card: name,
                vendor,
                vram_used_bytes,
                vram_total_bytes,
                busy_pct,
                temperature_c: None,
            });
        }
    }
    gpus.sort_by(|a, b| a.card.cmp(&b.card));
    gpus
}

/// Resolves which monitored mount point owns `path`.
#[must_use]
pub fn resolve_mount_point(path: &Path, mounts: &[MountUsage]) -> String {
    let path_str = path.to_string_lossy();
    let mut best = "/".to_owned();
    let mut best_len = 1usize;
    for m in mounts {
        let mp = m.mount_point.as_str();
        if (path_str == mp || (mp != "/" && path_str.starts_with(&format!("{mp}/"))))
            && mp.len() >= best_len
        {
            best = mp.to_owned();
            best_len = mp.len();
        }
    }
    best
}
