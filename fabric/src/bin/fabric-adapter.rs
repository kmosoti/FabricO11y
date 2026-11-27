use std::{
    env,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
const USAGE: &str = "Usage: fabric-adapter SOCKET PRODUCER [--run-context JSON | --batch-outbox PATH]\nOptions: -h, --help";

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let first = args.next().ok_or(USAGE)?;
    if matches!(first.as_str(), "--help" | "-h") {
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        println!("{USAGE}");
        return Ok(());
    }
    let socket = PathBuf::from(first);
    let producer = args.next().ok_or("producer required")?;
    let config = fabric::sdk::Config::local(socket, producer)?;
    enum Mode {
        Legacy,
        Run(fabric::run::RunContext),
        Batch(PathBuf),
    }
    let mode = match args.next().as_deref() {
        None => Mode::Legacy,
        Some("--run-context") => Mode::Run(
            serde_json::from_str::<fabric::run::RunContext>(
                &args.next().ok_or("--run-context requires JSON")?,
            )
            .map_err(|e| e.to_string())?,
        ),
        Some("--batch-outbox") => Mode::Batch(PathBuf::from(
            args.next().ok_or("--batch-outbox requires a directory")?,
        )),
        Some(_) => return Err(USAGE.into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    enum Adapter {
        Legacy(fabric::sdk::Client),
        Run(Box<fabric::sdk::RunClient>),
        Batch(Box<fabric::sdk::BatchClient>),
    }
    let mut client = match mode {
        Mode::Legacy => Adapter::Legacy(fabric::sdk::Client::new(config)?),
        Mode::Run(context) => Adapter::Run(Box::new(fabric::sdk::RunClient::new(config, context)?)),
        Mode::Batch(path) => Adapter::Batch(Box::new(
            fabric::sdk::BatchClient::open(config, &path).map_err(|e| e.to_string())?,
        )),
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
                Adapter::Batch(client) => fabric::sdk::batch_adapter_reply(client, &line),
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
    let reply_for_flush = |result| match result {
        Ok(receipt) => serde_json::json!({"eof_receipt":receipt}),
        Err(error) => serde_json::json!({"error":error,"at":"eof"}),
    };
    let reply = match &mut client {
        Adapter::Legacy(client) => reply_for_flush(client.flush(deadline)),
        Adapter::Run(client) => reply_for_flush(client.flush_local(deadline)),
        Adapter::Batch(client) => match client.flush_local(deadline) {
            Ok(receipt) => serde_json::json!({"eof_receipt":receipt}),
            Err(error) => {
                serde_json::json!({"error":error.to_string(),"code":error.code(),"at":"eof"})
            }
        },
    };
    writeln!(output, "{reply}").map_err(|e| e.to_string())?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("fabric-adapter: {error}");
        std::process::exit(1)
    }
}
