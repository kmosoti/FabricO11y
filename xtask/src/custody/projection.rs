//! Projects captured Guest metadata into the structural cutoff plan.
//! This selects/serializes inputs; custody::verify_plan remains the only cutoff
//! reducer. Raw journal/process binding and independent Python oracles stay outside.
use serde_json::{Value, json};

const MAX_COMMANDS: usize = 4096;
const MAX_SAMPLES: usize = 4096;
const MAX_EVENTS: usize = 65_536;
const MAX_SOURCES: usize = 65_536;
const MAX_TRACE: usize = 4096;
const INSPECTION_ENCODING: &str = "utf-8-from-subprocess-text";

pub struct Inputs<'a> {
    pub commands: &'a [Value],
    pub observed_events: &'a [Value],
    pub process_observations: &'a [Value],
    pub stopped_snapshot: &'a Value,
    pub phase: &'a str,
    pub prior_ack: u64,
    pub candidate: &'a str,
    pub relative_phase_root: &'a str,
}

use super::{array, number, require as need, text};

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    v.get(k).ok_or_else(|| format!("missing field {k}"))
}
fn lower_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn uuid_hex(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    need(
        b.len() == 36 && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-'),
        "canonical boot id required",
    )?;
    let compact: String = s.chars().filter(|&c| c != '-').collect();
    need(lower_hex(&compact, 32), "canonical boot id required")?;
    Ok(compact)
}
fn sender(proc: &Value) -> Result<(String, u64, String, u64, String), String> {
    let unit = text(proc, "unit")?.to_owned();
    need(
        unit == "fabrico11y-node.service",
        "unexpected candidate unit",
    )?;
    let pid = number(proc, "pid")?;
    need(
        pid > 0 && pid <= u32::MAX as u64,
        "candidate PID out of range",
    )?;
    let boot = uuid_hex(text(proc, "boot_id")?)?;
    let calibration = field(proc, "calibration")?;
    let start_ticks = number(calibration, "start_ticks")?;
    need(start_ticks > 0, "candidate start_ticks required")?;
    let invocation = text(proc, "invocation_id")?.to_owned();
    need(
        lower_hex(&invocation, 32) && invocation != "00000000000000000000000000000000",
        "candidate invocation id required",
    )?;
    need(
        lower_hex(text(proc, "exe_sha256")?, 64),
        "candidate executable digest required",
    )?;
    Ok((unit, pid, boot, start_ticks, invocation))
}
fn identity(pid: u64, boot: &str, ticks: u64, invocation: &str) -> Value {
    // Strict custody::identity schema: unit belongs in window/events, never here.
    json!({"boot": boot, "pid": pid, "start_ticks": ticks, "invocation": invocation})
}
fn bind_sample(sample: &Value, final_proc: &Value) -> Result<(), String> {
    let (unit, pid, boot, ticks, invocation) = sender(final_proc)?;
    let sample_sender = sender(sample)?;
    need(
        sample_sender == (unit, pid, boot, ticks, invocation),
        "process sample incarnation changed",
    )?;
    need(
        text(sample, "exe_sha256")? == text(final_proc, "exe_sha256")?,
        "process sample executable changed",
    )
}
fn bind_event(event: &Value, who: &(String, u64, String, u64, String)) -> Result<(), String> {
    need(
        text(event, "unit")? == who.0,
        "used journal unit differs from candidate",
    )?;
    need(
        number(event, "pid")? == who.1,
        "used journal PID differs from candidate",
    )?;
    need(
        uuid_hex(text(event, "boot_id")?)? == who.2,
        "used journal boot differs from candidate",
    )?;
    if event.get("invocation_id").is_some() {
        need(
            text(event, "invocation_id")? == who.4,
            "used journal invocation differs from candidate",
        )?;
    }
    Ok(())
}
fn argv_is(command: &Value, expected: &[&str]) -> bool {
    command
        .get("argv")
        .and_then(Value::as_array)
        .is_some_and(|argv| {
            argv.len() == expected.len()
                && argv
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| a.as_str() == Some(*b))
        })
}
fn successful(command: &Value) -> bool {
    command.get("exit").and_then(Value::as_i64) == Some(0)
}
fn interval(v: &Value) -> Result<(u64, u64), String> {
    let start = number(v, "started_ns")?;
    let finish = number(v, "finished_ns")?;
    need(finish >= start, "reversed observation interval")?;
    Ok((start, finish))
}
fn artifact(command: &Value, root: &str) -> Result<Value, String> {
    let a = field(command, "stdout_artifact")?;
    let obj = a.as_object().ok_or("stdout_artifact object required")?;
    need(
        obj.len() == 3
            && obj.contains_key("path")
            && obj.contains_key("sha256")
            && obj.contains_key("encoding"),
        "stdout_artifact must have exact path/sha256/encoding fields",
    )?;
    need(
        text(a, "encoding")? == INSPECTION_ENCODING,
        "unexpected stdout artifact encoding",
    )?;
    let rel = text(a, "path")?;
    need(
        !rel.starts_with('/')
            && !rel
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == ".."),
        "stdout artifact path must be canonical and relative",
    )?;
    let digest = text(a, "sha256")?;
    need(lower_hex(digest, 64), "stdout artifact digest required")?;
    Ok(json!({"path": format!("{root}/{rel}"), "sha256": digest}))
}
fn decimal(token: &str, prefix: &str) -> Result<u64, String> {
    token
        .strip_prefix(prefix)
        .ok_or("journal grammar mismatch")?
        .parse::<u64>()
        .map_err(|_| "journal integer grammar mismatch".into())
}
fn next_decimal(tokens: &mut std::str::Split<'_, char>, prefix: &str) -> Result<u64, String> {
    decimal(
        tokens
            .next()
            .ok_or("recognized journal message is truncated")?,
        prefix,
    )
}
fn parse_batch(message: &str) -> Result<u64, String> {
    let mut t = message.split(' ');
    let sequence = next_decimal(&mut t, "batch=")?;
    for prefix in [
        "metrics=",
        "logs=",
        "gaps=",
        "spool_bytes=",
        "log_backlog_bytes=",
        "acked_through=",
    ] {
        next_decimal(&mut t, prefix)?;
    }
    need(t.next().is_none(), "recognized batch has trailing tokens")?;
    Ok(sequence)
}
fn parse_delivery(message: &str) -> Result<Option<(u64, String, u64)>, String> {
    let mut t = message.split(' ');
    need(
        t.next() == Some("delivery"),
        "recognized delivery prefix mismatch",
    )?;
    let sequence = next_decimal(&mut t, "sequence=")?;
    let digest = t
        .next()
        .and_then(|v| v.strip_prefix("sha256="))
        .ok_or("ACK digest grammar mismatch")?;
    need(lower_hex(digest, 64), "ACK digest grammar mismatch")?;
    let through = match t.next() {
        Some("status=ack") => Some(next_decimal(&mut t, "committed_through=")?),
        Some("status=no_response" | "status=unavailable") => None,
        _ => return Err("ACK status grammar mismatch".to_owned()),
    };
    next_decimal(&mut t, "elapsed_us=")?;
    need(
        t.next().is_none(),
        "recognized delivery has trailing tokens",
    )?;
    Ok(through.map(|through| (sequence, digest.to_owned(), through)))
}

/// Projects caller-normalized observations to the existing custody v1 shape.
/// It performs no native attestation and leaves cutoff decisions to verify_plan.
pub fn project(input: Inputs<'_>) -> Result<Value, String> {
    need(
        input.phase == "drain" || input.phase == "reboot",
        "unsupported phase",
    )?;
    need(lower_hex(input.candidate, 40), "candidate commit required")?;
    need(
        !input.relative_phase_root.starts_with('/')
            && !input
                .relative_phase_root
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == ".."),
        "relative phase root required",
    )?;
    need(
        input.commands.len() <= MAX_COMMANDS && input.process_observations.len() <= MAX_SAMPLES,
        "command/process sample bound exceeded",
    )?;
    need(
        input.observed_events.len() <= MAX_EVENTS,
        "journal event bound exceeded",
    )?;

    let revision = if input.phase == "drain" { 2 } else { 4 };
    let snapshot = input.stopped_snapshot;
    let final_proc = field(snapshot, "candidate_process")?;
    let (unit, pid, boot, ticks, invocation) = sender(final_proc)?;
    let who = (unit.clone(), pid, boot.clone(), ticks, invocation.clone());
    let start_ns = number(final_proc, "start_ns")?;
    let end_ns = number(final_proc, "end_ns")?;
    need(start_ns <= end_ns, "invalid final process window")?;

    let node = text(snapshot, "node_id")?;
    need(lower_hex(node, 32), "snapshot node id required")?;
    let generation = number(snapshot, "generation")?;
    need(generation > 0, "snapshot generation required")?;

    let marker_text = format!("config revision={revision} paused=true interval_s=15");
    let mut marker_at = None;
    let mut previous_event_at = 0;
    let mut ack_events = Vec::new();
    for event in input.observed_events {
        let at = number(event, "at_ns")?;
        need(at >= previous_event_at, "journal stream reordered")?;
        previous_event_at = at;
        let message = text(event, "message")?;
        if message == marker_text {
            need(
                marker_at.is_none(),
                "unique applied-configuration marker required",
            )?;
            bind_event(event, &who)?;
            marker_at = Some(at);
        } else if message.starts_with("batch=") {
            parse_batch(message)?;
            bind_event(event, &who)?;
        } else if message.starts_with("delivery sequence=") {
            let ack = parse_delivery(message)?;
            bind_event(event, &who)?;
            if let Some((sequence, digest, through)) = ack {
                ack_events.push(json!({"unit":unit, "pid":pid, "boot":boot, "at_ns":at,
                    "sequence":sequence, "sha256":digest, "through":through}));
            }
        }
    }
    let marker_at = marker_at.ok_or("unique applied-configuration marker required")?;

    let pause_argv = [
        "fabricctl",
        "admin",
        "/root/accept/admin.conf",
        "node",
        "pause",
        "spindle-1",
    ];
    let resume_argv = [
        "fabricctl",
        "admin",
        "/root/accept/admin.conf",
        "node",
        "resume",
        "spindle-1",
    ];
    let stop_argv = ["systemctl", "stop", "fabrico11y-node.service"];
    let mut previous_command_finish = 0;
    let mut pause = None;
    let mut stop = None;
    let mut resume = None;
    for command in input.commands {
        let (start, finish) = interval(command)?;
        need(
            start >= previous_command_finish,
            "command stream reordered/overlapping",
        )?;
        previous_command_finish = finish;
        if argv_is(command, &pause_argv) {
            need(
                pause.is_none(),
                "unique pause/stop/resume commands required",
            )?;
            pause = Some((start, finish, command));
        }
        if argv_is(command, &stop_argv) {
            need(stop.is_none(), "unique pause/stop/resume commands required")?;
            stop = Some((start, finish, command));
        }
        if argv_is(command, &resume_argv) {
            need(
                resume.is_none(),
                "unique pause/stop/resume commands required",
            )?;
            resume = Some((start, finish, command));
        }
    }
    let (pause_start, pause_finish, pause_command) =
        pause.ok_or("unique pause/stop/resume commands required")?;
    let (stop_start, stop_finish, stop_command) =
        stop.ok_or("unique pause/stop/resume commands required")?;
    let (resume_start, _resume_finish, resume_command) =
        resume.ok_or("unique pause/stop/resume commands required")?;
    need(
        successful(pause_command) && successful(stop_command) && successful(resume_command),
        "pause/stop/resume must succeed",
    )?;
    need(
        pause_start <= marker_at && marker_at < resume_start,
        "applied marker outside phase",
    )?;

    let mut first_drain: Option<(&Value, u64, u64)> = None;
    let mut second_drain: Option<(&Value, u64, u64)> = None;
    let mut drain_count = 0usize;
    let mut stopped_inspection: Option<(&Value, u64, u64)> = None;
    let inspect_argv = ["fabricctl", "inspect", "/etc/fabrico11y/node.conf"];
    for command in input.commands {
        if !argv_is(command, &inspect_argv) || !successful(command) {
            continue;
        }
        let (start, finish) = interval(command)?;
        if start > marker_at && start >= pause_finish && finish <= stop_start {
            let row = (command, start, finish);
            if drain_count == 0 {
                first_drain = Some(row);
            } else if drain_count == 1 {
                second_drain = Some(row);
            } else {
                first_drain = second_drain;
                second_drain = Some(row);
            }
            drain_count += 1;
        }
        if start >= stop_finish && finish <= resume_start {
            stopped_inspection = Some((command, start, finish));
        }
    }
    need(
        drain_count >= 2 && stopped_inspection.is_some(),
        "two drain inspections and stopped inspection required",
    )?;
    let (first_command, first_start, first_finish) =
        first_drain.ok_or("two drain inspections required")?;
    let (second_command, second_start, second_finish) =
        second_drain.ok_or("two drain inspections required")?;
    need(
        first_finish <= second_start && second_finish <= stop_start,
        "drain inspections overlap stop or each other",
    )?;
    let (stopped_command, stopped_start, stopped_finish) = stopped_inspection.unwrap();

    let mut pre_marker_pause_sample = false;
    let mut post_marker_sample = false;
    let mut previous_sample_finish = 0;
    for sample in input.process_observations {
        bind_sample(sample, final_proc)?;
        let (start, finish) = interval(sample)?;
        need(
            start >= previous_sample_finish,
            "process observation intervals reordered/overlapping",
        )?;
        previous_sample_finish = finish;
        pre_marker_pause_sample |= finish <= pause_start;
        post_marker_sample |= start > marker_at && finish <= stop_start;
    }
    need(
        pre_marker_pause_sample,
        "process sample before pause required",
    )?;
    need(
        post_marker_sample,
        "distinct process sample after applied marker and before stop required",
    )?;

    let inc = identity(pid, &boot, ticks, &invocation);
    let inspection =
        |command: &Value, start: u64, finish: u64, is_stopped: bool| -> Result<Value, String> {
            Ok(
                json!({"kind":"inspection", "sender":inc, "started_ns":start, "finished_ns":finish,
            "revision":revision, "paused":true, "stopped":is_stopped,
            "input":artifact(command, input.relative_phase_root)?}),
            )
        };
    let mut selected: [Option<(u64, Value)>; 5] = std::array::from_fn(|_| None);
    selected[0] = Some((
        pause_start,
        json!({"kind":"pause-request", "sender":inc,
        "started_ns":pause_start, "finished_ns":pause_finish, "revision":revision}),
    ));
    selected[1] = Some((
        first_start,
        inspection(first_command, first_start, first_finish, false)?,
    ));
    selected[2] = Some((
        second_start,
        inspection(second_command, second_start, second_finish, false)?,
    ));
    selected[3] = Some((
        stop_start,
        json!({"kind":"stop", "started_ns":stop_start, "finished_ns":stop_finish}),
    ));
    selected[4] = Some((
        stopped_start,
        inspection(stopped_command, stopped_start, stopped_finish, true)?,
    ));

    let mut source_values = Vec::new();
    let mut previous_sequence = None;
    for source in array(snapshot, "sources", MAX_SOURCES)? {
        let sequence = number(source, "sequence")?;
        let digest = text(source, "sha256")?;
        need(lower_hex(digest, 64), "source SHA-256 required")?;
        need(
            previous_sequence.is_none_or(|prev| sequence > prev),
            "source sequence order changed",
        )?;
        previous_sequence = Some(sequence);
        if sequence > input.prior_ack {
            source_values.push(json!({"sequence":sequence, "sha256":digest}));
        }
    }

    // Both streams are already ordered. Keep only the fixed lifecycle selection and stream
    // relevant journal rows directly into the final trace; no interval/index/time side vectors.
    let mut selected_iter = selected.into_iter().flatten();
    let mut next_selected = selected_iter.next();
    let mut event_index = 0usize;
    let mut next_journal = next_journal_trace(
        input.observed_events,
        &mut event_index,
        &who,
        revision,
        &marker_text,
    )?;
    let mut trace = Vec::new();
    while next_selected.is_some() || next_journal.is_some() {
        if trace.len() >= MAX_TRACE {
            return Err("normalized trace bound exceeded".into());
        }
        let take_selected = match (&next_selected, &next_journal) {
            (Some((st, _)), Some((jt, _))) => {
                need(st != jt, "tied command/journal timestamp refused")?;
                st < jt
            }
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_selected {
            trace.push(next_selected.take().unwrap().1);
            next_selected = selected_iter.next();
        } else {
            trace.push(next_journal.take().unwrap().1);
            next_journal = next_journal_trace(
                input.observed_events,
                &mut event_index,
                &who,
                revision,
                &marker_text,
            )?;
        }
    }

    Ok(
        json!({"version":1, "candidate":input.candidate, "node_id":node, "generation":generation,
        "incarnation":inc, "revision":revision,
        "window":{"unit":unit, "pid":pid, "boot":boot, "start_ns":start_ns, "end_ns":end_ns},
        "sources":source_values, "events":ack_events, "trace":trace}),
    )
}

fn next_journal_trace(
    events: &[Value],
    cursor: &mut usize,
    who: &(String, u64, String, u64, String),
    revision: u64,
    marker_text: &str,
) -> Result<Option<(u64, Value)>, String> {
    let (_, pid, boot, ticks, invocation) = who;
    while *cursor < events.len() {
        let event = &events[*cursor];
        *cursor += 1;
        let message = text(event, "message")?;
        let at = number(event, "at_ns")?;
        if message.starts_with("batch=") {
            let sequence = parse_batch(message)?;
            bind_event(event, who)?;
            return Ok(Some((
                at,
                json!({"kind":"batch", "sender":identity(*pid, boot, *ticks, invocation), "at_ns":at, "sequence":sequence}),
            )));
        }
        if message == marker_text {
            bind_event(event, who)?;
            return Ok(Some((
                at,
                json!({"kind":"pause-applied", "sender":identity(*pid, boot, *ticks, invocation), "at_ns":at, "revision":revision}),
            )));
        }
        if message.starts_with("delivery sequence=") {
            // The ACK is represented in `events`; it is not duplicated in lifecycle trace.
            continue;
        }
    }
    Ok(None)
}

/// Admit the captured-metadata composition without another reader or dispatcher.
pub(super) fn normalize(input: Value, candidate: &str) -> Result<Value, String> {
    if input["version"].as_u64() != Some(2) {
        return Ok(input);
    }
    super::keys(
        &input,
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
    need(
        text(&input, "candidate")? == candidate,
        "plan version/candidate mismatch",
    )?;
    super::keys(
        &input["stopped_snapshot"],
        &["node_id", "generation", "sources", "candidate_process"],
    )?;
    project(Inputs {
        commands: array(&input, "commands", MAX_COMMANDS)?,
        observed_events: array(&input, "observed_events", MAX_EVENTS)?,
        process_observations: array(&input, "process_observations", MAX_SAMPLES)?,
        stopped_snapshot: &input["stopped_snapshot"],
        phase: text(&input, "phase")?,
        prior_ack: number(&input, "prior_ack")?,
        candidate,
        relative_phase_root: text(&input, "relative_phase_root")?,
    })
}
