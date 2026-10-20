//! Grade the two registered hosted-browser defect receipts.
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const CLEANUPS: [&str; 2] = [
    "owned fixtures removed; process groups stopped and bound children reaped",
    "failure evidence retained in owned fixture storage; process groups stopped and bound children reaped",
];
const API_CHECKS: [&str; 2] = [
    "actual browser matches reviewed version",
    "two real virtual-authenticator passkeys enrolled",
];
const ENROLLMENT_CHECKS: [&str; 2] = [
    "first owner consumed protected one-time bootstrap",
    "server enrollment requires resident credentials and user verification",
];

fn exact_int(value: Option<&Value>) -> Option<i64> {
    value
        .filter(|number| number.is_i64())
        .and_then(Value::as_i64)
}

fn has_checks(receipt: &Value, expected: &[&str]) -> bool {
    receipt
        .get("checks")
        .and_then(Value::as_array)
        .is_some_and(|checks| {
            checks.iter().all(Value::is_string)
                && expected
                    .iter()
                    .all(|required| checks.iter().any(|check| check.as_str() == Some(required)))
        })
}

fn cleaned(receipt: &Value) -> bool {
    let Some(groups) = receipt.get("group_stops").and_then(Value::as_array) else {
        return false;
    };
    let Some(exits) = receipt.get("process_exits").and_then(Value::as_object) else {
        return false;
    };
    if !receipt
        .get("cleanup")
        .and_then(Value::as_str)
        .is_some_and(|cleanup| CLEANUPS.contains(&cleanup))
        || receipt
            .get("cleanup_error")
            .is_some_and(|error| !error.is_null())
        || receipt.get("own_children_empty") != Some(&Value::Bool(true))
        || groups.is_empty()
        || exits.len() != groups.len()
        || exits.values().any(|code| !code.is_i64())
    {
        return false;
    }
    let mut identities = BTreeSet::new();
    for group in groups {
        let Some(pid) = exact_int(group.get("pid")) else {
            return false;
        };
        let Some(ticks) = exact_int(group.get("start_ticks")) else {
            return false;
        };
        if pid <= 0
            || ticks <= 0
            || group.get("kernel_empty") != Some(&Value::Bool(true))
            || group.get("children_reaped") != Some(&Value::Bool(true))
            || group
                .get("stop_error")
                .is_some_and(|error| !error.is_null())
            || !identities.insert((pid, ticks))
        {
            return false;
        }
    }
    true
}

fn detected(status: i64, receipt: &Value, kind: &str) -> bool {
    if status != 1 || exact_int(receipt.get("exit")) != Some(1) || !cleaned(receipt) {
        return false;
    }
    match kind {
        "api-cache" => {
            receipt.get("injected_api_cache_defect") == Some(&Value::Bool(true))
                && receipt.get("error").and_then(Value::as_str)
                    == Some("worker caches only public shell paths")
                && has_checks(receipt, &API_CHECKS)
        }
        "nonresident-enrollment" => {
            let witnesses = receipt
                .get("resident_credential_witnesses")
                .and_then(Value::as_array);
            receipt.get("injected_nonresident_enrollment") == Some(&Value::Bool(true))
                && receipt.get("error").and_then(Value::as_str)
                    == Some(
                        "owner enrollment stores actual resident credentials for the fixture RP",
                    )
                && has_checks(receipt, &ENROLLMENT_CHECKS)
                && witnesses.is_some_and(|values| {
                    values.len() == 1
                        && exact_int(values[0].get("count")) == Some(1)
                        && values[0].get("credentials")
                            == Some(&serde_json::json!([{"resident": false, "rp": "localhost"}]))
                })
        }
        _ => false,
    }
}

fn receipt_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().ok_or("receipt filename required")?;
    Ok(parent
        .canonicalize()
        .map_err(|error| error.to_string())?
        .join(name))
}

/// `status` and `path` are positional; the optional kind selects one exact defect.
pub fn cli(arguments: Vec<String>) -> ExitCode {
    if let Err(error) = crate::preflight::require_limits() {
        eprintln!("check-browser-defect NOT RUN: {error}");
        return ExitCode::from(2);
    }
    let result = (|| -> Result<(), String> {
        let mut args = arguments.into_iter();
        let raw_status = args.next().ok_or(
            "usage: check-browser-defect STATUS PATH [--kind api-cache|nonresident-enrollment]",
        )?;
        let status = raw_status
            .parse::<i64>()
            .map_err(|_| "status must be an integer")?;
        let raw_path = args.next().ok_or("receipt path required")?;
        let mut kind = "api-cache".to_owned();
        if let Some(flag) = args.next() {
            if flag != "--kind" {
                return Err("expected --kind".into());
            }
            kind = args.next().ok_or("kind required")?;
            if !matches!(kind.as_str(), "api-cache" | "nonresident-enrollment") {
                return Err("unknown defect kind".into());
            }
        }
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        let path = receipt_path(Path::new(&raw_path))?;
        let root = path.parent().ok_or("receipt parent required")?;
        let relative = path
            .file_name()
            .ok_or("receipt filename required")?
            .to_str()
            .ok_or("UTF-8 receipt filename required")?;
        let mut store = crate::preflight::ArtifactStore::new(root)?;
        let receipt = store.observe_json(relative)?.clone();
        store.recheck()?;
        if !detected(status, &receipt, &kind) {
            return Err(format!(
                "{kind} negative control did not reach its registered failure"
            ));
        }
        println!("registered {kind} defect detected");
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error)
            if error.starts_with("usage:")
                || error == "status must be an integer"
                || error == "receipt path required"
                || error == "expected --kind"
                || error == "kind required"
                || error == "unknown defect kind"
                || error == "unexpected argument" =>
        {
            eprintln!("{error}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
