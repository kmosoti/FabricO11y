//! Delivery over a simulated network (turmoil): the real server router and
//! store, and a Spindle-like sender that follows the delivery protocol, with
//! partitions, random link failures, held messages and latency injected on a
//! seeded schedule.
//!
//! What is simulated: the network and tokio time. What is real: the server's
//! HTTP handlers, commit thread (in individual-commit mode), journal and
//! fsync. The network schedule is
//! seeded, but the commit thread runs on an OS thread, so a run is not fully
//! deterministic; the checked properties hold for every schedule. Process
//! crashes are covered by the real-process fault harness
//! (tools/qualification/delivery_faults.py) and its TLA+ trace validation.
//!
//! Properties (ADR-0013):
//! - every Batch the sender offers is eventually acknowledged;
//! - a retry of the same bytes after a lost answer is acknowledged, never a
//!   conflict;
//! - the server's retained history holds each sequence exactly once, in
//!   order, with the bytes of its first offer.

use bytes::Bytes;
use fabric_frame::envelope::Batch;
use fabric_server::control::Control;
use fabric_server::http::{AppState, router};
use fabric_server::query::History;
use fabric_server::store::{CommitMode, Store};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use prost::Message;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ADMIN: &str = "sim-admin-token";
const BATCHES: u64 = 40;
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(1_500);

struct SimListener(turmoil::net::TcpListener);

impl axum::serve::Listener for SimListener {
    type Io = turmoil::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if let Ok(accepted) = self.0.accept().await {
                return accepted;
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

/// A state directory on tmpfs when there is one, so fsync is fast and the
/// real commit thread interferes little with simulated time.
fn state_dir(name: &str) -> PathBuf {
    let base = if std::path::Path::new("/dev/shm").is_dir() {
        PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    let dir = base.join(format!("fabric-sim-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn batch(sequence: u64) -> Vec<u8> {
    Batch {
        version: 1,
        node_id: vec![9; 16],
        generation: 1,
        sequence,
        metrics: Vec::new(),
        logs: Vec::new(),
        cursors: Vec::new(),
        collection_gaps: vec![format!("simulated batch {sequence}")],
    }
    .encode_to_vec()
}

/// One HTTP request on a fresh connection; `None` when the network or the
/// server did not answer within the attempt timeout.
async fn call(method: &str, path: &str, token: &str, body: Vec<u8>) -> Option<(u16, Value)> {
    let attempt = async {
        let stream = turmoil::net::TcpStream::connect(("server", 80))
            .await
            .ok()?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .ok()?;
        tokio::spawn(connection);
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "server")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/octet-stream")
            .body(Full::new(Bytes::from(body)))
            .ok()?;
        let response = sender.send_request(request).await.ok()?;
        let status = response.status().as_u16();
        let bytes = response.into_body().collect().await.ok()?.to_bytes();
        Some((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    };
    tokio::time::timeout(ATTEMPT_TIMEOUT, attempt)
        .await
        .ok()
        .flatten()
}

/// Retry `call` until it answers; counts the attempts left unanswered.
async fn call_until_answered(
    method: &str,
    path: &str,
    token: &str,
    body: Vec<u8>,
    unanswered: &mut u64,
) -> (u16, Value) {
    loop {
        if let Some(answer) = call(method, path, token, body.clone()).await {
            return answer;
        }
        *unanswered += 1;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// What the sender saw: the retained history it read back, and how many of
/// its attempts went unanswered (lost requests or lost answers).
#[derive(Debug, Default)]
struct Report {
    retained: Vec<(u64, String)>,
    unanswered: u64,
}

fn serve(sim: &mut turmoil::Sim<'_>, dir: PathBuf) {
    sim.host("server", move || {
        let dir = dir.clone();
        async move {
            let control = Arc::new(Mutex::new(Control::open(&dir)?));
            // Individual commits: the grouped mode waits up to 50 ms of real
            // time for a group, and simulated time runs far faster than real
            // time, so that wait became many simulated seconds and every
            // attempt timed out on a fast CI host. Grouping is covered by the
            // server's own delivery tests and the fault harness.
            let store = Store::open_with(&dir, 1 << 30, 64 << 20, CommitMode::INDIVIDUAL)?;
            let (intake, _commit_thread) = store.spawn_joinable()?;
            let app = router(AppState {
                intake,
                control,
                admin_token_sha256: Sha256::digest(ADMIN.as_bytes()).into(),
                history: Arc::new(History::new(&dir)),
            });
            let listener = turmoil::net::TcpListener::bind(("0.0.0.0", 80)).await?;
            axum::serve(SimListener(listener), app).await?;
            Ok(())
        }
    });
}

/// The sender: enroll, then deliver 1..=BATCHES one at a time with the
/// same bytes on every retry, and finally read back what was retained.
fn spindle(sim: &mut turmoil::Sim<'_>, report: Arc<Mutex<Option<Report>>>) {
    sim.client("spindle", async move {
        let mut unanswered = 0;
        let (status, enrolled) = call_until_answered(
            "POST",
            "/v1/admin/nodes",
            ADMIN,
            br#"{"name":"sim-node","metric_interval_s":15,"logs":[]}"#.to_vec(),
            &mut unanswered,
        )
        .await;
        assert_eq!(status, 201, "enroll: {enrolled}");
        let token = enrolled["token"].as_str().unwrap().to_owned();

        let mut next = 1;
        while next <= BATCHES {
            let (status, answer) =
                call_until_answered("POST", "/v1/batches", &token, batch(next), &mut unanswered)
                    .await;
            match (status, answer["status"].as_str()) {
                (200, Some("ack")) => {
                    let through = answer["committed_through"].as_u64().unwrap();
                    assert!(through >= next, "ack through {through} for sequence {next}");
                    next = through + 1;
                }
                (409, Some("gap")) => {
                    next = answer["committed_through"].as_u64().unwrap() + 1;
                }
                (503, _) => tokio::time::sleep(Duration::from_millis(100)).await,
                other => panic!("sequence {next}: unexpected answer {other:?} {answer}"),
            }
        }

        let query = serde_json::json!({
            "kind": "logs", "node": "sim-node", "from_ns": 0,
            "to_ns": u64::MAX, "limit": 10, "page": null,
        });
        let (status, answer) = call_until_answered(
            "POST",
            "/v1/admin/query",
            ADMIN,
            query.to_string().into_bytes(),
            &mut unanswered,
        )
        .await;
        assert_eq!(status, 200, "query: {answer}");
        let retained = answer["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["sequence"].as_u64().unwrap(),
                    g["gap"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        *report.lock().unwrap() = Some(Report {
            retained,
            unanswered,
        });
        Ok(())
    });
}

fn run(seed: u64, faults: impl Fn(&mut turmoil::Sim<'_>, Duration)) -> u64 {
    let dir = state_dir(&format!("seed{seed}"));
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .simulation_duration(Duration::from_secs(600))
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(40))
        .fail_rate(0.02)
        .repair_rate(0.2)
        .build();
    let report = Arc::new(Mutex::new(None));
    serve(&mut sim, dir.clone());
    spindle(&mut sim, Arc::clone(&report));
    loop {
        let elapsed = sim.elapsed();
        faults(&mut sim, elapsed);
        if sim.step().unwrap() {
            break;
        }
    }
    drop(sim);
    let report = report
        .lock()
        .unwrap()
        .take()
        .expect("the sender read back its history");
    let expected: Vec<(u64, String)> = (1..=BATCHES)
        .map(|s| (s, format!("simulated batch {s}")))
        .collect();
    assert_eq!(report.retained, expected, "seed {seed}: retained history");
    let _ = std::fs::remove_dir_all(&dir);
    report.unanswered
}

/// Random link failures and latency only.
#[test]
fn delivery_survives_a_lossy_network() {
    let unanswered: u64 = (1..=3).map(|seed| run(seed, |_, _| {})).sum();
    assert!(unanswered > 0, "the lossy network never lost an attempt");
}

/// Repeated partitions, and answers held back and then released, so that
/// requests commit while their answers are lost and the sender retries.
#[test]
fn delivery_survives_partitions_and_lost_answers() {
    // Every 10 s: hold the server's answers at 2 s (requests still reach it
    // and commit), partition at 4 s, repair and release at 6 s.
    let unanswered: u64 = (11..=13)
        .map(|seed| {
            run(seed, |sim, elapsed| match elapsed.as_millis() % 10_000 {
                2_000 => sim.hold("server", "spindle"),
                4_000 => sim.partition("spindle", "server"),
                6_000 => {
                    sim.repair("spindle", "server");
                    sim.release("server", "spindle");
                }
                _ => {}
            })
        })
        .sum();
    assert!(unanswered > 0, "no attempt lost its answer");
}
