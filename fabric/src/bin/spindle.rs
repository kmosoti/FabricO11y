use fabric::spindle::{Config, Daemon, Reply, Request, transport};
use std::{
    env,
    fs::File,
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
fn request(socket: &str, request: &Request) -> Result<Reply, String> {
    transport::request(
        Path::new(socket),
        request,
        Instant::now() + Duration::from_secs(2),
    )
}
fn print_reply(reply: Reply) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&reply).map_err(|e| e.to_string())?
    );
    Ok(())
}
fn export(socket: &str) -> Result<(), String> {
    let boundary = match request(socket, &Request::Status)? {
        Reply::Status { status } => status.highest_record_id,
        Reply::Error { error } => return Err(error),
        _ => return Err("unexpected status".into()),
    };
    let mut after = 0;
    while after < boundary {
        let records = transport::read_raw(
            Path::new(socket),
            after,
            Instant::now() + Duration::from_secs(2),
        )?;
        if records.is_empty() {
            return Err("export frontier unavailable".into());
        }
        for (id, raw) in records {
            if id > boundary {
                return Ok(());
            }
            if id <= after {
                return Err("export record order violation".into());
            }
            after = id;
            std::io::stdout()
                .write_all(&raw)
                .map_err(|e| e.to_string())?;
            std::io::stdout()
                .write_all(b"\n")
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice(){
        ["serve",path]=>{let mut bytes=Vec::new();File::open(path).map_err(|e|e.to_string())?.take(65537).read_to_end(&mut bytes).map_err(|e|e.to_string())?;fabric::observation::check_json(&bytes,65536)?;let config:Config=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;let mut daemon=Daemon::open(config)?;fabric::spindle::notify_systemd_ready();daemon.run()},
        ["status",socket]=>print_reply(request(socket,&Request::Status)?),
        ["stop",socket]=>print_reply(request(socket,&Request::Stop)?),
        ["read",socket]=>print_reply(request(socket,&Request::Read{after:0,limit:1})?),
        ["read",socket,after]=>print_reply(request(socket,&Request::Read{after:after.parse().map_err(|_|"invalid after")?,limit:1})?),
        ["export",socket]=>export(socket),
        _=>Err("usage: spindle serve CONFIG | status SOCKET | read SOCKET [AFTER] | export SOCKET | stop SOCKET".into())
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("spindle: {error}");
        std::process::exit(1)
    }
}
