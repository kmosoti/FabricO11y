#![no_main]
use fabric::observation;
use fabric_core::{BlameResource, FsChurnEntry, HostSnapshot, ProcessSample};
use fabric_graph::{CausalGraph, Direction, EdgeKind, NodeKind};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }

    if let Ok(record) = observation::decode_versioned(data) {
        let _ = record.identity();
    }

    let mut graph = CausalGraph::new();
    let mut snapshot = HostSnapshot {
        timestamp_unix_ms: 1_700_000_000_000,
        hostname: "fuzz-node".into(),
        ..Default::default()
    };

    let edge_count = usize::from(data.first().copied().unwrap_or(0) % 48);
    let node_mod = u8::max(data.get(1).copied().unwrap_or(8) % 16, 1);
    let depth = usize::from(data.get(2).copied().unwrap_or(3) % 6);
    let direction = match data.get(3).copied().unwrap_or(0) % 3 {
        0 => Direction::Upstream,
        1 => Direction::Downstream,
        _ => Direction::Both,
    };

    for chunk in data
        .iter()
        .skip(4)
        .take(edge_count * 3)
        .collect::<Vec<_>>()
        .chunks(3)
    {
        let src_id = format!("pid:{}", (chunk[0] % node_mod) + 1);
        let dst_idx = chunk.get(1).copied().copied().unwrap_or(0) % node_mod;
        let dst_id = format!("path:/tmp/fuzz-{dst_idx}");
        let weight = f64::from(chunk.get(2).copied().copied().unwrap_or(1));
        graph.upsert_node(&src_id, NodeKind::Process, &src_id, BTreeMap::new());
        graph.upsert_node(&dst_id, NodeKind::FilesystemPath, &dst_id, BTreeMap::new());
        graph.add_edge(&src_id, &dst_id, EdgeKind::MutatesPath, weight, "fuzz");
    }

    for i in 0..usize::from(node_mod).min(8) {
        snapshot.processes.push(ProcessSample {
            pid: (i as u32) + 1,
            ppid: 1,
            comm: format!("proc-{i}"),
            slice: "agent.slice".into(),
            rss_bytes: (u64::from(*data.get(i).unwrap_or(&1)) + 1) * 4096,
            swap_bytes: u64::from(*data.get(i + 1).unwrap_or(&0)) * 1024,
            ..Default::default()
        });
        snapshot.fs_churn.push(FsChurnEntry {
            path: format!("/tmp/fuzz-{i}"),
            mount_point: "/tmp".into(),
            size_bytes: u64::from(*data.get(i + 2).unwrap_or(&1)) * 1024,
            owner_pid: Some((i as u32) + 1),
            ..Default::default()
        });
    }

    let subgraph = graph.lineage("pid:1", direction, depth);
    assert!(subgraph.nodes.len() <= graph.nodes().len());

    for resource in [
        BlameResource::Swap,
        BlameResource::Memory,
        BlameResource::Tmpfs,
        BlameResource::Cpu,
        BlameResource::Disk("/data".into()),
    ] {
        let report = graph.blame(&resource, &snapshot);
        assert!(report.entries.len() <= 15);
    }
});
