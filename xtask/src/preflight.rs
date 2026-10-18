//! Read-only structural joins of caller-pinned evidence, never job execution or
//! acceptance. See docs/experiments/formal/engineering-evidence-protocol.md.
mod files;
mod graph;
mod json;

pub use files::{ArtifactStore, admitted_bytes};
pub use graph::ordered_jobs;
pub use json::strict_json;

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn keys<'a>(
    value: &'a Value,
    keys: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>, String> {
    let object = value.as_object().ok_or_else(|| format!("{label} object"))?;
    require(
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key)),
        &format!("{label} keys"),
    )?;
    Ok(object)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} string"))
}

fn exit(value: Option<&Value>, expected: u64) -> Result<(), String> {
    require(
        value.and_then(Value::as_u64) == Some(expected),
        "exit mismatch or type",
    )
}

struct Artifact<'a> {
    path: &'a str,
    sha256: &'a str,
}

fn command(value: &Value) -> Result<(), String> {
    let command = value.as_array().ok_or("command list")?;
    require(!command.is_empty() && command.len() <= 256, "command count")?;
    for argument in command {
        let argument = argument.as_str().ok_or("command argument string")?;
        require(
            !argument.is_empty() && argument.len() <= 1024 * 1024,
            "command argument size",
        )?;
    }
    Ok(())
}

fn receipt_path<'a>(path: &'a str, root: &str) -> Result<&'a str, String> {
    let path = if path.starts_with('/') {
        path.strip_prefix(&format!("{}/", root.trim_end_matches('/')))
            .ok_or("receipt path outside root")?
    } else {
        path
    };
    files::parts(path)?;
    Ok(path)
}

fn join<'a>(
    job: &Value,
    wrapper: &Value,
    resource: &'a Value,
    artifact: &Artifact<'_>,
    root: &str,
) -> Result<&'a str, String> {
    let command = &job["command"];
    let revision = text(job, "harness_revision")?;
    let expected = job["expected_exit"].as_u64().ok_or("expected exit")?;
    require(
        wrapper.get("state").and_then(Value::as_str) == Some("completed")
            && wrapper.get("command") == Some(command),
        "wrapper state/command",
    )?;
    require(
        text(wrapper, "harness_revision")? == revision,
        "wrapper revision mismatch",
    )?;
    require(
        text(wrapper, "source_status_before")?.is_empty()
            && text(wrapper, "source_status_after")?.is_empty(),
        "dirty source status",
    )?;
    exit(wrapper.get("exit"), expected)?;
    exit(wrapper.get("launcher_exit"), expected)?;
    let entries = wrapper
        .get("resource_receipts")
        .and_then(Value::as_array)
        .ok_or("resource receipt list")?;
    require(
        entries.len() == 1 && entries[0].is_object(),
        "exactly one resource receipt required",
    )?;
    let entry = &entries[0];
    require(
        receipt_path(text(entry, "path")?, root)? == artifact.path
            && text(entry, "sha256")? == artifact.sha256,
        "resource pin mismatch",
    )?;
    exit(entry.get("exit"), expected)?;
    require(
        entry.get("stop_confirmed") == Some(&Value::Bool(true)),
        "wrapper stop unconfirmed",
    )?;
    require(
        resource.get("command") == Some(command) && text(resource, "storage_root")? == root,
        "resource command/storage root",
    )?;
    exit(resource.get("exit"), expected)?;
    exit(resource.get("wrapper_exit"), expected)?;
    require(
        resource.get("stop_confirmed") == Some(&Value::Bool(true))
            && resource.get("stop_error") == Some(&Value::Null),
        "resource stop unconfirmed",
    )?;
    let unit = text(resource, "unit")?;
    require(
        unit.strip_prefix("fabric-work-")
            .is_some_and(|tail| !tail.is_empty() && hex(tail, tail.len())),
        "resource unit",
    )?;
    let child = resource
        .get("command_receipt")
        .ok_or("child receipt missing")?;
    require(
        child.is_object()
            && child.get("state").and_then(Value::as_str) == Some("completed")
            && child.get("command") == Some(command)
            && text(child, "unit")? == unit,
        "child receipt mismatch",
    )?;
    exit(child.get("exit"), expected)?;
    let cgroup = text(child, "cgroup")?;
    require(cgroup.starts_with("/sys/fs/cgroup/"), "child cgroup root")?;
    let parts = files::parts(&cgroup[1..])?;
    let service = format!("{unit}.service");
    let direct = parts.last() == Some(&service.as_str());
    let delegated = parts.len() >= 5
        && parts[parts.len() - 2] == service
        && parts.last() == Some(&"workload")
        && resource.get("delegated") == Some(&Value::Bool(true));
    require(direct || delegated, "cgroup unit mismatch")?;
    if let Some(child_revision) = child.get("revision") {
        require(
            child_revision.as_str() == Some(revision),
            "child revision mismatch",
        )?;
    }
    Ok(unit)
}

/// Validate a caller-pinned plan against bytes under a held no-follow root.
/// `matched` only means structural consistency; it grants no execution claim.
pub fn verify_plan(
    root: &Path,
    plan_path: &str,
    plan_sha256: &str,
    candidate: &str,
) -> Result<Value, String> {
    require(hex(candidate, 40), "candidate revision")?;
    let root_text = root.to_str().ok_or("UTF-8 root required")?;
    let mut files = ArtifactStore::new(root)?;
    let plan = files.plan(plan_path, plan_sha256)?;
    keys(
        &plan,
        &["version", "candidate", "artifacts", "jobs"],
        "plan",
    )?;
    require(plan["version"].as_u64() == Some(1), "plan version")?;
    require(
        text(&plan, "candidate")? == candidate,
        "candidate disagreement",
    )?;
    let artifacts = plan["artifacts"].as_array().ok_or("artifact list")?;
    require(artifacts.len() <= graph::ITEM_LIMIT, "artifact count")?;
    let jobs = plan["jobs"].as_array().ok_or("job list")?;
    let order = ordered_jobs(jobs)?;
    let mut indexed = BTreeMap::new();
    let mut paths = HashSet::new();
    for artifact in artifacts {
        keys(artifact, &["id", "path", "sha256"], "artifact")?;
        let name = text(artifact, "id")?;
        require(id(name), "artifact id")?;
        let path = text(artifact, "path")?;
        files::parts(path)?;
        require(paths.insert(path), "duplicate artifact path")?;
        let sha256 = text(artifact, "sha256")?;
        require(hex(sha256, 64), "artifact digest")?;
        require(
            indexed.insert(name, Artifact { path, sha256 }).is_none(),
            "duplicate artifact id",
        )?;
    }
    let mut by_job = HashMap::new();
    let mut receipts = HashSet::new();
    for job in jobs {
        keys(
            job,
            &[
                "id",
                "needs",
                "command",
                "harness_revision",
                "expected_exit",
                "evidence",
            ],
            "job",
        )?;
        command(&job["command"])?;
        require(hex(text(job, "harness_revision")?, 40), "harness revision")?;
        require(
            job["expected_exit"]
                .as_u64()
                .is_some_and(|exit| exit <= 255),
            "expected exit",
        )?;
        let evidence = &job["evidence"];
        if !evidence.is_null() {
            keys(evidence, &["wrapper", "resource"], "evidence")?;
            for role in ["wrapper", "resource"] {
                let name = text(evidence, role)?;
                require(indexed.contains_key(name), "unknown receipt artifact")?;
                require(receipts.insert(name), "shared receipt")?;
            }
        }
        by_job.insert(text(job, "id")?, job);
    }
    for (name, artifact) in &indexed {
        files.read(artifact.path, artifact.sha256, receipts.contains(name))?;
    }
    let mut states = HashMap::new();
    let mut units = HashSet::new();
    let mut output = Vec::with_capacity(jobs.len());
    for name in &order {
        let job = by_job[name.as_str()];
        let ready = job["needs"]
            .as_array()
            .ok_or("needs list")?
            .iter()
            .all(|parent| {
                parent
                    .as_str()
                    .is_some_and(|parent| states.get(parent) == Some(&"matched"))
            });
        let evidence = &job["evidence"];
        let state = if evidence.is_null() {
            if ready { "pending" } else { "blocked" }
        } else {
            require(ready, "evidence with unmet dependency")?;
            let wrapper = &indexed[text(evidence, "wrapper")?];
            let resource = &indexed[text(evidence, "resource")?];
            let unit = join(
                job,
                files.json(wrapper.path)?,
                files.json(resource.path)?,
                resource,
                root_text,
            )?;
            require(units.insert(unit), "resource unit shared between jobs")?;
            "matched"
        };
        states.insert(name.as_str(), state);
        output.push(json!({"id": name, "state": state}));
    }
    files.recheck()?;
    let status = if states.values().all(|state| *state == "matched") {
        "matched"
    } else {
        "pending"
    };
    Ok(json!({"status": status, "jobs": output}))
}

/// Check the actual native cgroup, independent of claimed receipt fields.
pub fn require_limits() -> Result<(), String> {
    let membership = io(std::fs::read_to_string("/proc/self/cgroup"))?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("unified cgroup v2 required")?;
    require(relative.starts_with('/'), "absolute cgroup membership")?;
    let cgroup = if relative == "/" {
        std::path::PathBuf::from("/sys/fs/cgroup")
    } else {
        files::parts(&relative[1..])?;
        Path::new("/sys/fs/cgroup").join(&relative[1..])
    };
    let memory = io(std::fs::read_to_string(cgroup.join("memory.max")))?;
    let swap = io(std::fs::read_to_string(cgroup.join("memory.swap.max")))?;
    let memory: u64 = memory
        .trim()
        .parse()
        .map_err(|_| "finite memory.max required")?;
    require(
        memory > 0 && memory <= 20 * 1024 * 1024 * 1024 && swap.trim() == "0",
        "unsafe actual memory/swap limits",
    )
}

/// JSON-only CLI boundary, including malformed arguments. No jobs are launched.
pub fn cli(arguments: Vec<String>) -> u8 {
    let result = (|| {
        let mut flags = HashMap::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            require(
                matches!(
                    flag.as_str(),
                    "--root" | "--plan" | "--plan-sha256" | "--candidate"
                ),
                "unknown argument",
            )?;
            let value = arguments.next().ok_or("missing argument value")?;
            require(flags.insert(flag, value).is_none(), "duplicate argument")?;
        }
        require(flags.len() == 4, "required root/plan/plan-sha256/candidate")?;
        require_limits()?;
        let storage = std::env::var("FABRIC_STORAGE_ROOT")
            .map_err(|_| "explicit FABRIC_STORAGE_ROOT required")?;
        require(
            flags["--root"] == storage,
            "root differs from launcher storage",
        )?;
        verify_plan(
            Path::new(&flags["--root"]),
            &flags["--plan"],
            &flags["--plan-sha256"],
            &flags["--candidate"],
        )
    })();
    match result {
        Ok(output) => {
            let code = if output["status"] == "matched" { 0 } else { 3 };
            println!("{output}");
            code
        }
        Err(error) => {
            println!("{}", json!({"status": "invalid", "error": error}));
            2
        }
    }
}
