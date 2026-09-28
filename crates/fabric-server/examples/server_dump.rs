//! Print every batch a fresh process recovers from a server state directory,
//! one JSON object per line, for the delivery oracle adapter. Diagnostic only.
use fabric_server::config::Config;
use fabric_server::store::{Store, identify};
use std::process::ExitCode;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, records) = match args.as_slice() {
        [path] => (path.clone(), false),
        [path, flag] if flag == "--records" => (path.clone(), true),
        _ => {
            eprintln!("usage: server_dump <SERVER_CONFIG> [--records]");
            return ExitCode::from(2);
        }
    };
    let result = Config::load(&path).and_then(|config| {
        Store::replay(&config.state_dir, config.journal_bytes, |entry| {
            if records {
                // The query oracle's record format.
                println!(
                    "{{\"label\":\"{}\",\"received_ns\":{},\"bytes\":\"{}\"}}",
                    entry.label,
                    entry.received_unix_nano,
                    b64(&entry.batch)
                );
                return Ok(());
            }
            let ((node, generation), sequence) = identify(&entry.batch)?;
            println!(
                "{{\"type\":\"recovered\",\"node_id\":\"{}\",\"generation\":{generation},\"sequence\":{sequence},\"bytes\":\"{}\"}}",
                hex(&node),
                b64(&entry.batch)
            );
            Ok(())
        })
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("server_dump: {error}");
            ExitCode::FAILURE
        }
    }
}
