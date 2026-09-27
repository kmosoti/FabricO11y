//! Print a node spool as delivery-oracle `source` records plus one
//! `node_state` record, for the phase-2 fault harness. Diagnostic only.
use fabric_o11y::alpha::journal::Journal;
use fabric_o11y::alpha::node::Config;
use prost::Message;
use std::process::ExitCode;

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: alpha_spool_dump <NODE_CONFIG>");
        return ExitCode::from(2);
    };
    let result = Config::load(&path).and_then(|config| {
        let mut retained = Vec::new();
        let mut node_hex = String::new();
        let status = Journal::inspect(&config.spool, config.spool_bytes, |batch| {
            node_hex = batch.node_id.iter().map(|b| format!("{b:02x}")).collect();
            println!(
                "{{\"type\":\"source\",\"node_id\":\"{node_hex}\",\"generation\":{},\"sequence\":{},\"bytes\":\"{}\"}}",
                batch.generation,
                batch.sequence,
                b64(&batch.encode_to_vec())
            );
            retained.push(batch.sequence.to_string());
            Ok(())
        })?;
        if status.recovery_required || status.interrupted_append {
            eprintln!(
                "alpha_spool_dump: recovery_required={} interrupted_append={}",
                status.recovery_required, status.interrupted_append
            );
        }
        let node_hex: String = status.node_id.iter().map(|b| format!("{b:02x}")).collect();
        println!(
            "{{\"type\":\"node_state\",\"node_id\":\"{node_hex}\",\"generation\":{},\"ack_cursor\":{},\"retained_sequences\":[{}]}}",
            status.generation,
            status.acked_through,
            retained.join(",")
        );
        Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("alpha_spool_dump: {error}");
            ExitCode::FAILURE
        }
    }
}
