//! Shared CLI subprocess runner and bundle assembly helpers for `fabric-client`.

use crate::schema::{
    FabricBlame, FabricBundle, FabricCustody, FabricDoctor, FabricDrift, FabricMetricQuery,
    FabricSnapshot, parse_envelope,
};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;

/// Writes a 4-byte big-endian length-prefixed frame and reads one bounded reply frame.
pub(crate) async fn exchange_len_frame<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    payload: &[u8],
    max_reply: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(|e| format!("{label} write header: {}", e.kind()))?;
    stream
        .write_all(payload)
        .await
        .map_err(|e| format!("{label} write body: {}", e.kind()))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("{label} flush: {}", e.kind()))?;
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| format!("{label} read header: {}", e.kind()))?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > max_reply {
        return Err(format!("{label} reply exceeded frame limit"));
    }
    let mut body = vec![0u8; size];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("{label} read body: {}", e.kind()))?;
    Ok(body)
}

pub(crate) fn assemble_bundle(
    fallback_source: String,
    extended: bool,
    snap_res: Result<FabricSnapshot, String>,
    cust_res: Result<FabricCustody, String>,
    blame_res: Result<FabricBlame, String>,
    drift_res: Result<FabricDrift, String>,
    doc_res: Result<FabricDoctor, String>,
) -> FabricBundle {
    let mut errs = Vec::new();
    let snapshot = collect_opt(snap_res, "snapshot", &mut errs);
    let custody = collect_opt(cust_res, "custody", &mut errs);
    let (blame_memory, drift, doctor) = if extended {
        (
            collect_opt(blame_res, "blame", &mut errs),
            collect_opt(drift_res, "drift", &mut errs),
            collect_opt(doc_res, "doctor", &mut errs),
        )
    } else {
        (None, None, None)
    };
    let metric_series = snapshot
        .as_ref()
        .map(|s| vec![(s.timestamp_unix_ms, s.memory.mem_available_bytes as f64)])
        .unwrap_or_default();
    let online = snapshot.is_some() || custody.is_some() || doctor.is_some();
    let source_desc = snapshot
        .as_ref()
        .map(|s| format!("fabric@{}", s.hostname))
        .unwrap_or(fallback_source);
    let (error, warnings) = if online {
        (None, errs)
    } else if errs.is_empty() {
        (None, Vec::new())
    } else {
        (Some(errs.join("; ")), Vec::new())
    };
    FabricBundle {
        online,
        source_desc,
        error,
        warnings,
        snapshot,
        custody,
        blame_memory,
        drift,
        doctor,
        metric_series,
    }
}

fn collect_opt<T>(res: Result<T, String>, label: &str, errs: &mut Vec<String>) -> Option<T> {
    match res {
        Ok(v) => Some(v),
        Err(e) => {
            errs.push(format!("{label}: {e}"));
            None
        }
    }
}

pub(crate) async fn run_json<T: DeserializeOwned>(
    bin: &Path,
    data_dir: &Option<PathBuf>,
    args: &[&str],
    timeout: Duration,
) -> Result<T, String> {
    let mut attempt = 0u8;
    let out = loop {
        let mut cmd = Command::new(bin);
        if let Some(dir) = data_dir {
            cmd.arg("--data-dir").arg(dir);
        }
        cmd.args(args)
            .env_remove("FABRIC_CREDENTIAL")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(o)) => break o,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 4 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(e)) => return Err(format!("spawn failed: {e}")),
            Err(_) => return Err("timeout".to_owned()),
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            format!("exit {}", out.status)
        } else {
            msg.chars().take(120).collect()
        });
    }
    Ok(parse_envelope::<T>(&out.stdout)?.data)
}

/// Resolves an executable via explicit env override, `$PATH`, or `~/.local/bin/<name>`.
pub(crate) fn resolve_binary(env_override: &str, bin_name: &str) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(env_override) {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join(bin_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let local = PathBuf::from(home).join(".local/bin").join(bin_name);
        if local.is_file() {
            return Some(local);
        }
    }
    None
}

pub(crate) async fn collect_via_cli(
    bin_path: Option<&Path>,
    data_dir: &Option<PathBuf>,
    extended: bool,
    fast_timeout: Duration,
) -> FabricBundle {
    let Some(bin) = bin_path else {
        return FabricBundle {
            online: false,
            source_desc: "fabric binary not found".into(),
            error: Some("Install FabricO11y or set FABRIC_BIN".into()),
            ..FabricBundle::default()
        };
    };

    let (snap_res, cust_res, blame_res, drift_res, doc_res) = if extended {
        tokio::join!(
            run_json::<FabricSnapshot>(bin, data_dir, &["snapshot", "--json"], fast_timeout),
            run_json::<FabricCustody>(bin, data_dir, &["custody", "--json"], fast_timeout),
            run_json::<FabricBlame>(bin, data_dir, &["blame", "memory", "--json"], fast_timeout),
            run_json::<FabricDrift>(bin, data_dir, &["drift", "--json"], fast_timeout),
            run_json::<FabricDoctor>(bin, data_dir, &["doctor", "--json"], fast_timeout),
        )
    } else {
        let (s, c) = tokio::join!(
            run_json::<FabricSnapshot>(bin, data_dir, &["snapshot", "--json"], fast_timeout),
            run_json::<FabricCustody>(bin, data_dir, &["custody", "--json"], fast_timeout),
        );
        (
            s,
            c,
            Err("skipped on fast poll".into()),
            Err("skipped on fast poll".into()),
            Err("skipped on fast poll".into()),
        )
    };

    assemble_bundle(
        bin.display().to_string(),
        extended,
        snap_res,
        cust_res,
        blame_res,
        drift_res,
        doc_res,
    )
}

pub(crate) async fn query_mem_history_cli(
    bin: &Path,
    data_dir: &Option<PathBuf>,
    query_timeout: Duration,
) -> Result<Vec<(u64, f64)>, String> {
    let args = [
        "query",
        "metrics",
        "--since",
        "5m",
        "--where",
        "name=host.mem.available_bytes",
        "--limit",
        "24",
        "--json",
    ];
    let q = run_json::<FabricMetricQuery>(bin, data_dir, &args, query_timeout).await?;
    let mut series: Vec<(u64, f64)> = q
        .rows
        .into_iter()
        .map(|r| (r.timestamp_ms, r.value))
        .collect();
    series.sort_by_key(|&(ts, _)| ts);
    Ok(series)
}
