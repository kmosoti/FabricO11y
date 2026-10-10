//! SPOOL-1: a scoped real ENOSPC preserves earlier unACKed custody and
//! records known I/O failure. No physical filesystem exhaustion or double fault.
use fabric_frame::frame::{RECOVERY_REQUIRED, read_frame};
use fabric_o11y::spindle::spool::{Batch, Spool};
use prost::Message;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

const CAPACITY: u64 = 1024 * 1024;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher scratch required"),
        );
        let path = root.join(format!("spool-enospc-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::write(
            path.join("origin.json"),
            json!({"seed":1,"contract":"SPOOL-1 unACKed custody; SPOOL-3 known-failure refusal",
            "origin":"first committed Batch followed by ENOSPC on next append header"})
            .to_string(),
        )
        .unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        } else {
            eprintln!("ENOSPC failure evidence retained: {}", self.0.display());
        }
    }
}

fn source(text: &str) -> Batch {
    Batch {
        version: 1,
        node_id: vec![],
        generation: 0,
        sequence: 0,
        metrics: vec![],
        logs: vec![],
        traces: vec![],
        cursors: vec![],
        collection_gaps: vec![text.into()],
    }
}

#[test]
#[ignore = "owned ENOSPC subprocess helper"]
fn spool_enospc_child() {
    let state = PathBuf::from(std::env::var_os("FABRIC_ENOSPC_STATE").unwrap());
    let inject = std::env::var_os("FABRIC_ENOSPC_EXPECT_FAILURE").is_some();
    let mut spool = Spool::open(&state, CAPACITY).unwrap();
    assert_eq!(spool.acked_through(), 0);
    assert_eq!(spool.next_sequence(), 2);
    let result = spool.append(&source("second independent source"));
    if inject {
        let error = result.unwrap_err();
        fs::write(
            state.join("child-receipt.json"),
            json!({"errno":error.raw_os_error(),
            "kind":format!("{:?}",error.kind()),"next_sequence":spool.next_sequence(),
            "ack_cursor":spool.acked_through()})
            .to_string(),
        )
        .unwrap();
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        assert_eq!(spool.next_sequence(), 2);
        assert_eq!(spool.acked_through(), 0);
        assert!(spool.append(&source("must refuse further writes")).is_err());
        assert!(
            spool.next_unacked().is_err(),
            "known failure must quarantine reads"
        );
    } else {
        assert_eq!(result.unwrap().sequence, 2);
        assert_eq!(spool.next_sequence(), 3);
    }
}

#[test]
fn native_enospc_preserves_unacked_bytes_and_requires_recovery() {
    use std::os::unix::process::ExitStatusExt;
    let scratch = Scratch::new();
    let library = scratch.0.join("fault.so");
    assert!(
        Command::new("gcc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&library)
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tools/bench/labs/completion/io_fault.c")
            )
            .arg("-ldl")
            .status()
            .unwrap()
            .success()
    );
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for inject in [false, true] {
        let label = if inject { "enospc" } else { "no-hit-control" };
        let state = scratch.0.join(label);
        fs::create_dir(&state).unwrap();
        // No Sender or ACK is installed: the retained Batch represents outage
        // custody. Commit and save its independent source before child injection.
        let mut spool = Spool::open(&state, CAPACITY).unwrap();
        let first = spool.append(&source("first independent source")).unwrap();
        assert_eq!(spool.acked_through(), 0);
        drop(spool);
        let path = state.join("batches.faj");
        let prior_bytes = fs::read(&path).unwrap();
        fs::write(
            scratch.0.join(format!("{label}.prior-batch.bin")),
            first.encode_to_vec(),
        )
        .unwrap();
        fs::write(
            scratch.0.join(format!("{label}.prior-file.bin")),
            &prior_bytes,
        )
        .unwrap();
        let stderr = scratch.0.join(format!("{label}.stderr"));
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "spool_enospc_child", "--nocapture"])
            .env("FABRIC_ENOSPC_STATE", &state)
            .env("LD_PRELOAD", &library)
            .env("FABRIC_FAULT_ROOT", &state)
            .env("FABRIC_FAULT_OP", "write")
            .env(
                "FABRIC_FAULT_MATCH",
                if inject {
                    "/batches.faj$"
                } else {
                    "DOES-NOT-EXIST"
                },
            )
            .env_remove("FABRIC_FAULT_PAUSE")
            .env_remove("FABRIC_FAULT_PARTIAL")
            .env_remove("FABRIC_FAULT_KILL")
            .env_remove("FABRIC_FAULT_N")
            .stdout(fs::File::create(scratch.0.join(format!("{label}.stdout"))).unwrap())
            .stderr(fs::File::create(&stderr).unwrap());
        if inject {
            command.env("FABRIC_ENOSPC_EXPECT_FAILURE", "1");
        } else {
            command.env_remove("FABRIC_ENOSPC_EXPECT_FAILURE");
        }
        let mut child = Child(command.spawn().unwrap());
        let began = Instant::now();
        let exit = loop {
            if let Some(exit) = child.0.try_wait().unwrap() {
                break exit;
            }
            assert!(
                began.elapsed() < Duration::from_secs(30),
                "ENOSPC child timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let trace = fs::read_to_string(&stderr).unwrap();
        fs::write(scratch.0.join(format!("{label}.exit.json")),
            json!({"exit":exit.code(),"signal":exit.signal(),"injected":trace.contains("FABRIC_INJECTION")}).to_string()).unwrap();
        assert!(exit.success(), "{label}: {trace}");
        assert_eq!(
            trace.contains("FABRIC_INJECTION"),
            inject,
            "{label}: {trace}"
        );
        let child_receipt = if inject {
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(state.join("child-receipt.json")).unwrap(),
            )
            .unwrap()
        } else {
            json!({"errno":null,"next_sequence":3,"ack_cursor":0})
        };
        if inject {
            assert!(trace.contains("op=write") && trace.contains("/batches.faj"));
            assert_eq!(
                fs::read(&path).unwrap(),
                prior_bytes,
                "ENOSPC erased unACKed custody"
            );
            assert!(state.join(RECOVERY_REQUIRED).exists());
            let mut visible = 0;
            let status = Spool::inspect(&state, CAPACITY, |_| {
                visible += 1;
                Ok(())
            })
            .unwrap();
            assert!(status.recovery_required);
            assert_eq!(status.acked_through, 0);
            assert_eq!(
                visible, 0,
                "known failure must not be served as trustworthy custody"
            );
            assert!(
                Spool::open(&state, CAPACITY).is_err(),
                "removing ENOSPC must not clear known failure"
            );
        } else {
            let mut spool = Spool::open(&state, CAPACITY).unwrap();
            let mut batches = Vec::new();
            spool
                .replay(|batch| {
                    batches.push(batch);
                    Ok(())
                })
                .unwrap();
            assert_eq!(batches.len(), 2);
            assert_eq!(batches[0], first);
            assert_eq!(spool.acked_through(), 0);
        }
        // Forensic read verifies the preserved committed prefix independently
        // of ordinary reopen. It does not clear or bypass production quarantine.
        let file = fs::File::open(&path).unwrap();
        let (raw, end) = read_frame(&file, 0, file.metadata().unwrap().len(), 1024 * 1024)
            .unwrap()
            .unwrap();
        assert_eq!(raw, first.encode_to_vec());
        assert_eq!(end, prior_bytes.len() as u64);
        println!(
            "{}",
            json!({"case":label,"exit":exit.code(),"signal":exit.signal(),
            "injected":trace.contains("FABRIC_INJECTION"),"child":child_receipt,
            "retained_prefix_bytes":end,"known_failure":state.join(RECOVERY_REQUIRED).exists(),
            "scope":"native ENOSPC append; no physical-full-filesystem or double-fault claim"})
        );
    }
}
