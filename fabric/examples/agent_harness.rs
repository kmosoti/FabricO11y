//! Run: cargo run -p fabric --example agent_harness -- /run/user/UID/spindle/spindle.sock
use fabric::{
    observation::{Data, Outcome, now_ns},
    run::RunContext,
    sdk::{Admission, Config, RunClient},
};
use std::{
    env,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn main() -> Result<(), String> {
    let socket = PathBuf::from(env::args().nth(1).ok_or("socket argument required")?);
    let context = match env::args().nth(2) {
        Some(run_id) => RunContext { run_id },
        None => RunContext::new()?,
    };
    let client = RunClient::new(Config::local(socket, "rust-agent-harness".into())?, context)?;
    println!(
        "run_context={}",
        serde_json::to_string(client.context()).map_err(|e| e.to_string())?
    );
    let started = Instant::now();
    let run_id = &client.context().run_id;
    // Generated tools use this same typed API; their result stays with the harness.
    let result = generated_tool(&client, run_id)?;
    record(
        &client,
        Data::AgentRun {
            agent: "example-agent".into(),
            run_id: run_id.to_owned(),
            duration_ns: started.elapsed().as_nanos() as u64,
            status: Outcome::Ok,
        },
    )?;
    let receipt = client.flush_local(Instant::now() + Duration::from_secs(2))?;
    println!("tool result={result}; local custody receipt={receipt:?}");
    Ok(())
}
fn generated_tool(client: &RunClient, run_id: &str) -> Result<u64, String> {
    let start = Instant::now();
    let result = 6 * 7;
    record(
        client,
        Data::ToolExecution {
            tool: "multiply".into(),
            run_id: run_id.to_owned(),
            duration_ns: start.elapsed().as_nanos() as u64,
            status: Outcome::Ok,
        },
    )?;
    Ok(result)
}
fn record(client: &RunClient, data: Data) -> Result<(), String> {
    let timestamp = now_ns();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if Instant::now() >= deadline {
            return Err("observation admission deadline".into());
        }
        match client.try_record(data.clone(), timestamp) {
            Admission::Accepted { .. } => return Ok(()),
            Admission::Contended | Admission::Full => {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("observation admission deadline")?;
                thread::sleep(remaining.min(Duration::from_millis(1)));
            }
            other => return Err(format!("observation refused: {other:?}")),
        }
    }
}
