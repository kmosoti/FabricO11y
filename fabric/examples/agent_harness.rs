//! Run: cargo run -p fabric --example agent_harness -- /run/user/UID/spindle/spindle.sock
use fabric::{
    observation::{Data, Outcome, now_ns},
    sdk::{Admission, Client, Config},
};
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<(), String> {
    let socket = PathBuf::from(env::args().nth(1).ok_or("socket argument required")?);
    let client = Client::new(Config::local(socket, "rust-agent-harness".into())?)?;
    let started = Instant::now();
    let run_id = "example-run";
    // Generated tools use this same typed API; their result stays with the harness.
    let result = generated_tool(&client, run_id)?;
    record(
        &client,
        Data::AgentRun {
            agent: "example-agent".into(),
            run_id: run_id.into(),
            duration_ns: started.elapsed().as_nanos() as u64,
            status: Outcome::Ok,
        },
    )?;
    let receipt = client.flush(Instant::now() + Duration::from_secs(2))?;
    println!("tool result={result}; durable receipt={receipt:?}");
    Ok(())
}
fn generated_tool(client: &Client, run_id: &str) -> Result<u64, String> {
    let start = Instant::now();
    let result = 6 * 7;
    record(
        client,
        Data::ToolExecution {
            tool: "multiply".into(),
            run_id: run_id.into(),
            duration_ns: start.elapsed().as_nanos() as u64,
            status: Outcome::Ok,
        },
    )?;
    Ok(result)
}
fn record(client: &Client, data: Data) -> Result<(), String> {
    match client.try_record(data, now_ns()) {
        Admission::Accepted { .. } => Ok(()),
        other => Err(format!("observation refused: {other:?}")),
    }
}
