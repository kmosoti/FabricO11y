use std::{
    env,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let socket = PathBuf::from(args.next().ok_or("usage: fabric-adapter SOCKET PRODUCER")?);
    let producer = args.next().ok_or("producer required")?;
    let config = fabric::sdk::Config::local(socket, producer)?;
    let context = match args.next().as_deref() {
        None => None,
        Some("--run-context") => Some(
            serde_json::from_str::<fabric::run::RunContext>(
                &args.next().ok_or("--run-context requires JSON")?,
            )
            .map_err(|e| e.to_string())?,
        ),
        Some(_) => return Err("usage: fabric-adapter SOCKET PRODUCER [--run-context JSON]".into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    enum Adapter {
        Legacy(fabric::sdk::Client),
        Run(Box<fabric::sdk::RunClient>),
    }
    let mut client = match context {
        Some(context) => Adapter::Run(Box::new(fabric::sdk::RunClient::new(config, context)?)),
        None => Adapter::Legacy(fabric::sdk::Client::new(config)?),
    };
    let input = io::stdin();
    let mut input = input.lock();
    let output = io::stdout();
    let mut output = output.lock();
    let mut line = Vec::with_capacity(65537);
    loop {
        line.clear();
        let mut oversized = false;
        loop {
            let bytes = input.fill_buf().map_err(|e| e.to_string())?;
            if bytes.is_empty() {
                break;
            }
            let end = bytes
                .iter()
                .position(|&b| b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(bytes.len());
            if line.len() + end <= 65537 {
                line.extend_from_slice(&bytes[..end])
            } else {
                oversized = true
            }
            let newline = bytes[end - 1] == b'\n';
            input.consume(end);
            if newline || oversized {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        let reply = if oversized {
            serde_json::json!({"error":"JSONL line exceeds limit"})
        } else {
            match &mut client {
                Adapter::Legacy(client) => fabric::sdk::adapter_reply(client, &line),
                Adapter::Run(client) => fabric::sdk::run_adapter_reply(client, &line),
            }
        };
        serde_json::to_writer(&mut output, &reply).map_err(|e| e.to_string())?;
        writeln!(output).map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
        if oversized {
            break;
        }
    }
    // EOF attempts a bounded flush. Failure is visible in a final structured reply.
    let deadline = Instant::now() + Duration::from_secs(2);
    match match &client {
        Adapter::Legacy(client) => client.flush(deadline),
        Adapter::Run(client) => client.flush_local(deadline),
    } {
        Ok(receipt) => writeln!(output, "{}", serde_json::json!({"eof_receipt":receipt})),
        Err(error) => writeln!(output, "{}", serde_json::json!({"error":error,"at":"eof"})),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("fabric-adapter: {error}");
        std::process::exit(1)
    }
}
