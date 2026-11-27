//! The CLI transports exactly the shared typed request; it does not open data files.
use fabric::{
    canonical::{Request, Response},
    query::{Client, QueryRequest},
};
use std::{
    io::{self, Read},
    time::{Duration, Instant},
};

const USAGE: &str =
    "Usage: fabric-query <query | plan | evidence | progress | capabilities | cancel> [ID]
  query, plan     Read a typed request from standard input
  evidence ID    Read a canonical observation
  progress       Read canonical source frontiers
  capabilities   Read receiver identity and protocol limits
  cancel ID      Cancel an active query

Options: -h, --help
Environment:
  FABRIC_ENDPOINT         Canonical fabric-server host:port
  FABRIC_CREDENTIAL_FILE  Private, owned reader credential file
  FABRIC_CREDENTIAL       Inline alternative to FABRIC_CREDENTIAL_FILE";

fn main() {
    if let Err(e) = run() {
        eprintln!("fabric-query: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(USAGE)?;
    if matches!(command.as_str(), "-h" | "--help") {
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        println!("{USAGE}");
        return Ok(());
    }
    let (endpoint, credential) = fabric::query::connection_from_env()?;
    let request = match command.as_str() {
        "query" | "plan" => {
            let mut bytes = String::new();
            io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_string(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("request too large".into());
            }
            let request: QueryRequest = serde_json::from_str(&bytes).map_err(|e| e.to_string())?;
            request.validate()?;
            if command == "query" {
                Request::Query { request }
            } else {
                Request::Plan { request }
            }
        }
        "evidence" => Request::Evidence {
            canonical_id: args
                .next()
                .ok_or("canonical ID required")?
                .parse()
                .map_err(|_| "invalid canonical ID")?,
        },
        "progress" => Request::Progress,
        "capabilities" => Request::Capabilities,
        "cancel" => Request::Cancel {
            query_id: args.next().ok_or("query ID required")?,
        },
        _ => return Err(USAGE.into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let response = Client::new(endpoint, credential)
        .exchange(&request, Instant::now() + Duration::from_secs(35))?;
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|e| e.to_string())?
    );
    if let Response::Error { error } = response {
        return Err(error);
    }
    Ok(())
}
