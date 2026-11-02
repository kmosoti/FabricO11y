//! Deterministic network schedules plus real commit/receipt process failure windows.
use serde_json::json;
use std::{
    io::{BufRead, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use xtask::{
    collection_lab::OwnedChild,
    representation::{self as r, Codec, custody::Store, fixtures},
};
fn worker(path: &Path, mode: &str) {
    let mut store = Store::open(path, 32).unwrap();
    println!("ready");
    std::io::stdout().flush().unwrap();
    let mut n = [0; 4];
    std::io::stdin().read_exact(&mut n).unwrap();
    let n = u32::from_le_bytes(n) as usize;
    assert!(n <= r::MAX_FRAME);
    let mut bytes = vec![0; n];
    std::io::stdin().read_exact(&mut bytes).unwrap();
    if mode == "before" {
        println!("before");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let records = r::decode(&bytes).unwrap();
    store.commit("origin-a", &records, Codec::Columns).unwrap();
    println!("committed");
    std::io::stdout().flush().unwrap();
    if mode == "lost-seal" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    println!("Fabric Seal");
}
fn killed_commit(path: &Path, frame: &[u8], mode: &str) {
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["worker", path.to_str().unwrap(), mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "ready");
    let stdin = child.0.stdin.as_mut().unwrap();
    stdin
        .write_all(&(frame.len() as u32).to_le_bytes())
        .unwrap();
    stdin.write_all(frame).unwrap();
    stdin.flush().unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        if mode == "before" {
            "before"
        } else {
            "committed"
        }
    );
    child.kill_wait();
    reader.join().unwrap();
    assert!(!rx.try_iter().any(|s| s == "Fabric Seal"));
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "worker") {
        worker(Path::new(&args[2]), &args[3]);
        return;
    }
    let mut cases = Vec::new();
    for codec in Codec::ALL {
        for seed in [17u64, 43, 991] {
            let root = tempfile::tempdir().unwrap();
            let server = root.path().join("server.sqlite");
            let local = root.path().join("spindle.sqlite");
            let records = fixtures::records(seed, "mixed", 16);
            let p = &records[0].identity.producer;
            let g = &records[0].identity.generation;
            {
                let mut store = Store::open(&local, 32).unwrap();
                store.commit("origin-a", &records, codec).unwrap();
                assert_eq!(store.frontier("origin-a", p, g).unwrap(), 16);
            }
            let first = r::encode(codec, &records[..1]).unwrap();
            killed_commit(&server, &first, "before");
            {
                let store = Store::open(&server, 32).unwrap();
                assert_eq!(store.frontier("origin-a", p, g).unwrap(), 0);
            }
            killed_commit(&server, &first, "lost-seal");
            let mut rng = seed;
            let mut schedule: Vec<usize> = (1..16).collect();
            for i in (1..schedule.len()).rev() {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let j = (rng as usize) % (i + 1);
                schedule.swap(i, j);
            }
            let mut known = std::collections::BTreeSet::from([1u64]);
            let mut delivered = 0;
            for (step, index) in schedule.iter().enumerate() {
                if step % 3 == 0 {
                    continue;
                } // Lost/delayed network messages; bounded retry backlog remains at sender.
                let frame = r::encode(codec, &records[*index..=*index]).unwrap();
                let mut store = Store::open(&server, 32).unwrap();
                let batch = r::decode(&frame).unwrap();
                store.commit("origin-a", &batch, codec).unwrap();
                store.commit("origin-a", &batch, codec).unwrap();
                known.insert(*index as u64 + 1);
                delivered += 1;
                let expected = (1..=16).take_while(|n| known.contains(n)).count() as u64;
                assert_eq!(store.frontier("origin-a", p, g).unwrap(), expected);
            }
            // Fair retry suffix: every pending record eventually arrives with capacity available.
            let mut store = Store::open(&server, 32).unwrap();
            store.commit("origin-a", &records, codec).unwrap();
            assert_eq!(store.frontier("origin-a", p, g).unwrap(), 16);
            let mut conflict = records[0].clone();
            conflict.timestamp_ns += 1;
            assert!(
                store
                    .commit("origin-a", &[conflict], codec)
                    .unwrap_err()
                    .contains("conflict")
            );
            // Same SDK identity under another persistent origin must not collide.
            store.commit("origin-b", &records, codec).unwrap();
            assert_eq!(store.read("origin-b").unwrap().len(), 16);
            let mut extra = records[0].clone();
            extra.identity.sequence = 17;
            assert!(
                store
                    .commit("origin-a", &[extra], codec)
                    .unwrap_err()
                    .contains("full")
            );
            assert_eq!(
                serde_json::to_vec(&store.read("origin-a").unwrap()).unwrap(),
                serde_json::to_vec(&records).unwrap()
            );
            drop(store);
            let store = Store::open(&server, 32).unwrap();
            assert_eq!(store.frontier("origin-a", p, g).unwrap(), 16);
            let local = Store::open(&local, 32).unwrap();
            assert_eq!(local.frontier("origin-a", p, g).unwrap(), 16);
            cases.push(json!({"codec":codec.name(),"seed":seed,"out_of_order_deliveries":delivered,"spindle_seal":16,"fabric_seal":16,"origins":2,"passed":true}));
        }
    }
    println!(
        "{}",
        json!({"passed":true,"cases":cases,"scope":"seeded channel loss/reorder/duplicate schedules; real SIGKILL before commit and after commit before Fabric Seal; atomic durable store assumed by proofs; no power-loss or replication claim"})
    );
}
