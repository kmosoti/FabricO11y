//! Reads pinned native capture records and composes the existing v2 projection.
//! It selects observations only; cutoff semantics stay in the established verifier.
use super::{array, keys, number, require, text};
use crate::preflight::ArtifactStore;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const MAX_COMMANDS: usize = 4096;
const MAX_SAMPLES: usize = 4096;
const MAX_EVENTS: usize = 65_536;
const MAX_SOURCES: usize = 65_536;

fn canonical(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_member(
    files: &mut ArtifactStore,
    root: &str,
    record: &Value,
    paths: &mut BTreeSet<String>,
) -> Result<Value, String> {
    keys(record, &["path", "sha256"])?;
    let path = text(record, "path")?;
    let prefix = format!("{root}/");
    require(canonical(path) && path.starts_with(&prefix), "capture member outside phase root")?;
    require(digest(text(record, "sha256")?), "capture member SHA-256 required")?;
    require(paths.insert(path.to_owned()), "capture member path reused")?;
    files.plan(path, text(record, "sha256")?)
}

fn pin_inspection_bodies(
    commands: &[Value],
    files: &mut ArtifactStore,
    root: &str,
) -> Result<(), String> {
    for command in commands {
        let Some(artifact) = command.get("stdout_artifact") else {
            continue;
        };
        keys(artifact, &["path", "sha256", "encoding"])?;
        require(
            text(artifact, "encoding")? == "utf-8-from-subprocess-text",
            "inspection encoding mismatch",
        )?;
        let relative = text(artifact, "path")?;
        require(canonical(relative), "noncanonical inspection path")?;
        let path = format!("{root}/{relative}");
        let pin = text(artifact, "sha256")?;
        require(digest(pin), "inspection SHA-256 required")?;
        let body = files.read_bytes(&path, pin)?;
        require(body.len() <= 4096, "inspection body exceeds 4096 bytes")?;
    }
    Ok(())
}

pub(super) fn verify_capture(
    root: &Path,
    manifest_path: &str,
    manifest_sha256: &str,
    candidate: &str,
) -> Result<Value, String> {
    super::hex::<20>(candidate)?;
    require(canonical(manifest_path), "canonical capture manifest path required")?;
    require(digest(manifest_sha256), "capture manifest SHA-256 required")?;
    let mut files = ArtifactStore::new(root)?;
    let manifest = files.plan(manifest_path, manifest_sha256)?;
    keys(
        &manifest,
        &[
            "version",
            "candidate",
            "phase",
            "prior_ack",
            "relative_phase_root",
            "commands",
            "observed_events",
            "process_observations",
            "stopped_snapshot",
        ],
    )?;
    require(number(&manifest, "version")? == 1, "capture manifest version")?;
    require(text(&manifest, "candidate")? == candidate, "capture candidate mismatch")?;
    require(
        text(&manifest, "phase")? == "drain" || text(&manifest, "phase")? == "reboot",
        "capture phase required",
    )?;
    let phase_root = text(&manifest, "relative_phase_root")?;
    require(canonical(phase_root), "canonical phase root required")?;
    let prior_ack = number(&manifest, "prior_ack")?;

    let mut paths = BTreeSet::new();
    paths.insert(manifest_path.to_owned());
    let commands = read_member(&mut files, phase_root, &manifest["commands"], &mut paths)?;
    let events = read_member(&mut files, phase_root, &manifest["observed_events"], &mut paths)?;
    let samples = read_member(
        &mut files,
        phase_root,
        &manifest["process_observations"],
        &mut paths,
    )?;
    let snapshot = read_member(
        &mut files,
        phase_root,
        &manifest["stopped_snapshot"],
        &mut paths,
    )?;
    let commands = commands.as_array().ok_or("commands array required")?;
    require(commands.len() <= MAX_COMMANDS, "command count exceeds bound")?;
    let events = events.as_array().ok_or("observed events array required")?;
    require(events.len() <= MAX_EVENTS, "journal event count exceeds bound")?;
    let samples = samples.as_array().ok_or("process observations array required")?;
    require(samples.len() <= MAX_SAMPLES, "process sample count exceeds bound")?;
    keys(
        &snapshot,
        &["node_id", "generation", "sources", "candidate_process"],
    )?;
    let sources = array(&snapshot, "sources", MAX_SOURCES)?;
    let mut selected_sources = Vec::with_capacity(sources.len());
    for source in sources {
        keys(source, &["sequence", "sha256"])?;
        selected_sources.push(json!({
            "sequence": number(source, "sequence")?,
            "sha256": text(source, "sha256")?
        }));
    }
    pin_inspection_bodies(commands, &mut files, phase_root)?;
    let v2 = json!({
        "version": 2,
        "candidate": candidate,
        "phase": text(&manifest, "phase")?,
        "prior_ack": prior_ack,
        "relative_phase_root": phase_root,
        "commands": commands,
        "observed_events": events,
        "process_observations": samples,
        "stopped_snapshot": {
            "node_id": text(&snapshot, "node_id")?,
            "generation": number(&snapshot, "generation")?,
            "sources": selected_sources,
            "candidate_process": snapshot["candidate_process"]
        }
    });
    let plan = super::projection::normalize(v2, candidate)?;
    super::verify_value(&mut files, plan, candidate)
}
