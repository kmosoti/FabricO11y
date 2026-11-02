//! Kill a real custody writer at controlled and timed fault windows.
use fabric::spindle::custody::{Checkpoint, Store};
use std::{
    io::{BufRead, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use xtask::collection_lab::{OwnedChild, observation};
const QUOTA: u64 = 4 * 1024 * 1024;
fn bytes(n: u64) -> Vec<u8> {
    serde_json::to_vec(&observation(n)).unwrap()
}
fn worker(path: &Path, mode: &str) {
    let mut store = Store::open(path, QUOTA).unwrap();
    println!("ready");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    if mode == "before" {
        println!("before");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    for n in 1..=64 {
        let receipt = store
            .submit(
                &bytes(n),
                Some(&Checkpoint {
                    source: "oracle".into(),
                    position: n.to_string(),
                }),
            )
            .unwrap();
        if mode == "lost-ack" {
            println!("committed");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        println!("ack {} {}", n, receipt.id);
        std::io::stdout().flush().unwrap();
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--worker") {
        worker(Path::new(&args[2]), &args[3]);
        return;
    }
    assert_eq!(args.len(), 1, "usage: collection-faults");
    let mut reports = Vec::new();
    for (mode, delay) in [
        ("before", 0),
        ("lost-ack", 0),
        ("timed", 1),
        ("timed", 5),
        ("timed", 20),
        ("timed", 70),
        ("timed", 150),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("store");
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .arg("--worker")
                .arg(&path)
                .arg(mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if send.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        assert_eq!(recv.recv_timeout(Duration::from_secs(5)).unwrap(), "ready");
        child.0.stdin.take().unwrap().write_all(b"start\n").unwrap();
        if mode != "timed" {
            assert_eq!(
                recv.recv_timeout(Duration::from_secs(5)).unwrap(),
                if mode == "before" {
                    "before"
                } else {
                    "committed"
                }
            );
        } else {
            std::thread::sleep(Duration::from_millis(delay));
        }
        child.kill_wait();
        reader.join().unwrap();
        let acknowledged: Vec<_> = recv
            .try_iter()
            .map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                assert_eq!(fields[0], "ack");
                (
                    fields[1].parse::<u64>().unwrap(),
                    fields[2].parse::<u64>().unwrap(),
                )
            })
            .collect();
        let mut store = Store::open(&path, QUOTA).unwrap();
        let count = store.count().unwrap();
        if mode == "before" {
            assert_eq!(count, 0);
        }
        if mode == "lost-ack" {
            assert_eq!(count, 1);
            assert!(acknowledged.is_empty());
        }
        assert_eq!(
            store.checkpoint("oracle").unwrap(),
            if count == 0 {
                None
            } else {
                Some(count.to_string())
            }
        );
        let rows = store.read(0, 1000).unwrap();
        assert_eq!(rows.len() as u64, count);
        for (index, (id, row)) in rows.iter().enumerate() {
            assert_eq!(*id, index as u64 + 1);
            assert_eq!(serde_json::to_vec(row).unwrap(), bytes(index as u64 + 1));
        }
        for (n, id) in &acknowledged {
            assert!(*n <= count, "lost acknowledged record");
            let receipt = store.submit(&bytes(*n), None).unwrap();
            assert!(receipt.duplicate);
            assert_eq!(receipt.id, *id);
        }
        // Retry every attempted identity, including unknown outcomes, to a fixed oracle.
        for n in 1..=64 {
            let r = store
                .submit(
                    &bytes(n),
                    Some(&Checkpoint {
                        source: "oracle".into(),
                        position: n.to_string(),
                    }),
                )
                .unwrap();
            assert_eq!(r.duplicate, n <= count);
            assert_eq!(r.id, n);
        }
        let mut conflict = observation(1);
        conflict.timestamp_ns += 1;
        assert!(
            store
                .submit(&serde_json::to_vec(&conflict).unwrap(), None)
                .unwrap_err()
                .contains("identity conflict")
        );
        assert_eq!(store.count().unwrap(), 64);
        assert_eq!(store.checkpoint("oracle").unwrap(), Some("64".into()));
        drop(store);
        let store = Store::open(&path, QUOTA).unwrap();
        for (index, (_, row)) in store.read(0, 1000).unwrap().iter().enumerate() {
            assert_eq!(serde_json::to_vec(row).unwrap(), bytes(index as u64 + 1));
        }
        reports.push(serde_json::json!({"mode":mode,"kill_delay_ms":delay,"acknowledged":acknowledged.len(),"recovered":count,"final_records":store.count().unwrap()}));
    }
    println!(
        "{}",
        serde_json::json!({"passed":true,"cases":reports,"scope":"SIGKILL process failures, not power loss or arbitrary storage corruption. Controlled pre-submit and post-commit/pre-ack windows plus timed kills; timed kills do not guarantee every internal SQLite instruction was interrupted."})
    );
}
