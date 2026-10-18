//! One bounded WebDriver BiDi viewport operation for the browser acceptance
//! adapter. The caller owns the WebDriver session and verifies actual geometry.

use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

const MAX_ARGS: usize = 4_096;
const MAX_URL: usize = 512;
const MAX_CONTEXT: usize = 256;
const MAX_PATH: usize = 320;
const MAX_MESSAGES: usize = 32;
const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_TOTAL_MS: u64 = 5_000;
const VIEWPORT_WIDTH: u32 = 390;
const VIEWPORT_HEIGHT: u32 = 844;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Narrow,
    Restore,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    ws_url: String,
    context: String,
    mode: Mode,
}

#[derive(Debug, PartialEq, Eq)]
struct Endpoint {
    address: SocketAddr,
    host_header: String,
    path: String,
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    if arguments.len() > MAX_ARGS / 2 {
        return Err("too many arguments".into());
    }
    let mut ws_url = None;
    let mut context = None;
    let mut mode = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = arguments.get(index + 1).ok_or("flag value required")?;
        let slot = match flag.as_str() {
            "--ws-url" => &mut ws_url,
            "--context" => &mut context,
            "--mode" => &mut mode,
            _ => return Err("unknown argument".into()),
        };
        if slot.replace(value.clone()).is_some() {
            return Err("duplicate argument".into());
        }
        index += 2;
    }
    let ws_url = ws_url.ok_or("--ws-url required")?;
    let context = context.ok_or("--context required")?;
    let mode = match mode.as_deref().ok_or("--mode required")? {
        "narrow" => Mode::Narrow,
        "restore" => Mode::Restore,
        _ => return Err("mode must be narrow or restore".into()),
    };
    if ws_url.len() > MAX_URL || context.is_empty() || context.len() > MAX_CONTEXT {
        return Err("URL or context exceeds limit".into());
    }
    if !context
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
    {
        return Err("invalid context handle".into());
    }
    let _ = endpoint(&ws_url)?;
    Ok(Options {
        ws_url,
        context,
        mode,
    })
}

fn endpoint(input: &str) -> Result<Endpoint, String> {
    if input.len() > MAX_URL || !input.is_ascii() || input.contains(['?', '#', '@']) {
        return Err("invalid WebSocket URL".into());
    }
    let (path, ip, host_header) = if let Some(rest) = input.strip_prefix("ws://127.0.0.1:") {
        let (port, path) = rest.split_once('/').ok_or("session path required")?;
        let port = port.parse::<u16>().map_err(|_| "invalid loopback port")?;
        if port == 0 {
            return Err("invalid loopback port".into());
        }
        (
            format!("/{path}"),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            format!("127.0.0.1:{port}"),
        )
    } else if let Some(rest) = input.strip_prefix("ws://[::1]:") {
        let (port, path) = rest.split_once('/').ok_or("session path required")?;
        let port = port.parse::<u16>().map_err(|_| "invalid loopback port")?;
        if port == 0 {
            return Err("invalid loopback port".into());
        }
        (
            format!("/{path}"),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            format!("[::1]:{port}"),
        )
    } else {
        return Err("URL must use ws and a literal loopback address".into());
    };
    if path.len() > MAX_PATH
        || path.contains(['?', '#', '%', '\\'])
        || !path.starts_with("/session/")
        || path[9..].is_empty()
        || !path[9..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
    {
        return Err("invalid or unbounded driver session path".into());
    }
    let port = host_header
        .rsplit_once(':')
        .ok_or("invalid loopback authority")?
        .1
        .parse::<u16>()
        .map_err(|_| "invalid loopback port")?;
    Ok(Endpoint {
        address: SocketAddr::new(ip, port),
        host_header,
        path,
    })
}

struct BoundedStream {
    stream: TcpStream,
    deadline: Instant,
    input_bytes: usize,
}

impl BoundedStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "absolute BiDi deadline"))
    }

    fn configure_read(&self) -> io::Result<()> {
        self.stream.set_read_timeout(Some(self.remaining()?))
    }

    fn configure_write(&self) -> io::Result<()> {
        self.stream.set_write_timeout(Some(self.remaining()?))
    }
}

impl Read for BoundedStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.input_bytes >= MAX_INPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "BiDi input byte limit",
            ));
        }
        self.configure_read()?;
        let remaining = MAX_INPUT_BYTES - self.input_bytes;
        let limited_len = bytes.len().min(remaining);
        let count = self.stream.read(&mut bytes[..limited_len])?;
        self.input_bytes += count;
        Ok(count)
    }
}

impl Write for BoundedStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.configure_write()?;
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.configure_write()?;
        self.stream.flush()
    }
}

fn connect(endpoint: &Endpoint, deadline: Instant) -> Result<BoundedStream, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or("absolute BiDi deadline")?;
    let stream = TcpStream::connect_timeout(&endpoint.address, remaining)
        .map_err(|error| format!("loopback connect failed: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|_| "cannot configure loopback socket")?;
    Ok(BoundedStream {
        stream,
        deadline,
        input_bytes: 0,
    })
}

fn client(endpoint: &Endpoint, deadline: Instant) -> Result<WebSocket<BoundedStream>, String> {
    let stream = connect(endpoint, deadline)?;
    let request = format!("ws://{}{}", endpoint.host_header, endpoint.path);
    let mut request = request
        .into_client_request()
        .map_err(|_| "cannot form bounded WebSocket upgrade request")?;
    request.headers_mut().insert(
        tungstenite::http::header::HOST,
        endpoint
            .host_header
            .parse()
            .map_err(|_| "invalid loopback host header")?,
    );
    let config = WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .write_buffer_size(4 * 1024)
        .max_write_buffer_size(64 * 1024)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let (socket, _) = tungstenite::client::client_with_config(request, stream, Some(config))
        .map_err(|error| format!("WebSocket upgrade failed: {error}"))?;
    if Instant::now() >= deadline {
        return Err("absolute BiDi deadline".into());
    }
    Ok(socket)
}

fn matching_contexts(result: &Value, expected: &str) -> Result<(), String> {
    let contexts = result
        .get("contexts")
        .and_then(Value::as_array)
        .ok_or("getTree result contexts missing")?;
    if contexts.len() > 32 {
        return Err("getTree context limit".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut matches = 0usize;
    for item in contexts {
        let id = item
            .get("context")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= MAX_CONTEXT)
            .ok_or("getTree context id missing or invalid")?;
        if !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        {
            return Err("invalid getTree context id".into());
        }
        if !ids.insert(id) {
            return Err("duplicate getTree context id".into());
        }
        if id == expected {
            if !item.get("parent").is_some_and(Value::is_null) {
                return Err("supplied context is not a root context".into());
            }
            matches += 1;
        }
    }
    if matches != 1 {
        return Err("supplied window handle does not uniquely match a root context".into());
    }
    Ok(())
}

fn validate_reply(message: Message, expected_id: u8, method: &str) -> Result<Value, String> {
    let Message::Text(text) = message else {
        return Err("BiDi reply must be a text message".into());
    };
    if text.len() > MAX_FRAME_BYTES {
        return Err("BiDi reply exceeds size limit".into());
    }
    let value = crate::preflight::strict_json(text.as_str().as_bytes())
        .map_err(|_| "malformed or ambiguous BiDi JSON")?;
    if value.get("type").and_then(Value::as_str) != Some("success")
        || value.get("id").and_then(Value::as_u64) != Some(u64::from(expected_id))
        || value.get("error").is_some()
    {
        return Err("unexpected BiDi reply id, type or error".into());
    }
    let result = value.get("result").ok_or("BiDi result missing")?;
    if method == "browsingContext.setViewport"
        && (result.as_object().is_none_or(|object| !object.is_empty()))
    {
        return Err("setViewport result must be an empty object".into());
    }
    Ok(result.clone())
}

fn send_command(
    socket: &mut WebSocket<BoundedStream>,
    id: u8,
    method: &str,
    params: Value,
    deadline: Instant,
    received: &mut usize,
) -> Result<Value, String> {
    if Instant::now() >= deadline {
        return Err("absolute BiDi deadline".into());
    }
    let body = json!({"id": id, "method": method, "params": params}).to_string();
    if body.len() > 4 * 1024 {
        return Err("BiDi request exceeds size limit".into());
    }
    socket
        .send(Message::Text(body.into()))
        .map_err(|error| format!("BiDi send failed: {error}"))?;
    if Instant::now() >= deadline {
        return Err("absolute BiDi deadline".into());
    }
    if *received >= MAX_MESSAGES {
        return Err("BiDi received-message limit".into());
    }
    let message = socket
        .read()
        .map_err(|error| format!("BiDi reply read failed: {error}"))?;
    *received += 1;
    if Instant::now() >= deadline {
        return Err("absolute BiDi deadline".into());
    }
    validate_reply(message, id, method)
}

fn run(options: Options) -> Result<Value, String> {
    run_with_timeout(options, Duration::from_millis(MAX_TOTAL_MS))
}

fn run_with_timeout(options: Options, timeout: Duration) -> Result<Value, String> {
    let endpoint = endpoint(&options.ws_url)?;
    let deadline = Instant::now() + timeout.min(Duration::from_millis(MAX_TOTAL_MS));
    let mut socket = client(&endpoint, deadline)?;
    let mut received = 0;
    let tree = send_command(
        &mut socket,
        1,
        "browsingContext.getTree",
        json!({"maxDepth": 0}),
        deadline,
        &mut received,
    )?;
    matching_contexts(&tree, &options.context)?;
    let viewport = match options.mode {
        Mode::Narrow => json!({"width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT}),
        Mode::Restore => Value::Null,
    };
    let result = send_command(
        &mut socket,
        2,
        "browsingContext.setViewport",
        json!({"context": options.context, "viewport": viewport}),
        deadline,
        &mut received,
    )?;
    let _ = socket.close(None);
    Ok(json!({
        "command": "browsingContext.setViewport",
        "context": options.context,
        "mode": if options.mode == Mode::Narrow { "narrow" } else { "restore" },
        "result": result,
        "received_messages": received,
        "actual_viewport_verified": false
    }))
}

pub fn cli(arguments: Vec<String>) -> u8 {
    match parse(&arguments).and_then(run) {
        Ok(acknowledgement) => {
            println!("{acknowledgement}");
            0
        }
        Err(error) => {
            eprintln!("browser BiDi viewport failed: {error}");
            2
        }
    }
}

use tungstenite::client::IntoClientRequest;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    fn accept_with_deadline(listener: &TcpListener) -> io::Result<TcpStream> {
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                    return Ok(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "mock listener accept deadline",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn options(mode: &str) -> Options {
        parse(&[
            "--ws-url".into(),
            "ws://127.0.0.1:4444/session/abc123".into(),
            "--context".into(),
            "window-1".into(),
            "--mode".into(),
            mode.into(),
        ])
        .unwrap()
    }

    #[test]
    fn endpoint_admits_only_literal_loopback_session_urls() {
        assert_eq!(
            endpoint("ws://127.0.0.1:4444/session/abc123")
                .unwrap()
                .address,
            "127.0.0.1:4444".parse().unwrap()
        );
        assert_eq!(
            endpoint("ws://[::1]:4444/session/abc123").unwrap().address,
            "[::1]:4444".parse().unwrap()
        );
        for bad in [
            "wss://127.0.0.1:4444/session/a",
            "ws://localhost:4444/session/a",
            "ws://127.0.0.2:4444/session/a",
            "ws://127.0.0.1/session/a",
            "ws://127.0.0.1:4444/session/a?x=1",
            "ws://127.0.0.1:4444/session/a#x",
            "ws://u@127.0.0.1:4444/session/a",
            "ws://127.0.0.1:4444/other/a",
            "ws://127.0.0.1:4444/session/a/b",
        ] {
            assert!(endpoint(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn parser_rejects_duplicate_unknown_and_unbounded_inputs() {
        let mut duplicate = vec!["--mode".into(), "restore".into()];
        duplicate.extend_from_slice(&[
            "--ws-url".into(),
            "ws://127.0.0.1:4/session/a".into(),
            "--context".into(),
            "ctx".into(),
            "--mode".into(),
            "narrow".into(),
        ]);
        assert!(parse(&duplicate).is_err());
        assert!(
            parse(&[
                "--ws-url".into(),
                "x".into(),
                "--context".into(),
                "c".into(),
                "--bad".into(),
                "v".into()
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--ws-url".into(),
                "ws://127.0.0.1:4/session/a".into(),
                "--context".into(),
                "ctx".into(),
                "--mode".into(),
                "anything".into()
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--ws-url".into(),
                "ws://127.0.0.1:4/session/a".into(),
                "--context".into(),
                "x".repeat(MAX_CONTEXT + 1),
                "--mode".into(),
                "narrow".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn context_binding_accepts_other_roots_but_requires_one_unique_match() {
        let valid = json!({"contexts": [
            {"context": "window-1", "parent": null},
            {"context": "other-window", "parent": null}
        ]});
        assert!(matching_contexts(&valid, "window-1").is_ok());
        assert!(
            matching_contexts(
                &json!({"contexts": [{"context":"other", "parent":null}]}),
                "window-1"
            )
            .is_err()
        );
        assert!(
            matching_contexts(
                &json!({"contexts": [
                    {"context":"window-1", "parent":null}, {"context":"window-1", "parent":null}
                ]}),
                "window-1"
            )
            .is_err()
        );
        let too_many = json!({"contexts": (0..33).map(|i| json!({"context": format!("ctx-{i}"), "parent": null})).collect::<Vec<_>>()});
        assert!(matching_contexts(&too_many, "ctx-0").is_err());
    }

    #[test]
    fn reply_checker_rejects_representative_wrong_id_and_wrong_shape_defects() {
        let valid = Message::Text(
            json!({"type":"success", "id":2, "result":{}})
                .to_string()
                .into(),
        );
        assert_eq!(
            validate_reply(valid, 2, "browsingContext.setViewport").unwrap(),
            json!({})
        );
        for defect in [
            json!({"type":"success", "id":1, "result":{}}),
            json!({"type":"error", "id":2, "error":"unknown error", "message":"bad"}),
            json!({"type":"success", "id":2, "result":{"unexpected":true}}),
            json!({"type":"success", "id":2}),
        ] {
            assert!(
                validate_reply(
                    Message::Text(defect.to_string().into()),
                    2,
                    "browsingContext.setViewport"
                )
                .is_err()
            );
        }
        assert!(
            validate_reply(
                Message::Binary(vec![1, 2, 3].into()),
                2,
                "browsingContext.setViewport"
            )
            .is_err()
        );
        assert!(
            validate_reply(Message::Text("{".into()), 2, "browsingContext.setViewport").is_err()
        );
        assert!(
            validate_reply(
                Message::Text(r#"{"type":"success","type":"success","id":2,"result":{}}"#.into()),
                2,
                "browsingContext.setViewport"
            )
            .is_err()
        );
        assert!(
            validate_reply(
                Message::Text(r#"{"type":"success","id":2,"result":{},"result":{}}"#.into()),
                2,
                "browsingContext.setViewport"
            )
            .is_err()
        );
    }

    #[test]
    fn mode_arguments_encode_only_fixed_narrow_or_null_restore() {
        assert_eq!(options("narrow").mode, Mode::Narrow);
        assert_eq!(options("restore").mode, Mode::Restore);
    }

    fn serve(
        handler: impl FnOnce(WebSocket<TcpStream>) + Send + 'static,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("ws://{address}/session/mock-session");
        let worker = thread::spawn(move || {
            let Ok(stream) = accept_with_deadline(&listener) else {
                return;
            };
            if let Ok(socket) = tungstenite::accept(stream) {
                handler(socket);
            }
        });
        (url, worker)
    }

    fn read_command(socket: &mut WebSocket<TcpStream>, expected_method: &str) -> Value {
        let Message::Text(text) = socket.read().unwrap() else {
            panic!("expected text command")
        };
        let command: Value = serde_json::from_str(text.as_str()).unwrap();
        assert_eq!(command["method"], expected_method);
        command
    }

    fn tree_reply(id: u64) -> Message {
        Message::Text(
            json!({
                "type": "success",
                "id": id,
                "result": {"contexts": [{"context": "window-1", "parent": null}]}
            })
            .to_string()
            .into(),
        )
    }

    #[test]
    fn mock_server_observes_exact_bun_reference_command_sequence_and_payload() {
        let (url, worker) = serve(|mut socket| {
            let tree = read_command(&mut socket, "browsingContext.getTree");
            assert_eq!(tree["id"], 1);
            assert_eq!(tree["params"], json!({"maxDepth": 0}));
            socket.send(tree_reply(1)).unwrap();
            let viewport = read_command(&mut socket, "browsingContext.setViewport");
            assert_eq!(viewport["id"], 2);
            assert_eq!(
                viewport["params"],
                json!({
                    "context": "window-1",
                    "viewport": {"width": 390, "height": 844}
                })
            );
            socket
                .send(Message::Text(
                    json!({"type":"success", "id":2, "result":{}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        let acknowledgement = run(configured).unwrap();
        assert_eq!(acknowledgement["actual_viewport_verified"], false);
        assert_eq!(acknowledgement["received_messages"], 2);
        worker.join().unwrap();
    }

    #[test]
    fn restore_mode_sends_null_viewport_after_unique_handle_binding() {
        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            socket.send(tree_reply(1)).unwrap();
            let viewport = read_command(&mut socket, "browsingContext.setViewport");
            assert_eq!(viewport["params"]["viewport"], Value::Null);
            socket
                .send(Message::Text(
                    json!({"type":"success", "id":2, "result":{}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
        });
        let mut configured = options("restore");
        configured.ws_url = url;
        assert!(run(configured).is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn mock_server_defects_wrong_id_event_and_oversized_reply_fail_closed() {
        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            socket.send(tree_reply(99)).unwrap();
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        assert!(run(configured).is_err());
        worker.join().unwrap();

        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            socket
                .send(Message::Text(
                    json!({"type":"event", "method":"log.entryAdded", "params":{}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        assert!(run(configured).is_err());
        worker.join().unwrap();

        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            let over = "x".repeat(MAX_FRAME_BYTES + 1);
            socket.send(Message::Text(over.into())).unwrap();
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        assert!(run(configured).is_err());
        worker.join().unwrap();
    }

    #[test]
    fn one_absolute_deadline_covers_stalled_reply() {
        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            thread::sleep(Duration::from_millis(100));
            let _ = socket.send(tree_reply(1));
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        assert!(run_with_timeout(configured, Duration::from_millis(30)).is_err());
        worker.join().unwrap();
    }

    #[test]
    fn absolute_deadline_also_covers_stalled_upgrade_and_peer_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let _stream = accept_with_deadline(&listener).unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let mut configured = options("narrow");
        configured.ws_url = format!("ws://{address}/session/mock-session");
        assert!(run_with_timeout(configured, Duration::from_millis(30)).is_err());
        worker.join().unwrap();

        let (url, worker) = serve(|mut socket| {
            let _ = read_command(&mut socket, "browsingContext.getTree");
            drop(socket);
        });
        let mut configured = options("narrow");
        configured.ws_url = url;
        assert!(run(configured).is_err());
        worker.join().unwrap();
    }

    struct DribbleUpgrade(TcpStream);

    impl Read for DribbleUpgrade {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.read(bytes)
        }
    }

    impl Write for DribbleUpgrade {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_millis(15));
            self.0.write(&bytes[..bytes.len().min(1)])
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[test]
    fn absolute_deadline_covers_slowly_dribbled_upgrade_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handshake_finished = Arc::new(AtomicBool::new(false));
        let server_finished = Arc::clone(&handshake_finished);
        let worker = thread::spawn(move || {
            let Ok(stream) = accept_with_deadline(&listener) else {
                return;
            };
            if tungstenite::accept(DribbleUpgrade(stream)).is_ok() {
                server_finished.store(true, Ordering::Release);
            }
        });
        let mut configured = options("narrow");
        configured.ws_url = format!("ws://{address}/session/mock-session");
        let started = Instant::now();
        assert!(run_with_timeout(configured, Duration::from_millis(120)).is_err());
        assert!(started.elapsed() < Duration::from_millis(700));
        assert!(
            !handshake_finished.load(Ordering::Acquire),
            "deadline must expire while the server is still dribbling the Upgrade response"
        );
        worker.join().unwrap();
    }

    #[test]
    fn cumulative_input_wrapper_never_reads_past_two_mibibytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let mut stream = accept_with_deadline(&listener).unwrap();
            let bytes = vec![0x5a; MAX_INPUT_BYTES + 1];
            let _ = stream.write_all(&bytes);
        });
        let stream = TcpStream::connect(address).unwrap();
        let mut bounded = BoundedStream {
            stream,
            deadline: Instant::now() + Duration::from_secs(2),
            input_bytes: 0,
        };
        let mut bytes = vec![0u8; MAX_INPUT_BYTES + 1];
        let mut total = 0;
        while total < MAX_INPUT_BYTES {
            let count = bounded.read(&mut bytes[total..]).unwrap();
            assert!(count > 0);
            total += count;
        }
        assert_eq!(total, MAX_INPUT_BYTES);
        assert!(bounded.read(&mut bytes[MAX_INPUT_BYTES..]).is_err());
        assert_eq!(bounded.input_bytes, MAX_INPUT_BYTES);
        worker.join().unwrap();
    }
}
