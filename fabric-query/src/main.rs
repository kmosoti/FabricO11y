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
    "Usage: fabric-query <query | query-versioned VERSION | plan | evidence ID | progress | capabilities | cancel ID>
  query, plan              Read a typed request from standard input
  query-versioned VERSION  Read a typed request with an operation version
  evidence ID              Read a canonical observation
  progress                 Read canonical source frontiers
  capabilities             Read receiver identity and protocol limits
  cancel ID                Cancel an active query

Options: -h, --help
Environment:
  FABRIC_ENDPOINT         Canonical fabric-server host:port
  FABRIC_CREDENTIAL_FILE  Private, owned reader credential file
  FABRIC_CREDENTIAL       Inline alternative to FABRIC_CREDENTIAL_FILE";

fn main() {
    if let Err(e) = run() {
        eprintln!("fabric-query: {}", fabric_core::sanitize_terminal_text(&e));
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
        "query" | "query-versioned" | "plan" => {
            let operation_version = if command == "query-versioned" {
                Some(
                    args.next()
                        .ok_or("operation version required")?
                        .parse::<u32>()
                        .map_err(|_| "invalid operation version")?,
                )
            } else {
                None
            };
            let mut bytes = String::new();
            io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_string(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("request too large".into());
            }
            query_command(&command, operation_version, &bytes)?
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

fn query_command(
    command: &str,
    operation_version: Option<u32>,
    bytes: &str,
) -> Result<Request, String> {
    let request: QueryRequest = serde_json::from_str(bytes).map_err(|e| e.to_string())?;
    request.validate()?;
    match (command, operation_version) {
        ("query-versioned", Some(operation_version)) => Ok(Request::QueryWithOperationVersion {
            request,
            operation_version,
        }),
        ("query", None) => Ok(Request::Query { request }),
        ("plan", None) => Ok(Request::Plan { request }),
        _ => Err("invalid query command metadata".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versioned_command_preserves_existing_query_body() {
        let input = r#"{"version":1,"query_id":"q","scope":["node"],"operation":{"operation":"list_runs"},"cursor":null}"#;
        let Request::Query { request: legacy } = query_command("query", None, input).unwrap()
        else {
            panic!("legacy")
        };
        let Request::QueryWithOperationVersion {
            request,
            operation_version,
        } = query_command("query-versioned", Some(2), input).unwrap()
        else {
            panic!("versioned")
        };
        assert_eq!(legacy, request);
        assert_eq!(operation_version, 2);
        assert!(query_command("query-versioned", None, input).is_err());
    }
}
