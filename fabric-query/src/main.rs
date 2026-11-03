//! The CLI transports exactly the shared typed request; it does not open data files.
use fabric::{
    canonical::{Request, Response},
    query::{Client, QueryRequest},
};
use std::{
    io::{self, Read},
    time::{Duration, Instant},
};
fn main() {
    if let Err(e) = run() {
        eprintln!("fabric-query: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command=args.next().ok_or("usage: fabric-query query|plan|evidence|progress|cancel [canonical-id|query-id]; endpoint and credential from FABRIC_ENDPOINT and FABRIC_CREDENTIAL")?;
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
        "cancel" => Request::Cancel {
            query_id: args.next().ok_or("query ID required")?,
        },
        _ => return Err("unknown command".into()),
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
