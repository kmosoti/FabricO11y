//! One sample per fresh process; parent orchestrates separate SDK/Spindle/server stages.
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};
use xtask::{
    collection_lab::OwnedChild,
    representation::{self as r, Codec, custody::Store, fixtures},
};
#[cfg(feature = "allocation-metrics")]
#[global_allocator]
static ALLOC: r::allocation::Counter = r::allocation::Counter;
fn cpu() -> u128 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    t.tv_sec as u128 * 1_000_000_000 + t.tv_nsec as u128
}
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|s| s.strip_prefix("VmHWM:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}
fn write_frame(w: &mut impl Write, b: &[u8]) {
    w.write_all(&(b.len() as u32).to_le_bytes()).unwrap();
    w.write_all(b).unwrap();
    w.flush().unwrap();
}
fn read_frame(reader: &mut impl Read) -> Vec<u8> {
    let mut n = [0; 4];
    reader.read_exact(&mut n).unwrap();
    let n = u32::from_le_bytes(n) as usize;
    assert!(n <= r::MAX_FRAME);
    let mut b = vec![0; n];
    reader.read_exact(&mut b).unwrap();
    b
}
fn line(reader: &mut impl BufRead) -> Value {
    let mut s = String::new();
    reader.read_line(&mut s).unwrap();
    serde_json::from_str(&s).unwrap()
}
fn emit(v: Value) {
    println!("{v}");
    std::io::stdout().flush().unwrap();
}
fn codec(s: &str) -> Codec {
    Codec::ALL.into_iter().find(|c| c.name() == s).unwrap()
}
fn spawn(args: &[&str]) -> OwnedChild {
    OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
fn worker(role: &str, path: &Path, mode: &str, c: Codec) {
    let setup = Instant::now();
    let mut downstream = if role == "spindle" {
        Some(spawn(&[
            "worker",
            "server",
            path.join("server").to_str().unwrap(),
            mode,
            c.name(),
        ]))
    } else {
        None
    };
    let mut downstream_reader = downstream
        .as_mut()
        .map(|child| std::io::BufReader::new(child.0.stdout.take().unwrap()));
    if let Some(reader) = &mut downstream_reader {
        assert_eq!(line(reader)["ready"], true);
    }
    std::fs::create_dir_all(path).unwrap();
    let db = path.join("native.sqlite");
    let production = path.join("compatible");
    let mut native = if mode == "native" {
        Some(Store::open(&db, 1000).unwrap())
    } else {
        None
    };
    let mut compatible = if mode == "compatible" {
        Some(fabric::spindle::custody::Store::open(&production, 8 * 1024 * 1024).unwrap())
    } else {
        None
    };
    let setup_ns = setup.elapsed().as_nanos();
    emit(json!({"ready":true}));
    let begin = cpu();
    let frame = read_frame(&mut std::io::stdin());
    // Model stage service separately from waiting for an upstream process.
    let start = Instant::now();
    let decode_start = cpu();
    let validated = r::validate_frame(&frame).unwrap();
    let records = validated.records();
    let decode_cpu = cpu() - decode_start;
    let commit = Instant::now();
    if let Some(store) = &mut native {
        store
            .commit_validated("persistent-origin", &validated)
            .unwrap();
    }
    if let Some(store) = &mut compatible {
        for record in records {
            store
                .submit(&serde_json::to_vec(record).unwrap(), None)
                .unwrap();
        }
    }
    let commit_ns = commit.elapsed().as_nanos();
    let seal_ns = start.elapsed().as_nanos();
    if role == "spindle" {
        emit(json!({"seal":"Spindle Seal","local_stage_ns":seal_ns}));
    }
    let stage_cpu = cpu() - begin;
    let forward_start = cpu();
    if let Some(child) = &mut downstream {
        write_frame(child.0.stdin.as_mut().unwrap(), &frame);
        let seal = line(downstream_reader.as_mut().unwrap());
        assert_eq!(seal["seal"], "Fabric Seal");
        emit(seal);
    } else {
        emit(json!({"seal":"Fabric Seal","server_stage_ns":seal_ns}));
    }
    let forward_cpu = cpu() - forward_start;
    let materialize = cpu();
    let restored = if let Some(store) = &native {
        store.read("persistent-origin").unwrap()
    } else {
        compatible
            .as_ref()
            .unwrap()
            .read(0, 32)
            .unwrap()
            .into_iter()
            .map(|(_, o)| o)
            .collect()
    };
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&records).unwrap()
    );
    let materialize_cpu = cpu() - materialize;
    let project = cpu();
    let seqs: Vec<String> = if let Some(store) = &native {
        store.projected_sequences("persistent-origin").unwrap()
    } else {
        compatible
            .as_ref()
            .unwrap()
            .read(0, 32)
            .unwrap()
            .iter()
            .map(|(_, o)| o.identity.sequence.to_string())
            .collect()
    };
    assert_eq!(
        seqs,
        records
            .iter()
            .map(|o| o.identity.sequence.to_string())
            .collect::<Vec<_>>()
    );
    let projection_cpu = cpu() - project;
    drop(native);
    drop(compatible);
    let reopen = Instant::now();
    let recovered: Vec<_> = if mode == "native" {
        let store = Store::open(&db, 1000).unwrap();
        store.read("persistent-origin").unwrap()
    } else {
        let store = fabric::spindle::custody::Store::open(&production, 8 * 1024 * 1024).unwrap();
        store
            .read(0, 32)
            .unwrap()
            .into_iter()
            .map(|(_, o)| o)
            .collect()
    };
    assert_eq!(
        serde_json::to_vec(&recovered).unwrap(),
        serde_json::to_vec(&records).unwrap()
    );
    let recovery_ns = reopen.elapsed().as_nanos();
    let disk = if mode == "native" {
        std::fs::metadata(db).unwrap().len()
    } else {
        std::fs::read_dir(production)
            .unwrap()
            .map(|p| p.unwrap().metadata().unwrap().len())
            .sum()
    };
    let report = json!({"role":role,"cpu_ns":stage_cpu+forward_cpu,"decode_cpu_ns":decode_cpu,"commit_ns":commit_ns,"setup_ns":setup_ns,"seal_stage_ns":seal_ns,"materialize_cpu_ns":materialize_cpu,"projection_cpu_ns":projection_cpu,"recovery_ns":recovery_ns,"recovered_records":recovered.len(),"disk_bytes":disk,"peak_rss_bytes":rss(),"allocations":r::allocation::stats()});
    if let Some(child) = &mut downstream {
        let server = line(downstream_reader.as_mut().unwrap());
        assert!(child.0.wait().unwrap().success());
        emit(json!({"spindle":report,"server":server}));
    } else {
        emit(report);
    }
}
fn sample(seed: u64, case: &str, batch: usize, c: Codec, mode: &str) {
    let records = fixtures::records(seed, case, batch);
    let root = tempfile::tempdir().unwrap();
    let mut spindle = spawn(&[
        "worker",
        "spindle",
        root.path().to_str().unwrap(),
        mode,
        c.name(),
    ]);
    let mut reader = std::io::BufReader::new(spindle.0.stdout.take().unwrap());
    let setup = Instant::now();
    assert_eq!(line(&mut reader)["ready"], true);
    let setup_ns = setup.elapsed().as_nanos();
    let cpu_start = cpu();
    let start = Instant::now();
    let frame = r::encode(c, &records).unwrap();
    let encode_cpu = cpu() - cpu_start;
    write_frame(spindle.0.stdin.as_mut().unwrap(), &frame);
    let local = line(&mut reader);
    assert_eq!(local["seal"], "Spindle Seal");
    let local_ns = start.elapsed().as_nanos();
    let fabric = line(&mut reader);
    let fabric_ns = start.elapsed().as_nanos();
    assert_eq!(fabric["seal"], "Fabric Seal");
    let sender_cpu = cpu() - cpu_start;
    let stages = line(&mut reader);
    assert!(spindle.0.wait().unwrap().success());
    let wire = frame.len() + 4;
    let (dictionary_entries, dictionary_bytes) = r::dictionary_usage(&frame);
    let service = encode_cpu
        .max(stages["spindle"]["seal_stage_ns"].as_u64().unwrap() as u128)
        .max(stages["server"]["seal_stage_ns"].as_u64().unwrap() as u128);
    let network:Vec<_>=[10u64,100,1000].into_iter().flat_map(|mbps|[1u64,20,100].into_iter().map(move|rtt|{
        let transfer=wire as u128*8000/mbps as u128;let rtt_ns=rtt as u128*1_000_000;
        json!({"mbps":mbps,"rtt_ms":rtt,"added_remote_ns":transfer+rtt_ns,
            "serial_fabric_seal_estimate_ns":fabric_ns+transfer+rtt_ns,
            "pipeline_window":32,"pipeline_records_per_second_upper_bound":batch as f64*1e9/service.max(transfer).max(rtt_ns/32) as f64})
    })).collect();
    let batching: Vec<_> = [100u64, 1000, 10000]
        .into_iter()
        .map(|rate| {
            json!({"arrival_records_per_second":rate,
        "mean_batch_fill_ns":(batch-1) as u128*1_000_000_000/(2*rate as u128),
        "first_record_batch_fill_ns":(batch-1) as u128*1_000_000_000/rate as u128})
        })
        .collect();
    emit(
        json!({"case":case,"seed":seed,"batch":batch,"codec":c.name(),"custody":mode,"setup_ns":setup_ns,"sender_encode_cpu_ns":encode_cpu,"sender_peak_rss_bytes":rss(),"sender_allocations":r::allocation::stats(),"wire_bytes_per_hop":frame.len()+4,"encoded_buffer_capacity":frame.capacity(),"dictionary_entries":dictionary_entries,"dictionary_bytes":dictionary_bytes,"unchanged_production_sdk_reservation_bytes":64*65536,"spindle_seal_ns":local_ns,"fabric_seal_ns":fabric_ns,"records_per_second":batch as f64*1e9/fabric_ns as f64,"sender_cpu_ns":sender_cpu,"total_ingest_cpu_ns":sender_cpu+stages["spindle"]["cpu_ns"].as_u64().unwrap() as u128+stages["server"]["cpu_ns"].as_u64().unwrap() as u128,"stages":stages,"network_model":network,"batch_fill_model":batching,"allocation_run":cfg!(feature="allocation-metrics")}),
    );
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("worker") => worker(&args[2], Path::new(&args[3]), &args[4], codec(&args[5])),
        Some("sample") => sample(
            args[2].parse().unwrap(),
            &args[3],
            args[4].parse().unwrap(),
            codec(&args[5]),
            &args[6],
        ),
        Some("run") => {
            let held = args.get(2).is_some_and(|s| s == "held-out");
            let seed = if held { 991 } else { 17 };
            for case in fixtures::CASES {
                for batch in [1, if *case == "maximum" { 7 } else { 32 }] {
                    for c in Codec::ALL {
                        for mode in ["compatible", "native"] {
                            for repetition in 0..6 {
                                let out = Command::new("timeout")
                                    .args([
                                        "30",
                                        std::env::current_exe().unwrap().to_str().unwrap(),
                                        "sample",
                                        &seed.to_string(),
                                        case,
                                        &batch.to_string(),
                                        c.name(),
                                        mode,
                                    ])
                                    .output()
                                    .unwrap();
                                assert!(
                                    out.status.success(),
                                    "{}",
                                    String::from_utf8_lossy(&out.stderr)
                                );
                                let mut row: Value = serde_json::from_slice(&out.stdout).unwrap();
                                row["repetition"] = json!(repetition);
                                row["warmup"] = json!(repetition == 0);
                                emit(row);
                            }
                        }
                    }
                }
            }
        }
        _ => panic!(
            "usage: representation-evaluator run [development|held-out] | sample SEED CASE BATCH CODEC compatible|native"
        ),
    }
}
