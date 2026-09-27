use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use transport_sim::{Config, Run, Trace, Variant, simulate};

const SUMMARY_HEADER: &str = "profile,seed,variant,producers,bursts,message_bytes,packet_size,data_delay_ticks,control_delay_ticks,bdp_packets,sender_queue_cap_bytes,switch_queue_cap_bytes,receiver_queue_cap_bytes,injection_interval_ticks,drain_deadline_ticks,nominal_burst_spacing_ticks,jitter_magnitude_ticks,control_message_bytes,switch_egress_packets_per_tick,sender_packets_per_tick,offered_messages,offered_bytes,completed_injection_messages,completed_injection_bytes,completed_deadline_messages,ack_count,rejection_count,loss_count,control_messages,control_bytes,data_packets_sent,data_packets_delivered,modeled_commits,sender_peak_bytes,max_sender_queue_per_producer_bytes,sender_retained_peak_bytes,max_sender_retained_per_producer_bytes,sender_byte_ticks,switch_peak_bytes,switch_byte_ticks,receiver_peak_bytes,receiver_byte_ticks,total_queue_peak_bytes,total_queue_byte_ticks,p99_burst_peak_hotspot_bytes,max_granted_outstanding_packets,max_sender_window_outstanding_packets,cap_overflow_count,unacked_at_deadline,unscheduled_prefix_packets,scheduled_packets_sent,unscheduled_packets_sent,embedded_metadata_bytes,data_wire_bytes_sent";
const MESSAGES_HEADER: &str = "profile,seed,variant,producer,sequence,burst,bytes,release_tick,first_send_tick,complete_tick,modeled_commit_tick,ack_tick";
const BURSTS_HEADER: &str =
    "profile,seed,variant,burst,release_tick,last_complete_tick,peak_hotspot_bytes";

fn write_summary(writer: &mut impl Write, profile: &str, run: &Run) -> Result<(), String> {
    let c = &run.config;
    let s = &run.summary;
    let offered_messages = run.trace.messages.len();
    let offered_bytes = offered_messages as u64 * c.message_bytes as u64;
    let fields = [
        profile.into(),
        run.seed.to_string(),
        run.variant.as_str().into(),
        c.producers.to_string(),
        c.bursts.to_string(),
        c.message_bytes.to_string(),
        c.packet_size.to_string(),
        c.data_delay_ticks.to_string(),
        c.control_delay_ticks.to_string(),
        c.bdp_packets.to_string(),
        c.sender_queue_cap_bytes.to_string(),
        c.switch_queue_cap_bytes.to_string(),
        "0".into(), // network-limited receiver queue
        c.injection_interval_ticks.to_string(),
        c.drain_deadline_ticks.to_string(),
        c.nominal_burst_spacing_ticks.to_string(),
        c.jitter_magnitude_ticks.to_string(),
        "32".into(), // bytes per control message
        "1".into(),  // switch egress packets per tick
        "1".into(),  // sender packets per tick
        offered_messages.to_string(),
        offered_bytes.to_string(),
        s.completed_injection_messages.to_string(),
        s.completed_injection_bytes.to_string(),
        s.completed_deadline_messages.to_string(),
        s.ack_count.to_string(),
        s.rejection_count.to_string(),
        s.loss_count.to_string(),
        s.control_messages.to_string(),
        s.control_bytes.to_string(),
        s.data_packets_sent.to_string(),
        s.data_packets_delivered.to_string(),
        s.modeled_commits.to_string(),
        s.sender_peak_bytes.to_string(),
        s.max_sender_queue_per_producer_bytes.to_string(),
        s.sender_retained_peak_bytes.to_string(),
        s.max_sender_retained_per_producer_bytes.to_string(),
        s.sender_byte_ticks.to_string(),
        s.switch_peak_bytes.to_string(),
        s.switch_byte_ticks.to_string(),
        s.receiver_peak_bytes.to_string(),
        s.receiver_byte_ticks.to_string(),
        s.total_queue_peak_bytes.to_string(),
        s.total_queue_byte_ticks.to_string(),
        s.p99_burst_peak_hotspot_bytes.to_string(),
        s.max_granted_outstanding_packets.to_string(),
        s.max_sender_window_outstanding_packets.to_string(),
        s.cap_overflow_count.to_string(),
        s.unacked_at_deadline.to_string(),
        if run.variant == Variant::M2 {
            c.unscheduled_prefix_packets.to_string()
        } else {
            "0".into()
        },
        s.scheduled_packets_sent.to_string(),
        s.unscheduled_packets_sent.to_string(),
        s.embedded_metadata_bytes.to_string(),
        s.data_wire_bytes_sent.to_string(),
    ];
    writeln!(writer, "{}", fields.join(",")).map_err(|error| error.to_string())
}

fn write_messages(writer: &mut impl Write, profile: &str, run: &Run) -> Result<(), String> {
    // Group by producer then sequence, keeping each profile/seed/variant group stable.
    for producer in 0..run.config.producers {
        for burst in 0..run.config.bursts {
            let index = burst * run.config.producers + producer;
            let spec = &run.trace.messages[index];
            let result = &run.messages[index];
            let first = result.first_send_tick.ok_or("missing first send tick")?;
            let complete = result.complete_tick.ok_or("missing complete tick")?;
            let commit = result
                .modeled_commit_tick
                .ok_or("missing modeled commit tick")?;
            let ack = result.ack_tick.ok_or("missing ACK tick")?;
            writeln!(
                writer,
                "{},{},{},{},{},{},{},{},{},{},{},{}",
                profile,
                run.seed,
                run.variant.as_str(),
                spec.producer,
                spec.sequence,
                spec.burst,
                spec.bytes,
                spec.release_tick,
                first,
                complete,
                commit,
                ack
            )
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn write_bursts(writer: &mut impl Write, profile: &str, run: &Run) -> Result<(), String> {
    for (burst, result) in run.bursts.iter().enumerate() {
        let complete = result
            .last_complete_tick
            .ok_or("missing burst completion tick")?;
        writeln!(
            writer,
            "{},{},{},{},{},{},{}",
            profile,
            run.seed,
            run.variant.as_str(),
            burst,
            run.trace.releases[burst],
            complete,
            result.peak_hotspot_bytes
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn run_m2(output: &Path) -> Result<(), String> {
    if output.exists() {
        return Err(format!(
            "output directory already exists: {}",
            output.display()
        ));
    }
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create parent {}: {error}", parent.display()))?;
    }
    fs::create_dir(output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let mut summary =
        BufWriter::new(File::create(output.join("summary.csv")).map_err(|e| e.to_string())?);
    let mut messages =
        BufWriter::new(File::create(output.join("messages.csv")).map_err(|e| e.to_string())?);
    let mut bursts =
        BufWriter::new(File::create(output.join("bursts.csv")).map_err(|e| e.to_string())?);
    writeln!(summary, "{SUMMARY_HEADER}").map_err(|error| error.to_string())?;
    writeln!(messages, "{MESSAGES_HEADER}").map_err(|error| error.to_string())?;
    writeln!(bursts, "{BURSTS_HEADER}").map_err(|error| error.to_string())?;
    for (profile, config) in [
        ("small", Config::m2_small()),
        ("incast", Config::m2_incast()),
    ] {
        for seed in 0..10 {
            let trace = Trace::generate(&config, seed)?;
            for variant in [Variant::M1, Variant::M2] {
                let run = simulate(&config, &trace, seed, variant).map_err(|error| {
                    format!(
                        "profile {profile}, seed {seed} {}: {error}",
                        variant.as_str()
                    )
                })?;
                write_summary(&mut summary, profile, &run)?;
                write_messages(&mut messages, profile, &run)?;
                write_bursts(&mut bursts, profile, &run)?;
            }
        }
    }
    summary.flush().map_err(|error| error.to_string())?;
    messages.flush().map_err(|error| error.to_string())?;
    bursts.flush().map_err(|error| error.to_string())?;
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 || args[1] != "m2" {
        eprintln!("usage: prefix_probe m2 <fresh-output-dir>");
        std::process::exit(2);
    }
    if let Err(error) = run_m2(Path::new(&args[2])) {
        eprintln!("prefix_probe: {error}");
        std::process::exit(1);
    }
}
