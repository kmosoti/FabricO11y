//! Sanitized human/table terminal renderers for `fabric` CLI outputs.

use crate::CustodyStatusReport;
use fabric_core::{
    BlameReport, DoctorReport, DriftReport, HostSnapshot, sanitize_terminal_text as esc,
};
use fabric_graph::LineageSubgraph;
use std::io::Write;

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", (bytes as f64) / (1024.0 * 1024.0))
}

fn sanitize_and_truncate(s: &str, max: usize) -> String {
    let clean = esc(s);
    if clean.chars().count() <= max {
        clean
    } else if max > 3 {
        let mut out: String = clean.chars().take(max - 3).collect();
        out.push_str("...");
        out
    } else {
        clean.chars().take(max).collect()
    }
}

pub(crate) fn render_snapshot_table(out: &mut impl Write, s: &HostSnapshot) -> std::io::Result<()> {
    writeln!(
        out,
        "HOST {} | mem_avail={} | swap_used={} | psi(mem={:.2}, io={:.2}) | self_rss={}",
        esc(&s.hostname),
        mib(s.memory.mem_available_bytes),
        mib(s.memory.swap_used_bytes),
        s.psi.memory_some.avg10,
        s.psi.io_some.avg10,
        mib(s.self_health.rss_bytes),
    )?;
    writeln!(
        out,
        "{:<20} {:>12} {:>12} {:>8}",
        "SLICE", "MEM_CUR", "MEM_HIGH", "PIDS"
    )?;
    for sl in &s.slices {
        let high = sl.memory_high_bytes.map_or_else(|| "max".into(), mib);
        writeln!(
            out,
            "{:<20} {:>12} {:>12} {:>8}",
            esc(&sl.slice_name),
            mib(sl.memory_current_bytes),
            high,
            sl.pids_current
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{:<12} {:<10} {:>12} {:>12} {:>8}",
        "MOUNT", "FSTYPE", "USED", "AVAIL", "USED%"
    )?;
    for m in &s.mounts {
        writeln!(
            out,
            "{:<12} {:<10} {:>12} {:>12} {:>7.1}%",
            esc(&m.mount_point),
            esc(&m.fs_type),
            mib(m.used_bytes),
            mib(m.available_bytes),
            m.used_pct
        )?;
    }
    Ok(())
}

pub(crate) fn render_query_table(
    out: &mut impl Write,
    domain: &str,
    rows: &[serde_json::Value],
) -> std::io::Result<()> {
    writeln!(out, "QUERY domain={} rows={}", esc(domain), rows.len())?;
    let Some(first) = rows.first().and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let cols: Vec<&String> = first.keys().take(7).collect();
    for c in &cols {
        write!(out, "{:<20} ", sanitize_and_truncate(c, 20))?;
    }
    writeln!(out)?;
    for row in rows.iter().take(50) {
        if let Some(obj) = row.as_object() {
            for c in &cols {
                let s = match obj.get(*c) {
                    Some(serde_json::Value::String(v)) => v.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                write!(out, "{:<20} ", sanitize_and_truncate(&s, 20))?;
            }
            writeln!(out)?;
        }
    }
    Ok(())
}

pub(crate) fn render_lineage_table(
    out: &mut impl Write,
    sub: &LineageSubgraph,
) -> std::io::Result<()> {
    let matched: Vec<String> = sub.matched_roots.iter().map(|r| esc(r)).collect();
    writeln!(
        out,
        "LINEAGE target={} matched={:?} nodes={} edges={}",
        esc(&sub.target),
        matched,
        sub.nodes.len(),
        sub.edges.len()
    )?;
    writeln!(out, "{:<40} {:<16} {:<30}", "NODE_ID", "KIND", "LABEL")?;
    for n in &sub.nodes {
        writeln!(
            out,
            "{:<40} {:<16} {:<30}",
            sanitize_and_truncate(&n.id, 40),
            esc(&n.kind.to_string()),
            sanitize_and_truncate(&n.label, 30)
        )?;
    }
    for e in &sub.edges {
        writeln!(
            out,
            "  {} --[{}]--> {}",
            esc(&e.src),
            esc(&e.kind.to_string()),
            esc(&e.dst)
        )?;
    }
    Ok(())
}

pub(crate) fn render_blame_table(
    out: &mut impl Write,
    report: &BlameReport,
) -> std::io::Result<()> {
    writeln!(
        out,
        "BLAME resource={} | {}",
        esc(&report.resource.to_string()),
        esc(&report.pressure_summary)
    )?;
    writeln!(
        out,
        "{:<4} {:<26} {:>12} {:>7} {:<14} {:<28}",
        "RANK", "LABEL", "VALUE", "SHARE", "SLICE", "UNIT/PATH"
    )?;
    for e in &report.entries {
        let val = if e.unit_label == "bytes" {
            mib(e.contribution_value)
        } else {
            format!("{} {}", e.contribution_value, esc(&e.unit_label))
        };
        let extra = e
            .target_path
            .as_deref()
            .or(e.unit.as_deref())
            .unwrap_or("-");
        writeln!(
            out,
            "{:<4} {:<26} {:>12} {:>6.1}% {:<14} {:<28}",
            e.rank,
            sanitize_and_truncate(&e.label, 26),
            val,
            e.share_pct,
            sanitize_and_truncate(&e.slice, 14),
            sanitize_and_truncate(extra, 28)
        )?;
    }
    Ok(())
}

pub(crate) fn render_drift_table(
    out: &mut impl Write,
    report: &DriftReport,
) -> std::io::Result<()> {
    writeln!(
        out,
        "DRIFT findings={} critical={} warn={} info={}",
        report.total_findings, report.critical_count, report.warn_count, report.info_count
    )?;
    writeln!(
        out,
        "{:<14} {:<10} {:<28} SUMMARY",
        "PILLAR", "SEVERITY", "TARGET"
    )?;
    for f in &report.findings {
        writeln!(
            out,
            "{:<14} {:<10} {:<28} {}",
            esc(&f.pillar.to_string()),
            esc(&f.severity.to_string()),
            sanitize_and_truncate(&f.target, 28),
            esc(&f.summary)
        )?;
    }
    Ok(())
}

pub(crate) fn render_doctor_table(
    out: &mut impl Write,
    report: &DoctorReport,
) -> std::io::Result<()> {
    writeln!(
        out,
        "DOCTOR healthy={} daemon_running={} pid={} rss={} fds={} tmp_fds={} slice={}",
        report.healthy,
        report.daemon_running,
        report.pid,
        mib(report.rss_bytes),
        report.open_fds,
        report.tmp_files_created,
        esc(&report.cgroup_path)
    )?;
    writeln!(
        out,
        "{:<22} {:<8} {:<32} THRESHOLD",
        "CHECK", "STATUS", "ACTUAL"
    )?;
    for c in &report.checks {
        writeln!(
            out,
            "{:<22} {:<8} {:<32} {}",
            esc(&c.name),
            if c.passed { "PASS" } else { "FAIL" },
            sanitize_and_truncate(&c.actual, 32),
            esc(&c.threshold)
        )?;
    }
    Ok(())
}

pub(crate) fn render_custody_table(
    out: &mut impl Write,
    report: &CustodyStatusReport,
) -> std::io::Result<()> {
    writeln!(
        out,
        "SPINDLE online={} socket={} origin={} records={} highest={} canonical={} reclaimed={}",
        report.spindle_online,
        esc(report.spindle_socket.as_deref().unwrap_or("-")),
        esc(report.spindle_origin.as_deref().unwrap_or("-")),
        report.records,
        report.highest_record_id,
        report.canonical_through,
        report.reclaimed_through,
    )?;
    if let Some(err) = &report.spindle_error {
        writeln!(out, "  spindle_error={}", esc(err))?;
    }
    if let Some(err) = &report.forwarding_error {
        writeln!(out, "  forwarding_error={}", esc(err))?;
    }
    writeln!(
        out,
        "SERVER  online={} endpoint={} authority={}",
        report.server_online,
        esc(report.server_endpoint.as_deref().unwrap_or("-")),
        esc(report.server_authority.as_deref().unwrap_or("-")),
    )?;
    if let Some(err) = &report.server_error {
        writeln!(out, "  server_error={}", esc(err))?;
    }
    for (origin, frontier) in &report.server_frontiers {
        writeln!(out, "  frontier origin={} through={frontier}", esc(origin))?;
    }
    writeln!(
        out,
        "COLLECTOR spindle_watermark_id={}",
        report
            .collector_spindle_watermark_id
            .map_or_else(|| "none".to_owned(), |id| id.to_string())
    )?;
    if let Some(err) = &report.catalog_error {
        writeln!(out, "  catalog_error={}", esc(err))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_and_custody_tables_escape_ansi_osc_c1_and_bidi_controls() {
        let mut buf = Vec::new();
        let rows = vec![json!({
            "msg\x1b[31m": "hello\x1b]0;title\x07\u{009b}2J\u{202e}world\x7f\nline2"
        })];
        render_query_table(&mut buf, "logs\x1b[2J", &rows).unwrap();
        let rendered = String::from_utf8(buf).unwrap();
        assert!(!rendered.contains('\x1b'));
        assert!(!rendered.contains('\x07'));
        assert!(!rendered.contains('\x7f'));
        assert!(!rendered.contains('\u{009b}'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(!rendered.contains("line2\n\n"));

        let mut custody_buf = Vec::new();
        let report = CustodyStatusReport {
            spindle_socket: Some("/run/\x1b]8;;https://evil\x07sock".into()),
            spindle_online: false,
            spindle_origin: Some("origin\x08\u{009d}osc".into()),
            spindle_error: Some("bad\x1b[31merror\r\nspoof".into()),
            records: 1,
            highest_record_id: 1,
            canonical_through: 0,
            reclaimed_through: 0,
            forwarding_error: None,
            server_endpoint: None,
            server_online: false,
            server_authority: None,
            server_error: Some("srv\x00err".into()),
            server_frontiers: [("org\x1b[1m".into(), 3)].into_iter().collect(),
            collector_spindle_watermark_id: None,
            catalog_error: Some("cat\x1b[0merr".into()),
        };
        render_custody_table(&mut custody_buf, &report).unwrap();
        let out = String::from_utf8(custody_buf).unwrap();
        for bad in ['\x1b', '\x07', '\x08', '\x00', '\r', '\u{009d}'] {
            assert!(!out.contains(bad), "raw control {bad:?} leaked in {out:?}");
        }
        assert!(out.contains("\\x1b[31m"));
        assert!(!out.contains("\nspoof"));
    }
}
