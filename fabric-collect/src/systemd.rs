//! Systemd unit & transient scope hygiene detector (flags missing `--collect` and failed units).

use fabric_core::{CgroupSample, ProcessSample, UnitRecord};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Discovers user & cgroup systemd units and flags orphaned/failed transient scopes.
#[must_use]
pub fn collect_unit_records(
    cgroups: &[CgroupSample],
    processes: &[ProcessSample],
) -> Vec<UnitRecord> {
    let mut records: BTreeMap<String, UnitRecord> = BTreeMap::new();

    for cg in cgroups {
        let Some(unit_name) = &cg.unit else {
            continue;
        };
        let manager = if cg.slice == "system.slice" {
            "system".to_owned()
        } else {
            "user".to_owned()
        };
        let entry = records
            .entry(unit_name.clone())
            .or_insert_with(|| UnitRecord {
                name: unit_name.clone(),
                manager,
                slice: cg.slice.clone(),
                active_state: if cg.populated {
                    "active".into()
                } else {
                    "inactive".into()
                },
                sub_state: if cg.populated {
                    "running".into()
                } else {
                    "dead".into()
                },
                transient: false,
                collect_mode_enabled: false,
                orphaned_reason: None,
                main_pid: processes
                    .iter()
                    .find(|p| p.unit.as_deref() == Some(unit_name.as_str()))
                    .map(|p| p.pid),
                memory_current_bytes: cg.memory_current_bytes,
                cpu_usage_usec: cg.cpu_usage_usec,
                populated: cg.populated,
            });
        entry.memory_current_bytes = entry.memory_current_bytes.max(cg.memory_current_bytes);
        entry.cpu_usage_usec = entry.cpu_usage_usec.max(cg.cpu_usage_usec);
        entry.populated |= cg.populated;
    }

    // Inspect user transient unit definitions under $XDG_RUNTIME_DIR/systemd/transient.
    if let Some(transient_dir) = transient_units_dir()
        && let Ok(entries) = fs::read_dir(&transient_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.ends_with(".service") || name.ends_with(".scope")) {
                continue;
            }
            let raw = fs::read_to_string(&path).unwrap_or_default();
            let has_collect = raw.lines().any(|line| {
                let l = line.trim();
                l == "CollectMode=inactive-or-failed" || l == "CollectMode=inactive"
            });
            let slice = raw
                .lines()
                .find_map(|l| l.trim().strip_prefix("Slice="))
                .map(str::to_owned)
                .unwrap_or_else(|| "app.slice".into());

            let rec = records.entry(name.clone()).or_insert_with(|| UnitRecord {
                name,
                manager: "user".into(),
                slice,
                active_state: "unknown".into(),
                sub_state: "unknown".into(),
                transient: true,
                collect_mode_enabled: has_collect,
                orphaned_reason: None,
                main_pid: None,
                memory_current_bytes: 0,
                cpu_usage_usec: 0,
                populated: false,
            });
            rec.transient = true;
            rec.collect_mode_enabled = has_collect;
        }
    }

    // Optionally enrich active/failed states via user systemd manager.
    enrich_from_systemctl(&mut records);

    // Flag failed units and transient units missing `--collect` or lingering unpopulated scopes.
    for rec in records.values_mut() {
        if rec.active_state == "failed" {
            rec.orphaned_reason = Some(format!(
                "unit entered failed state ({}/{})",
                rec.active_state, rec.sub_state
            ));
        } else if rec.transient && !rec.collect_mode_enabled {
            rec.orphaned_reason = Some(
                "transient unit created without --collect (CollectMode=inactive-or-failed)".into(),
            );
        } else if rec.transient && !rec.populated && rec.active_state != "active" {
            rec.orphaned_reason =
                Some("transient scope unpopulated yet retained in systemd unit table".into());
        }
    }

    records.into_values().collect()
}

fn transient_units_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|p| p.join("systemd/transient"))
}

fn enrich_from_systemctl(records: &mut BTreeMap<String, UnitRecord>) {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
        ])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let unit = cols[0];
        if !(unit.ends_with(".service") || unit.ends_with(".scope")) {
            continue;
        }
        let active = cols[2];
        let sub = cols[3];
        if let Some(rec) = records.get_mut(unit) {
            rec.active_state = active.to_owned();
            rec.sub_state = sub.to_owned();
        } else if active == "failed" {
            records.insert(
                unit.to_owned(),
                UnitRecord {
                    name: unit.to_owned(),
                    manager: "user".into(),
                    slice: "user.slice".into(),
                    active_state: active.to_owned(),
                    sub_state: sub.to_owned(),
                    transient: false,
                    collect_mode_enabled: false,
                    orphaned_reason: None,
                    main_pid: None,
                    memory_current_bytes: 0,
                    cpu_usage_usec: 0,
                    populated: false,
                },
            );
        }
    }
}
