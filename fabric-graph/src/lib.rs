//! Sans-I/O causal lineage graph and ranked resource blame engine.

use arrow::array::StringArray;
use arrow::record_batch::RecordBatch;
use fabric_core::arrow_schemas::LINEAGE_NODES_SCHEMA;
use fabric_core::{
    BlameEntry, BlameReport, BlameResource, FsChurnEntry, GraphEdgeRecord, HostSnapshot,
    PillarFinding, ProcessSample,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::str::FromStr;

/// Typed vertex categories inside the causal observability graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Session,
    Agent,
    SystemdUnit,
    Process,
    Cgroup,
    GpuDevice,
    Socket,
    FilesystemPath,
    PodmanVolume,
    Anomaly,
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Session => "session",
            Self::Agent => "agent",
            Self::SystemdUnit => "systemd_unit",
            Self::Process => "process",
            Self::Cgroup => "cgroup",
            Self::GpuDevice => "gpu_device",
            Self::Socket => "socket",
            Self::FilesystemPath => "filesystem_path",
            Self::PodmanVolume => "podman_volume",
            Self::Anomaly => "anomaly",
        };
        f.write_str(s)
    }
}

/// Directed causal relationship edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Spawns,
    RunsIn,
    Controls,
    ContainsPid,
    AllocatesGpu,
    BindsSocket,
    MutatesPath,
    MountsVolume,
    TriggersAnomaly,
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Spawns => "spawns",
            Self::RunsIn => "runs_in",
            Self::Controls => "controls",
            Self::ContainsPid => "contains_pid",
            Self::AllocatesGpu => "allocates_gpu",
            Self::BindsSocket => "binds_socket",
            Self::MutatesPath => "mutates_path",
            Self::MountsVolume => "mounts_volume",
            Self::TriggersAnomaly => "triggers_anomaly",
        };
        f.write_str(s)
    }
}

/// BFS traversal direction for `fabric lineage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upstream,
    Downstream,
    #[default]
    Both,
}

impl FromStr for Direction {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "upstream" | "up" => Ok(Self::Upstream),
            "downstream" | "down" => Ok(Self::Downstream),
            "both" | "all" => Ok(Self::Both),
            other => Err(format!(
                "invalid lineage direction '{other}' (expected upstream, downstream, or both)"
            )),
        }
    }
}

/// Vertex stored in `CausalGraph`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub metadata: BTreeMap<String, String>,
}

/// Directed edge stored in `CausalGraph`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEdge {
    pub src: String,
    pub dst: String,
    pub kind: EdgeKind,
    pub weight: f64,
    pub note: String,
}

/// Result returned by `CausalGraph::lineage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LineageSubgraph {
    pub target: String,
    pub matched_roots: Vec<String>,
    pub direction: Direction,
    pub max_depth: usize,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Converts a `LineageSubgraph` into an Arrow `RecordBatch` of nodes.
pub fn lineage_to_record_batch(subgraph: &LineageSubgraph) -> Result<RecordBatch, String> {
    let mut kinds = Vec::with_capacity(subgraph.nodes.len());
    let mut meta = Vec::with_capacity(subgraph.nodes.len());
    for node in &subgraph.nodes {
        kinds.push(node.kind.to_string());
        meta.push(serde_json::to_string(&node.metadata).unwrap_or_else(|_| "{}".into()));
    }
    RecordBatch::try_new(
        LINEAGE_NODES_SCHEMA.clone(),
        vec![
            std::sync::Arc::new(StringArray::from_iter_values(
                subgraph.nodes.iter().map(|n| n.id.as_str()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                kinds.iter().map(String::as_str),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                subgraph.nodes.iter().map(|n| n.label.as_str()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                meta.iter().map(String::as_str),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Deterministic causal observability graph built from snapshots and findings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CausalGraph {
    nodes: BTreeMap<String, GraphNode>,
    edges: Vec<GraphEdge>,
    edge_keys: BTreeSet<(String, String, EdgeKind)>,
    adj_out: BTreeMap<String, Vec<usize>>,
    adj_in: BTreeMap<String, Vec<usize>>,
}

impl CausalGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_node(
        &mut self,
        id: impl Into<String>,
        kind: NodeKind,
        label: impl Into<String>,
        metadata: BTreeMap<String, String>,
    ) {
        let id = id.into();
        let label = label.into();
        self.nodes
            .entry(id.clone())
            .and_modify(|existing| {
                existing.kind = kind;
                if !label.is_empty() {
                    existing.label = label.clone();
                }
                for (k, v) in &metadata {
                    existing.metadata.insert(k.clone(), v.clone());
                }
            })
            .or_insert_with(|| GraphNode {
                id,
                kind,
                label,
                metadata,
            });
    }

    pub fn add_edge(
        &mut self,
        src: impl Into<String>,
        dst: impl Into<String>,
        kind: EdgeKind,
        weight: f64,
        note: impl Into<String>,
    ) {
        let src = src.into();
        let dst = dst.into();
        let key = (src.clone(), dst.clone(), kind);
        if self.edge_keys.contains(&key) {
            return;
        }
        self.edge_keys.insert(key);
        let idx = self.edges.len();
        self.edges.push(GraphEdge {
            src: src.clone(),
            dst: dst.clone(),
            kind,
            weight,
            note: note.into(),
        });
        self.adj_out.entry(src).or_default().push(idx);
        self.adj_in.entry(dst).or_default().push(idx);
    }

    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<String, GraphNode> {
        &self.nodes
    }

    #[must_use]
    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    /// Exports all edges into `GraphEdgeRecord` rows suitable for OLAP persistence.
    #[must_use]
    pub fn to_edge_records(&self, timestamp_ms: i64) -> Vec<GraphEdgeRecord> {
        self.edges
            .iter()
            .map(|e| GraphEdgeRecord {
                timestamp_ms,
                src_id: e.src.clone(),
                src_kind: self
                    .nodes
                    .get(&e.src)
                    .map_or_else(|| "unknown".into(), |n| n.kind.to_string()),
                edge_kind: e.kind.to_string(),
                dst_id: e.dst.clone(),
                dst_kind: self
                    .nodes
                    .get(&e.dst)
                    .map_or_else(|| "unknown".into(), |n| n.kind.to_string()),
                weight: e.weight,
                note: e.note.clone(),
            })
            .collect()
    }

    /// Deterministically builds a full causal graph connecting sessions, agents,
    /// slices, units, processes, GPU devices, mounts/paths, and pillar anomalies.
    #[must_use]
    pub fn from_snapshot(snapshot: &HostSnapshot, findings: &[PillarFinding]) -> Self {
        let mut graph = Self::new();
        let host_id = format!("session:{}", snapshot.hostname);
        graph.upsert_node(
            &host_id,
            NodeKind::Session,
            &snapshot.hostname,
            BTreeMap::from([(
                "timestamp_ms".into(),
                snapshot.timestamp_unix_ms.to_string(),
            )]),
        );

        for slice in &snapshot.slices {
            let slice_id = format!("slice:{}", slice.slice_name);
            graph.upsert_node(
                &slice_id,
                NodeKind::Cgroup,
                &slice.slice_name,
                BTreeMap::from([
                    ("cgroup_path".into(), slice.cgroup_path.clone()),
                    (
                        "memory_current_bytes".into(),
                        slice.memory_current_bytes.to_string(),
                    ),
                ]),
            );
            graph.add_edge(
                &host_id,
                &slice_id,
                EdgeKind::Controls,
                slice.memory_current_bytes as f64,
                "slice",
            );
        }

        for unit in &snapshot.units {
            let slice_id = format!("slice:{}", unit.slice);
            graph.upsert_node(&slice_id, NodeKind::Cgroup, &unit.slice, BTreeMap::new());
            let unit_id = format!("unit:{}", unit.name);
            let mut meta = BTreeMap::from([
                ("active_state".into(), unit.active_state.clone()),
                ("sub_state".into(), unit.sub_state.clone()),
                ("transient".into(), unit.transient.to_string()),
                (
                    "collect_mode_enabled".into(),
                    unit.collect_mode_enabled.to_string(),
                ),
            ]);
            if let Some(reason) = &unit.orphaned_reason {
                meta.insert("orphaned_reason".into(), reason.clone());
            }
            graph.upsert_node(&unit_id, NodeKind::SystemdUnit, &unit.name, meta);
            graph.add_edge(
                &slice_id,
                &unit_id,
                EdgeKind::Controls,
                unit.memory_current_bytes as f64,
                "unit",
            );
        }

        for agent in &snapshot.agents {
            let agent_id = format!("agent:{}", agent.session_id);
            graph.upsert_node(
                &agent_id,
                NodeKind::Agent,
                &agent.agent_name,
                BTreeMap::from([
                    ("slice".into(), agent.slice.clone()),
                    ("rss_bytes".into(), agent.rss_bytes.to_string()),
                ]),
            );
            graph.add_edge(&host_id, &agent_id, EdgeKind::Spawns, 1.0, "agent_session");
            if let Some(unit) = &agent.unit {
                let unit_id = format!("unit:{unit}");
                graph.upsert_node(&unit_id, NodeKind::SystemdUnit, unit, BTreeMap::new());
                graph.add_edge(
                    &agent_id,
                    &unit_id,
                    EdgeKind::RunsIn,
                    agent.rss_bytes as f64,
                    "agent_unit",
                );
            } else {
                let slice_id = format!("slice:{}", agent.slice);
                graph.add_edge(
                    &agent_id,
                    &slice_id,
                    EdgeKind::RunsIn,
                    agent.rss_bytes as f64,
                    "agent_slice",
                );
            }
        }

        for proc_sample in &snapshot.processes {
            let pid_id = format!("pid:{}", proc_sample.pid);
            graph.upsert_node(
                &pid_id,
                NodeKind::Process,
                format!("{}({})", proc_sample.comm, proc_sample.pid),
                BTreeMap::from([
                    ("comm".into(), proc_sample.comm.clone()),
                    ("slice".into(), proc_sample.slice.clone()),
                    ("rss_bytes".into(), proc_sample.rss_bytes.to_string()),
                    ("swap_bytes".into(), proc_sample.swap_bytes.to_string()),
                    ("cgroup_path".into(), proc_sample.cgroup_path.clone()),
                ]),
            );
            if let Some(unit) = &proc_sample.unit {
                let unit_id = format!("unit:{unit}");
                graph.upsert_node(&unit_id, NodeKind::SystemdUnit, unit, BTreeMap::new());
                graph.add_edge(
                    &unit_id,
                    &pid_id,
                    EdgeKind::ContainsPid,
                    proc_sample.rss_bytes as f64,
                    "unit_pid",
                );
            } else {
                let slice_id = format!("slice:{}", proc_sample.slice);
                graph.upsert_node(
                    &slice_id,
                    NodeKind::Cgroup,
                    &proc_sample.slice,
                    BTreeMap::new(),
                );
                graph.add_edge(
                    &slice_id,
                    &pid_id,
                    EdgeKind::ContainsPid,
                    proc_sample.rss_bytes as f64,
                    "slice_pid",
                );
            }
            if let Some(agent_hint) = &proc_sample.agent_hint {
                let agent_id = format!("agent:{agent_hint}:{}", proc_sample.pid);
                graph.upsert_node(
                    &agent_id,
                    NodeKind::Agent,
                    agent_hint,
                    BTreeMap::from([("pid".into(), proc_sample.pid.to_string())]),
                );
                graph.add_edge(
                    &agent_id,
                    &pid_id,
                    EdgeKind::Spawns,
                    proc_sample.rss_bytes as f64,
                    "agent_process",
                );
            }
            for open_path in &proc_sample.open_paths {
                let path_id = format!("path:{open_path}");
                graph.upsert_node(
                    &path_id,
                    NodeKind::FilesystemPath,
                    open_path,
                    BTreeMap::new(),
                );
                graph.add_edge(&pid_id, &path_id, EdgeKind::MutatesPath, 1.0, "open_fd");
            }
        }

        for entry in &snapshot.fs_churn {
            let path_id = format!("path:{}", entry.path);
            let kind = if entry.path.contains("containers/storage/volumes")
                || entry.path.contains("libpod")
            {
                NodeKind::PodmanVolume
            } else {
                NodeKind::FilesystemPath
            };
            let mut meta = BTreeMap::from([
                ("mount_point".into(), entry.mount_point.clone()),
                ("size_bytes".into(), entry.size_bytes.to_string()),
                ("file_count".into(), entry.file_count.to_string()),
            ]);
            if let Some(unit) = &entry.owner_unit {
                meta.insert("owner_unit".into(), unit.clone());
            }
            if let Some(slice) = &entry.owner_slice {
                meta.insert("owner_slice".into(), slice.clone());
            }
            let edge_kind = if kind == NodeKind::PodmanVolume {
                EdgeKind::MountsVolume
            } else {
                EdgeKind::MutatesPath
            };
            graph.upsert_node(&path_id, kind, &entry.path, meta);
            if let Some(pid) = entry.owner_pid {
                let pid_id = format!("pid:{pid}");
                graph.upsert_node(
                    &pid_id,
                    NodeKind::Process,
                    format!("pid:{pid}"),
                    BTreeMap::new(),
                );
                graph.add_edge(
                    &pid_id,
                    &path_id,
                    edge_kind,
                    entry.size_bytes as f64,
                    "fs_owner",
                );
            } else if let Some(unit) = &entry.owner_unit {
                let unit_id = format!("unit:{unit}");
                graph.add_edge(
                    &unit_id,
                    &path_id,
                    edge_kind,
                    entry.size_bytes as f64,
                    "fs_unit",
                );
            }
        }

        for gpu in &snapshot.gpus {
            let gpu_id = format!("gpu:{}", gpu.card);
            graph.upsert_node(
                &gpu_id,
                NodeKind::GpuDevice,
                format!("{} ({})", gpu.card, gpu.vendor),
                BTreeMap::from([("vendor".into(), gpu.vendor.clone())]),
            );
            graph.add_edge(&host_id, &gpu_id, EdgeKind::AllocatesGpu, 1.0, "host_gpu");
        }

        for finding in findings {
            let anom_id = format!("anomaly:{}:{}", finding.pillar, finding.target);
            graph.upsert_node(
                &anom_id,
                NodeKind::Anomaly,
                &finding.summary,
                BTreeMap::from([
                    ("pillar".into(), finding.pillar.clone()),
                    ("severity".into(), finding.severity.clone()),
                ]),
            );
            if self_or_matching_node(&graph.nodes, &finding.target) {
                graph.add_edge(
                    &finding.target,
                    &anom_id,
                    EdgeKind::TriggersAnomaly,
                    finding.metric_value,
                    &finding.pillar,
                );
            } else {
                graph.add_edge(
                    &host_id,
                    &anom_id,
                    EdgeKind::TriggersAnomaly,
                    finding.metric_value,
                    &finding.pillar,
                );
            }
        }

        graph
    }

    /// Bounded BFS lineage traversal supporting exact node IDs, bare names, and path prefixes.
    #[must_use]
    pub fn lineage(&self, target: &str, direction: Direction, max_depth: usize) -> LineageSubgraph {
        let roots = self.resolve_targets(target);
        if roots.is_empty() {
            return LineageSubgraph {
                target: target.to_owned(),
                matched_roots: Vec::new(),
                direction,
                max_depth,
                nodes: Vec::new(),
                edges: Vec::new(),
            };
        }

        let mut visited_nodes: BTreeSet<String> = BTreeSet::new();
        let mut collected_edge_idxs: BTreeSet<usize> = BTreeSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        for root in &roots {
            if visited_nodes.insert(root.clone()) {
                queue.push_back((root.clone(), 0));
            }
        }

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            if matches!(direction, Direction::Upstream | Direction::Both)
                && let Some(in_edges) = self.adj_in.get(&node_id)
            {
                for &edge_idx in in_edges {
                    collected_edge_idxs.insert(edge_idx);
                    let src = &self.edges[edge_idx].src;
                    if visited_nodes.insert(src.clone()) {
                        queue.push_back((src.clone(), depth + 1));
                    }
                }
            }
            if matches!(direction, Direction::Downstream | Direction::Both)
                && let Some(out_edges) = self.adj_out.get(&node_id)
            {
                for &edge_idx in out_edges {
                    collected_edge_idxs.insert(edge_idx);
                    let dst = &self.edges[edge_idx].dst;
                    if visited_nodes.insert(dst.clone()) {
                        queue.push_back((dst.clone(), depth + 1));
                    }
                }
            }
        }

        let nodes = visited_nodes
            .into_iter()
            .filter_map(|id| self.nodes.get(&id).cloned())
            .collect();
        let edges = collected_edge_idxs
            .into_iter()
            .filter_map(|idx| self.edges.get(idx).cloned())
            .collect();

        LineageSubgraph {
            target: target.to_owned(),
            matched_roots: roots,
            direction,
            max_depth,
            nodes,
            edges,
        }
    }

    /// Computes ranked blame attribution for swap, memory, tmpfs, disk mount, or CPU contention.
    #[must_use]
    pub fn blame(&self, resource: &BlameResource, snapshot: &HostSnapshot) -> BlameReport {
        match resource {
            BlameResource::Memory => self.blame_memory(snapshot),
            BlameResource::Swap => self.blame_swap(snapshot),
            BlameResource::Tmpfs => self.blame_fs_mount("/tmp", "tmpfs", snapshot),
            BlameResource::Disk(mount) => {
                self.blame_fs_mount(mount, &format!("disk:{mount}"), snapshot)
            }
            BlameResource::Cpu => self.blame_cpu(snapshot),
        }
    }

    fn resolve_targets(&self, target: &str) -> Vec<String> {
        let trimmed = target.trim();
        if self.nodes.contains_key(trimmed) {
            return vec![trimmed.to_owned()];
        }
        let mut matched = Vec::new();
        let candidates = [
            format!("path:{trimmed}"),
            format!("unit:{trimmed}"),
            format!("slice:{trimmed}"),
            format!("pid:{trimmed}"),
            format!("agent:{trimmed}"),
            format!("gpu:{trimmed}"),
        ];
        for cand in candidates {
            if self.nodes.contains_key(&cand) {
                matched.push(cand);
            }
        }
        if !matched.is_empty() {
            return matched;
        }
        // Prefix / child / parent matching for filesystem paths and unit substrings.
        let normalized_trimmed = trimmed.trim_end_matches('/');
        for (id, node) in &self.nodes {
            if let Some(path_part) = id.strip_prefix("path:") {
                let clean = path_part.trim_end_matches('/');
                if (!normalized_trimmed.is_empty()
                    && (clean.starts_with(&format!("{normalized_trimmed}/"))
                        || normalized_trimmed.starts_with(&format!("{clean}/"))))
                    || clean == normalized_trimmed
                {
                    matched.push(id.clone());
                }
            } else if node.label == trimmed || id.ends_with(&format!(":{trimmed}")) {
                matched.push(id.clone());
            }
        }
        matched
    }

    /// Returns ranked multi-parent upstream causal paths ending at `node_id`,
    /// ordered from root to `node_id` with the highest-ranked primary path first.
    #[must_use]
    pub fn chain_for_node(&self, node_id: &str) -> Vec<Vec<String>> {
        const MAX_CHAIN_DEPTH: usize = 8;
        const MAX_CAUSAL_PATHS: usize = 8;
        const MAX_CANDIDATES: usize = 32;

        fn edge_priority(kind: EdgeKind) -> u8 {
            match kind {
                EdgeKind::Spawns => 9,
                EdgeKind::RunsIn => 8,
                EdgeKind::MutatesPath => 7,
                EdgeKind::AllocatesGpu => 6,
                EdgeKind::BindsSocket => 5,
                EdgeKind::ContainsPid => 4,
                EdgeKind::Controls => 3,
                EdgeKind::MountsVolume => 2,
                EdgeKind::TriggersAnomaly => 1,
            }
        }

        let mut raw_paths: Vec<(f64, Vec<String>)> = Vec::new();
        let mut stack: Vec<(String, Vec<String>, f64, BTreeSet<String>)> = vec![(
            node_id.to_owned(),
            vec![node_id.to_owned()],
            0.0,
            BTreeSet::from([node_id.to_owned()]),
        )];

        while let Some((cur, path, weight, seen)) = stack.pop() {
            if raw_paths.len() >= MAX_CANDIDATES {
                break;
            }
            let mut next_edges: Vec<&GraphEdge> = self
                .adj_in
                .get(&cur)
                .into_iter()
                .flat_map(|idxs| idxs.iter().map(|&idx| &self.edges[idx]))
                .filter(|edge| !seen.contains(&edge.src))
                .collect();

            if next_edges.is_empty() || path.len() >= MAX_CHAIN_DEPTH {
                let mut forward = path;
                forward.reverse();
                raw_paths.push((weight, forward));
                continue;
            }

            next_edges.sort_by(|a, b| {
                b.weight
                    .total_cmp(&a.weight)
                    .then_with(|| edge_priority(b.kind).cmp(&edge_priority(a.kind)))
                    .then_with(|| a.src.cmp(&b.src))
            });

            // Push in reverse so the highest-ranked parent is explored first.
            for edge in next_edges.into_iter().rev() {
                let mut next_path = path.clone();
                next_path.push(edge.src.clone());
                let mut next_seen = seen.clone();
                next_seen.insert(edge.src.clone());
                let edge_w = if edge.weight.is_finite() {
                    edge.weight.max(0.0)
                } else {
                    0.0
                };
                stack.push((edge.src.clone(), next_path, weight + edge_w, next_seen));
            }
        }

        raw_paths.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| b.1.len().cmp(&a.1.len()))
                .then_with(|| a.1.cmp(&b.1))
        });

        let mut deduped = Vec::new();
        let mut seen_paths = BTreeSet::new();
        for (_, path) in raw_paths {
            if seen_paths.insert(path.clone()) {
                deduped.push(path);
                if deduped.len() >= MAX_CAUSAL_PATHS {
                    break;
                }
            }
        }
        if deduped.is_empty() {
            deduped.push(vec![node_id.to_owned()]);
        }
        deduped
    }

    fn primary_chain_for_node(&self, node_id: &str) -> Vec<String> {
        self.chain_for_node(node_id)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    fn blame_memory(&self, snapshot: &HostSnapshot) -> BlameReport {
        let mut items: Vec<(u64, &ProcessSample)> = snapshot
            .processes
            .iter()
            .filter(|p| p.rss_bytes > 0)
            .map(|p| (p.rss_bytes, p))
            .collect();
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.pid.cmp(&b.1.pid)));

        let total_rss: u64 = snapshot
            .memory
            .mem_total_bytes
            .saturating_sub(snapshot.memory.mem_available_bytes)
            .max(items.iter().map(|i| i.0).sum());

        let entries = items
            .into_iter()
            .take(15)
            .enumerate()
            .map(|(idx, (bytes, proc_sample))| {
                let node_id = format!("pid:{}", proc_sample.pid);
                BlameEntry {
                    rank: (idx + 1) as u32,
                    node_id: node_id.clone(),
                    label: format!("{} (pid {})", proc_sample.comm, proc_sample.pid),
                    contribution_value: bytes,
                    unit_label: "bytes".into(),
                    share_pct: pct(bytes, total_rss),
                    pid: Some(proc_sample.pid),
                    comm: Some(proc_sample.comm.clone()),
                    unit: proc_sample.unit.clone(),
                    slice: proc_sample.slice.clone(),
                    agent: proc_sample.agent_hint.clone(),
                    target_path: proc_sample.open_paths.first().cloned(),
                    lineage_chain: self.primary_chain_for_node(&node_id),
                }
            })
            .collect();

        BlameReport {
            resource: "memory".into(),
            total_observed: total_rss,
            unit_label: "bytes".into(),
            pressure_summary: format!(
                "available={} MiB mem.some.avg10={:.2}",
                snapshot.memory.mem_available_bytes / (1024 * 1024),
                snapshot.psi.memory_some.avg10
            ),
            entries,
        }
    }

    fn blame_swap(&self, snapshot: &HostSnapshot) -> BlameReport {
        let mut entries_raw: Vec<BlameEntry> = snapshot
            .processes
            .iter()
            .filter(|p| p.swap_bytes > 0)
            .map(|p| {
                let node_id = format!("pid:{}", p.pid);
                BlameEntry {
                    rank: 0,
                    node_id: node_id.clone(),
                    label: format!("{} (pid {} swap)", p.comm, p.pid),
                    contribution_value: p.swap_bytes,
                    unit_label: "bytes".into(),
                    share_pct: 0.0,
                    pid: Some(p.pid),
                    comm: Some(p.comm.clone()),
                    unit: p.unit.clone(),
                    slice: p.slice.clone(),
                    agent: p.agent_hint.clone(),
                    target_path: p.open_paths.first().cloned(),
                    lineage_chain: self.primary_chain_for_node(&node_id),
                }
            })
            .collect();

        // /tmp tmpfs allocations push cold anonymous memory into zram0 swap.
        for fs in snapshot.fs_churn.iter().filter(|f| f.mount_point == "/tmp") {
            let node_id = format!("path:{}", fs.path);
            entries_raw.push(BlameEntry {
                rank: 0,
                node_id: node_id.clone(),
                label: format!("tmpfs:{}", fs.path),
                contribution_value: fs.size_bytes,
                unit_label: "bytes".into(),
                share_pct: 0.0,
                pid: fs.owner_pid,
                comm: fs.owner_comm.clone(),
                unit: fs.owner_unit.clone(),
                slice: fs.owner_slice.clone().unwrap_or_else(|| "unknown".into()),
                agent: fs.owner_agent.clone(),
                target_path: Some(fs.path.clone()),
                lineage_chain: self.primary_chain_for_node(&node_id),
            });
        }

        entries_raw.sort_by(|a, b| {
            b.contribution_value
                .cmp(&a.contribution_value)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });

        let total_swap = snapshot
            .memory
            .swap_used_bytes
            .max(entries_raw.iter().map(|e| e.contribution_value).sum());
        let entries = finalize_blame_ranks(entries_raw, total_swap, 15);

        BlameReport {
            resource: "swap".into(),
            total_observed: total_swap,
            unit_label: "bytes".into(),
            pressure_summary: format!(
                "swap_used={} MiB / {} MiB",
                snapshot.memory.swap_used_bytes / (1024 * 1024),
                snapshot.memory.swap_total_bytes / (1024 * 1024)
            ),
            entries,
        }
    }

    fn blame_fs_mount(
        &self,
        mount: &str,
        resource_name: &str,
        snapshot: &HostSnapshot,
    ) -> BlameReport {
        let mut entries_raw: Vec<BlameEntry> = snapshot
            .fs_churn
            .iter()
            .filter(|f| path_matches_mount(f, mount))
            .map(|f| {
                let node_id = format!("path:{}", f.path);
                BlameEntry {
                    rank: 0,
                    node_id: node_id.clone(),
                    label: f.path.clone(),
                    contribution_value: f.size_bytes,
                    unit_label: "bytes".into(),
                    share_pct: 0.0,
                    pid: f.owner_pid,
                    comm: f.owner_comm.clone(),
                    unit: f.owner_unit.clone(),
                    slice: f.owner_slice.clone().unwrap_or_else(|| "unknown".into()),
                    agent: f.owner_agent.clone(),
                    target_path: Some(f.path.clone()),
                    lineage_chain: self.primary_chain_for_node(&node_id),
                }
            })
            .collect();

        entries_raw.sort_by(|a, b| {
            b.contribution_value
                .cmp(&a.contribution_value)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });

        let mount_used = snapshot
            .mounts
            .iter()
            .find(|m| m.mount_point == mount)
            .map(|m| m.used_bytes)
            .unwrap_or(0);
        let sum_entries: u64 = entries_raw.iter().map(|e| e.contribution_value).sum();
        let denominator = sum_entries.max(mount_used).max(1);
        let entries = finalize_blame_ranks(entries_raw, denominator, 15);

        BlameReport {
            resource: resource_name.to_owned(),
            total_observed: sum_entries,
            unit_label: "bytes".into(),
            pressure_summary: format!(
                "mount={mount} mount_used={} MiB",
                mount_used / (1024 * 1024)
            ),
            entries,
        }
    }

    fn blame_cpu(&self, snapshot: &HostSnapshot) -> BlameReport {
        let mut entries_raw: Vec<BlameEntry> = snapshot
            .processes
            .iter()
            .filter(|p| p.cpu_time_ticks > 0)
            .map(|p| {
                let node_id = format!("pid:{}", p.pid);
                BlameEntry {
                    rank: 0,
                    node_id: node_id.clone(),
                    label: format!("{} (pid {})", p.comm, p.pid),
                    contribution_value: p.cpu_time_ticks,
                    unit_label: "ticks".into(),
                    share_pct: 0.0,
                    pid: Some(p.pid),
                    comm: Some(p.comm.clone()),
                    unit: p.unit.clone(),
                    slice: p.slice.clone(),
                    agent: p.agent_hint.clone(),
                    target_path: p.cwd.clone(),
                    lineage_chain: self.primary_chain_for_node(&node_id),
                }
            })
            .collect();

        entries_raw.sort_by(|a, b| {
            b.contribution_value
                .cmp(&a.contribution_value)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });

        let total_ticks: u64 = entries_raw.iter().map(|e| e.contribution_value).sum();
        let entries = finalize_blame_ranks(entries_raw, total_ticks.max(1), 15);

        BlameReport {
            resource: "cpu".into(),
            total_observed: total_ticks,
            unit_label: "ticks".into(),
            pressure_summary: format!("cpu.some.avg10={:.2}", snapshot.psi.cpu_some.avg10),
            entries,
        }
    }
}

fn self_or_matching_node(nodes: &BTreeMap<String, GraphNode>, id: &str) -> bool {
    nodes.contains_key(id)
}

fn path_matches_mount(entry: &FsChurnEntry, mount: &str) -> bool {
    if entry.mount_point == mount {
        return true;
    }
    if mount == "/" {
        return entry.mount_point == "/";
    }
    entry.path == mount || entry.path.starts_with(&format!("{mount}/"))
}

fn finalize_blame_ranks(
    entries: Vec<BlameEntry>,
    denominator: u64,
    limit: usize,
) -> Vec<BlameEntry> {
    entries
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(idx, mut e)| {
            e.rank = (idx + 1) as u32;
            e.share_pct = pct(e.contribution_value, denominator);
            e
        })
        .collect()
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((part as f64) / (total as f64)) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::{MemorySnapshot, MountUsage};
    use proptest::prelude::*;

    fn sample_snapshot() -> HostSnapshot {
        HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_000,
            hostname: "workstation-01".into(),
            memory: MemorySnapshot {
                mem_total_bytes: 32 * 1024 * 1024 * 1024,
                mem_available_bytes: 16 * 1024 * 1024 * 1024,
                swap_total_bytes: 8 * 1024 * 1024 * 1024,
                swap_used_bytes: 512 * 1024 * 1024,
                zram: None,
            },
            mounts: vec![MountUsage {
                mount_point: "/data".into(),
                fs_type: "btrfs".into(),
                total_bytes: 500 * 1024 * 1024 * 1024,
                available_bytes: 300 * 1024 * 1024 * 1024,
                used_bytes: 200 * 1024 * 1024 * 1024,
                used_pct: 40.0,
            }],
            processes: vec![ProcessSample {
                pid: 4242,
                ppid: 1,
                comm: "synthetic_agent".into(),
                cmdline: "synthetic_agent_load".into(),
                unit: Some("fabric-synth-load-test.scope".into()),
                slice: "agent.slice".into(),
                cgroup_path: "/sys/fs/cgroup/user.slice/agent.slice/fabric-synth-load-test.scope"
                    .into(),
                rss_bytes: 512 * 1024 * 1024,
                swap_bytes: 0,
                cpu_time_ticks: 1200,
                open_fds: 8,
                unlinked_fds: 0,
                cwd: Some("/home/user".into()),
                open_paths: vec!["/data/tmp/fabric-synth-load/payload.bin".into()],
                agent_hint: Some("synthetic_agent_load".into()),
            }],
            fs_churn: vec![FsChurnEntry {
                path: "/data/tmp/fabric-synth-load".into(),
                mount_point: "/data".into(),
                size_bytes: 200 * 1024 * 1024,
                recent_delta_bytes: 200 * 1024 * 1024,
                file_count: 1,
                unlinked_open_fds: 0,
                mtime_unix_ms: 1_700_000_000_000,
                owner_pid: Some(4242),
                owner_comm: Some("synthetic_agent".into()),
                owner_unit: Some("fabric-synth-load-test.scope".into()),
                owner_slice: Some("agent.slice".into()),
                owner_agent: Some("synthetic_agent_load".into()),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn builds_lineage_and_ranks_blame_across_resources() {
        let snap = sample_snapshot();
        let graph = CausalGraph::from_snapshot(&snap, &[]);

        let lineage = graph.lineage("/data/tmp/fabric-synth-load", Direction::Both, 4);
        assert!(lineage.nodes.iter().any(|n| n.id == "pid:4242"));
        assert!(
            lineage
                .nodes
                .iter()
                .any(|n| n.id == "unit:fabric-synth-load-test.scope")
        );

        let mem = graph.blame(&BlameResource::Memory, &snap);
        assert_eq!(mem.entries[0].pid, Some(4242));
        assert_eq!(mem.entries[0].slice, "agent.slice");

        let disk = graph.blame(&BlameResource::Disk("/data".into()), &snap);
        assert_eq!(
            disk.entries[0].target_path.as_deref(),
            Some("/data/tmp/fabric-synth-load")
        );
        assert_eq!(
            disk.entries[0].unit.as_deref(),
            Some("fabric-synth-load-test.scope")
        );
    }

    #[test]
    fn chain_for_node_returns_multi_parent_ranked_paths_with_primary_first() {
        let mut graph = CausalGraph::new();
        for (id, kind) in [
            ("session:h", NodeKind::Session),
            ("agent:high", NodeKind::Agent),
            ("slice:low", NodeKind::Cgroup),
            ("unit:svc", NodeKind::SystemdUnit),
            ("pid:99", NodeKind::Process),
        ] {
            graph.upsert_node(id, kind, id, BTreeMap::new());
        }
        graph.add_edge("session:h", "agent:high", EdgeKind::Spawns, 50.0, "agent");
        graph.add_edge("agent:high", "unit:svc", EdgeKind::RunsIn, 100.0, "high");
        graph.add_edge("session:h", "slice:low", EdgeKind::Controls, 10.0, "slice");
        graph.add_edge("slice:low", "unit:svc", EdgeKind::Controls, 20.0, "low");
        graph.add_edge("unit:svc", "pid:99", EdgeKind::ContainsPid, 40.0, "pid");

        let paths = graph.chain_for_node("pid:99");
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0],
            vec!["session:h", "agent:high", "unit:svc", "pid:99"]
        );
        assert_eq!(
            paths[1],
            vec!["session:h", "slice:low", "unit:svc", "pid:99"]
        );
        assert_eq!(graph.primary_chain_for_node("pid:99"), paths[0]);
    }

    proptest! {
        #[test]
        fn proptest_bfs_handles_cycles_and_respects_depth(edge_count in 1usize..20, depth in 0usize..5) {
            let mut graph = CausalGraph::new();
            for i in 0..=edge_count {
                graph.upsert_node(format!("n:{i}"), NodeKind::Process, format!("n{i}"), BTreeMap::new());
            }
            for i in 0..edge_count {
                graph.add_edge(format!("n:{i}"), format!("n:{}", (i + 1) % (edge_count + 1)), EdgeKind::Spawns, 1.0, "cycle");
            }
            let sub = graph.lineage("n:0", Direction::Downstream, depth);
            prop_assert!(sub.nodes.len() <= depth + 1);
        }
    }
}
