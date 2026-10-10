//! An accepted control publication must stay inside the reopen byte limit.
use fabric_server::control::{
    Control, DesiredConfig, MAX_NODES, NodeRecord, Status, check_desired,
};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

const STATE_CAP: usize = 16 * 1024 * 1024;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::var_os("FABRIC_SCRATCH_ROOT")
            .expect("run tests through the resource launcher");
        let path = PathBuf::from(root).join(format!("control-size-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned control scratch");
    }
}

#[derive(Serialize)]
struct Stored<'a> {
    version: u32,
    nodes: &'a [NodeRecord],
}
fn encode(nodes: &[NodeRecord]) -> Vec<u8> {
    serde_json::to_vec_pretty(&Stored { version: 1, nodes }).unwrap()
}

#[test]
fn an_oversized_inventory_update_preserves_the_reopenable_state() {
    let scratch = Scratch::new();
    // Backslashes are legal path bytes but double in JSON. This valid shape
    // exceeds the persisted byte cap before reaching the node-count ceiling.
    let large = DesiredConfig {
        logs: vec![format!("/{}", "\\".repeat(239)); 16],
        metric_interval_s: 15,
    };
    check_desired(&large).unwrap();
    let mut nodes: Vec<_> = (0..MAX_NODES)
        .map(|index| NodeRecord {
            name: format!("node-{index:04}"),
            token_sha256: format!("{index:064x}"),
            status: Status::Active,
            revision: 1,
            desired: large.clone(),
            enrolled_unix_s: 0,
        })
        .collect();
    let full_size = encode(&nodes).len();
    assert!(full_size > STATE_CAP);
    // Equal-length names, hashes and configurations give equal-sized records.
    // Find the first inventory whose representation exceeds the byte ceiling.
    let record_size = encode(&nodes[..2]).len() - encode(&nodes[..1]).len();
    let count = (STATE_CAP - encode(&[]).len()) / record_size + 2;
    nodes.truncate(count);
    while encode(&nodes[..nodes.len() - 1]).len() > STATE_CAP {
        nodes.pop();
    }
    assert!(encode(&nodes).len() > STATE_CAP);
    let target = nodes.last_mut().unwrap();
    let name = target.name.clone();
    target.desired.logs.clear();
    // The final record's non-log overhead can straddle the boundary; trim a
    // preceding valid configuration until the small candidate fits.
    while encode(&nodes).len() > STATE_CAP {
        let preceding = nodes.len() - 2;
        assert!(nodes[preceding].desired.logs.pop().is_some());
    }
    let before = encode(&nodes);
    assert!(before.len() <= STATE_CAP);
    fs::write(scratch.0.join("control.json"), &before).unwrap();

    let mut control = Control::open(&scratch.0).unwrap();
    let error = control.set_config(&name, large).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(error.to_string(), "control state exceeds its size cap");
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), before);
    assert!(!scratch.0.join("control.json.tmp").exists());
    let current = control.inventory();
    let record = &current.iter().find(|(r, _)| r.name == name).unwrap().0;
    assert_eq!(record.revision, 1);
    assert!(record.desired.logs.is_empty());
    drop(control);
    let mut reopened = Control::open(&scratch.0).unwrap();
    // A smaller accepted update also survives restart with its revision.
    reopened
        .set_config(
            &name,
            DesiredConfig {
                logs: vec![],
                metric_interval_s: 30,
            },
        )
        .unwrap();
    drop(reopened);
    let final_state = Control::open(&scratch.0).unwrap().inventory();
    let record = &final_state.iter().find(|(r, _)| r.name == name).unwrap().0;
    assert_eq!(record.revision, 2);
    assert_eq!(record.desired.metric_interval_s, 30);
}
