//! Structural cutoff checks on caller-normalized observations. Raw producer
//! authenticity and the independent custody oracle remain separate obligations.
/// Pure cutoff/coverage primitives; finite controls do not attest their inputs.
pub mod cutoff;
mod inspection;
#[cfg(test)]
mod kernel_controls;

use crate::preflight::ArtifactStore;
use cutoff::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

fn require(condition: bool, error: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(error.to_owned())
    }
}

fn keys(value: &Value, expected: &[&str]) -> Result<(), String> {
    let object = value.as_object().ok_or("object required")?;
    require(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "unexpected or missing fields",
    )
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("{key}: string required"))
}

fn number(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("{key}: unsigned integer required"))
}

fn boolean(value: &Value, key: &str) -> Result<bool, String> {
    value[key]
        .as_bool()
        .ok_or_else(|| format!("{key}: boolean required"))
}

fn array<'a>(value: &'a Value, key: &str, limit: usize) -> Result<&'a [Value], String> {
    let rows = value[key]
        .as_array()
        .ok_or_else(|| format!("{key}: array required"))?;
    require(rows.len() <= limit, "observation count exceeds bound")?;
    Ok(rows)
}

fn hex<const N: usize>(value: &str) -> Result<[u8; N], String> {
    require(
        value.len() == 2 * N
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "canonical hex required",
    )?;
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&value[2 * index..2 * index + 2], 16).map_err(|_| "invalid hex")?;
    }
    Ok(bytes)
}

fn identity(value: &Value) -> Result<SenderIncarnation, String> {
    keys(value, &["boot", "pid", "start_ticks", "invocation"])?;
    let pid = u32::try_from(number(value, "pid")?).map_err(|_| "PID overflow")?;
    let start_ticks = number(value, "start_ticks")?;
    let invocation = u128::from_be_bytes(hex(text(value, "invocation")?)?);
    require(
        pid > 0 && start_ticks > 0 && invocation > 0,
        "invalid incarnation",
    )?;
    Ok(SenderIncarnation {
        boot: u128::from_be_bytes(hex(text(value, "boot")?)?),
        pid,
        start_ticks,
        invocation,
    })
}

fn command_interval(value: &Value, previous_finish: &mut u64) -> Result<u64, String> {
    let start = number(value, "started_ns")?;
    let finish = number(value, "finished_ns")?;
    require(
        start >= *previous_finish && finish >= start,
        "overlapping or reversed command intervals",
    )?;
    *previous_finish = finish;
    Ok(start)
}

/// Read exactly the pinned inspection bytes, not a reopened path or copied counters.
fn trace(plan: &Value, files: &mut ArtifactStore) -> Result<Vec<PrefixEvent>, String> {
    let mut output = Vec::new();
    let mut previous_finish = 0;
    let node = text(plan, "node_id")?;
    let generation = number(plan, "generation")?;
    for value in array(plan, "trace", 4096)? {
        let kind = text(value, "kind")?;
        let event = match kind {
            "batch" => {
                keys(value, &["kind", "sender", "at_ns", "sequence"])?;
                PrefixEvent::BatchCommit {
                    sequence: number(value, "sequence")?,
                    sender: identity(&value["sender"])?,
                    at_ns: number(value, "at_ns")?,
                }
            }
            "pause-request" => {
                keys(
                    value,
                    &["kind", "sender", "started_ns", "finished_ns", "revision"],
                )?;
                PrefixEvent::PauseRequest {
                    revision: number(value, "revision")?,
                    sender: identity(&value["sender"])?,
                    at_ns: command_interval(value, &mut previous_finish)?,
                }
            }
            "pause-applied" => {
                keys(value, &["kind", "sender", "at_ns", "revision"])?;
                PrefixEvent::PauseApplied {
                    revision: number(value, "revision")?,
                    sender: identity(&value["sender"])?,
                    at_ns: number(value, "at_ns")?,
                }
            }
            "inspection" => {
                keys(
                    value,
                    &[
                        "kind",
                        "sender",
                        "started_ns",
                        "finished_ns",
                        "revision",
                        "paused",
                        "stopped",
                        "input",
                    ],
                )?;
                keys(&value["input"], &["path", "sha256"])?;
                let at_ns = command_interval(value, &mut previous_finish)?;
                let bytes = files.read_bytes(
                    text(&value["input"], "path")?,
                    text(&value["input"], "sha256")?,
                )?;
                let raw = std::str::from_utf8(bytes).map_err(|_| "inspection UTF-8 required")?;
                let parsed = inspection::parse_inspection(raw)
                    .map_err(|error| format!("inspection: {error:?}"))?;
                require(
                    parsed.node_id == node && parsed.generation == generation,
                    "inspection node/generation mismatch",
                )?;
                PrefixEvent::Inspection(PrefixInspection {
                    sender: identity(&value["sender"])?,
                    at_ns,
                    applied_revision: number(value, "revision")?,
                    paused: boolean(value, "paused")?,
                    stopped: boolean(value, "stopped")?,
                    next_sequence: parsed.next_sequence,
                    acked_through: parsed.acked_through,
                    recovery_required: parsed.recovery_required,
                    interrupted_append: parsed.interrupted_append,
                })
            }
            "stop" => {
                keys(value, &["kind", "started_ns", "finished_ns"])?;
                PrefixEvent::Stop {
                    at_ns: command_interval(value, &mut previous_finish)?,
                }
            }
            _ => return Err("unknown lifecycle event".to_owned()),
        };
        output.push(event);
    }
    Ok(output)
}

pub fn verify_plan(root: &Path, path: &str, pin: &str, candidate: &str) -> Result<Value, String> {
    hex::<20>(candidate)?;
    let mut files = ArtifactStore::new(root)?;
    let plan = files.plan(path, pin)?;
    keys(
        &plan,
        &[
            "version",
            "candidate",
            "node_id",
            "generation",
            "incarnation",
            "revision",
            "window",
            "sources",
            "events",
            "trace",
        ],
    )?;
    require(
        number(&plan, "version")? == 1 && text(&plan, "candidate")? == candidate,
        "plan version/candidate mismatch",
    )?;
    hex::<16>(text(&plan, "node_id")?)?;
    require(number(&plan, "generation")? > 0, "zero generation")?;
    let incarnation = identity(&plan["incarnation"])?;
    let window = &plan["window"];
    keys(window, &["unit", "pid", "boot", "start_ns", "end_ns"])?;
    let process = ProcessWindow {
        unit: text(window, "unit")?,
        pid: u32::try_from(number(window, "pid")?).map_err(|_| "PID overflow")?,
        boot: u128::from_be_bytes(hex(text(window, "boot")?)?),
        start_ns: number(window, "start_ns")?,
        end_ns: number(window, "end_ns")?,
    };
    require(
        process.unit == "fabrico11y-node.service" && process.start_ns <= process.end_ns,
        "invalid process window",
    )?;
    let sources = array(&plan, "sources", 65536)?
        .iter()
        .map(|value| {
            keys(value, &["sequence", "sha256"])?;
            Ok(Source {
                sequence: number(value, "sequence")?,
                digest: hex(text(value, "sha256")?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let events = array(&plan, "events", 65536)?
        .iter()
        .map(|value| {
            keys(
                value,
                &[
                    "unit", "pid", "boot", "at_ns", "sequence", "sha256", "through",
                ],
            )?;
            Ok(Event {
                unit: text(value, "unit")?,
                pid: u32::try_from(number(value, "pid")?).map_err(|_| "PID overflow")?,
                boot: u128::from_be_bytes(hex(text(value, "boot")?)?),
                at_ns: number(value, "at_ns")?,
                message: Message::Ack {
                    sequence: number(value, "sequence")?,
                    digest: hex(text(value, "sha256")?)?,
                    through: number(value, "through")?,
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let trace = trace(&plan, &mut files)?;
    let verified = verify_quiescent_prefix_composite(
        &trace,
        incarnation,
        number(&plan, "revision")?,
        &sources,
        &events,
        process,
    )
    .map_err(|error| format!("cutoff: {error:?}"))?;
    files.recheck()?;
    Ok(
        json!({"status":"structural-cutoff", "candidate":candidate, "cutoff":verified.prefix().cutoff(), "modeled_linear_visits":verified.modeled_linear_visits(), "bytes_hashed":files.bytes_hashed(), "scope":"Caller-normalized observations only; independent native binding and custody oracles remain mandatory"}),
    )
}

pub fn cli(arguments: Vec<String>) -> u8 {
    let result = (|| {
        let mut flags = BTreeMap::new();
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
        require(flags.len() == 4, "root/plan/plan-sha256/candidate required")?;
        crate::preflight::require_limits()?;
        require(
            std::env::var("FABRIC_STORAGE_ROOT").as_deref() == Ok(flags["--root"].as_str()),
            "root differs from launcher DATA",
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
            println!("{output}");
            0
        }
        Err(error) => {
            println!("{}", json!({"status":"invalid", "error":error}));
            2
        }
    }
}
