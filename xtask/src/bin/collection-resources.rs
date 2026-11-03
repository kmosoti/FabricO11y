//! Outage saturation and recovery against the real SDK and Spindle process.
use fabric::{
    observation::{Data, Level},
    sdk::{Admission, Client, Config},
    spindle::{self, custody::Store},
};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};
use xtask::collection_lab::{config, daemon, rss_bytes};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("serve") {
        let cfg: spindle::Config =
            serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
        spindle::Daemon::open(cfg).unwrap().run().unwrap();
        return;
    }
    assert!(
        args.len() <= 2,
        "usage: collection-resources [absolute-spindle-path]"
    );
    let binary = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap());
    assert!(binary.is_absolute() && binary.is_file());
    let mut reports = Vec::new();
    // Fixed coverage cases, not performance-selection holdouts.
    for (slots, width, producers, case) in [
        (1, 1024, 1, "short-record"),
        (8, 4096, 4, "full-width"),
        (17, 2048, 3, "sequence-width"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = config(root.path());
        cfg.client_timeout_ms = 1000;
        let before = rss_bytes();
        let setup = Instant::now();
        let mut sdk = Config::local(cfg.socket.clone(), "resource-program".into()).unwrap();
        sdk.slots = slots;
        sdk.max_record = width;
        sdk.delivery_timeout = Duration::from_millis(100);
        let empty = fabric::observation::Observation {
            version: 2,
            registry: fabric::observation::REGISTRY.into(),
            identity: fabric::observation::Identity {
                producer: sdk.producer.clone(),
                generation: sdk.generation.clone(),
                sequence: slots as u64,
            },
            resource: sdk.resource.clone(),
            timestamp_ns: 1,
            data: Data::Log {
                level: Level::Info,
                message: String::new(),
            },
        };
        let overhead = serde_json::to_vec(&empty).unwrap().len();
        let payload_len = if case == "short-record" {
            width / 2
        } else {
            width - overhead
        };
        let client = Arc::new(Client::new(sdk).unwrap());
        let setup_ns = setup.elapsed().as_nanos();
        let barrier = Arc::new(Barrier::new(producers));
        let start = Instant::now();
        let workers: Vec<_> = (0..producers)
            .map(|_| {
                let c = client.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut timing = Vec::new();
                    let mut accepted = Vec::new();
                    let end = Instant::now() + Duration::from_secs(3);
                    let mut contended = 0;
                    loop {
                        assert!(timing.len() < 100_000, "admission attempt budget");
                        assert!(Instant::now() < end, "admission progress deadline");
                        // Caller allocation is outside the measured SDK call.
                        let data = Data::Log {
                            level: Level::Info,
                            message: "x".repeat(payload_len),
                        };
                        let t = Instant::now();
                        let result = c.try_record(data, 1);
                        timing.push((
                            t.elapsed().as_nanos(),
                            matches!(result, Admission::Accepted { .. }),
                        ));
                        match result {
                            Admission::Accepted { sequence } => accepted.push(sequence),
                            Admission::Contended => {
                                contended += 1;
                                std::thread::yield_now();
                            }
                            Admission::Full => break,
                            r => panic!("unexpected admission: {r:?}"),
                        }
                    }
                    (timing, accepted, contended)
                })
            })
            .collect();
        let mut timing = Vec::new();
        let mut accepted = Vec::new();
        let mut contended = 0;
        for w in workers {
            let (t, a, c) = w.join().unwrap();
            timing.extend(t);
            accepted.extend(a);
            contended += c;
        }
        accepted.sort();
        assert_eq!(accepted, (1..=slots as u64).collect::<Vec<_>>());
        let status = client.status();
        assert_eq!(status.charged_slots, slots);
        assert_eq!(status.confirmed, 0);
        assert_eq!(status.reserved_payload_bytes, slots * width);
        assert!(
            client
                .flush(Instant::now() + Duration::from_millis(30))
                .is_err()
        );
        assert_eq!(
            client.status().charged_slots,
            slots,
            "outage released capacity"
        );
        let outage_ns = start.elapsed().as_nanos();
        let rss = rss_bytes();
        let mut child = daemon(&binary, &cfg, root.path());
        let flush = Instant::now();
        let receipt = client
            .flush(Instant::now() + Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let flush_ns = flush.elapsed().as_nanos();
        // Incomplete maximum-sized frames must remain within configured connection buffers.
        use std::io::Write;
        let mut slow = Vec::new();
        for _ in 0..cfg.max_connections - 1 {
            let mut stream = std::os::unix::net::UnixStream::connect(&cfg.socket).unwrap();
            stream
                .write_all(&(fabric::observation::MAX_FRAME as u32).to_be_bytes())
                .unwrap();
            stream.write_all(b"{").unwrap();
            slow.push(stream);
        }
        let end = Instant::now() + Duration::from_millis(500);
        let collector = loop {
            let reply = spindle::transport::request(
                &cfg.socket,
                &spindle::Request::Status,
                Instant::now() + Duration::from_millis(200),
            )
            .unwrap();
            if let spindle::Reply::Status { status } = reply {
                assert!(status.active_connections <= cfg.max_connections);
                assert!(
                    status.queue_bytes
                        <= cfg.max_connections
                            * (fabric::observation::MAX_FRAME + spindle::MAX_REPLY + 4)
                );
                if status.active_connections >= slow.len()
                    && status.queue_bytes >= slow.len() * fabric::observation::MAX_FRAME
                {
                    break status;
                }
            }
            assert!(Instant::now() < end, "slow connection observation deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(collector.queue_bytes >= slow.len() * fabric::observation::MAX_FRAME);
        drop(slow);

        assert_eq!(receipt.identity.sequence, slots as u64);
        assert_eq!(client.status().charged_slots, 0);
        drop(client);
        assert!(matches!(
            spindle::transport::request(
                &cfg.socket,
                &spindle::Request::Stop,
                Instant::now() + Duration::from_secs(2)
            ),
            Ok(spindle::Reply::Stopped)
        ));
        // kill/wait also handles the short graceful-shutdown window before reopening.
        child.kill_wait();
        let store = Store::open(&cfg.storage, cfg.quota_bytes).unwrap();
        assert_eq!(store.count().unwrap(), slots as u64);
        let records = store.read(0, slots).unwrap();
        let mut payload_bytes = 0;
        for (index, (_, record)) in records.iter().enumerate() {
            assert_eq!(record.identity.sequence, index as u64 + 1);
            assert!(
                matches!(&record.data,Data::Log{message,..} if message==&"x".repeat(payload_len))
            );
            payload_bytes += serde_json::to_vec(record).unwrap().len();
        }
        if case == "full-width" {
            assert_eq!(payload_bytes, slots * width);
        }
        timing.sort();
        let admitted_timing: Vec<_> = timing
            .iter()
            .filter(|(_, accepted)| *accepted)
            .map(|(n, _)| *n)
            .collect();
        reports.push(serde_json::json!({"case":case,"slots":slots,"record_capacity":width,
            "producers":producers,"reserved_payload_bytes":slots*width,"retained_encoded_bytes":payload_bytes,
            "unused_payload_bytes":slots*width-payload_bytes,"rss_before":before,"rss_at_full":rss,
            "setup_ns":setup_ns,"outage_phase_ns":outage_ns,"recovery_flush_ns":flush_ns,
            "all_call_p50_ns":timing[(timing.len()*50).div_ceil(100)-1].0,
            "all_call_p99_ns":timing[(timing.len()*99).div_ceil(100)-1].0,
            "accepted_call_p50_ns":admitted_timing[(admitted_timing.len()*50).div_ceil(100)-1],
            "accepted_call_p99_ns":admitted_timing[(admitted_timing.len()*99).div_ceil(100)-1],
            "calls":timing.len(),"contended":contended,"collector_queue_bytes_with_slow_clients":collector.queue_bytes,
            "collector_rss_with_slow_clients":collector.process_resident_bytes,
            "storage_bytes":store.disk_bytes().unwrap()}));
    }
    println!(
        "{}",
        serde_json::json!({"passed":true,"workloads":reports,
        "scope":"One run per configuration; SDK calls include input drop but exclude input construction; all admission outcomes included. RSS snapshots include harness and allocator, not a hard bound or isolated delta. Persistent daemon is separate. Warm cache uncontrolled; no latency optimality claim."})
    );
}
