//! Numeric graph storage, with IDs sorted once for deterministic ready selection.
use super::{id, require};
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

pub const ITEM_LIMIT: usize = 4096;
const EDGE_LIMIT: usize = 65536;

/// Validate only `id` and `needs`; other job fields belong to composition.
pub fn ordered_jobs(jobs: &[Value]) -> Result<Vec<String>, String> {
    require(!jobs.is_empty() && jobs.len() <= ITEM_LIMIT, "job count")?;
    let mut nodes = Vec::with_capacity(jobs.len());
    for job in jobs {
        let name = job.get("id").and_then(Value::as_str).ok_or("job id")?;
        require(id(name), "job id")?;
        nodes.push((name, job));
    }
    nodes.sort_unstable_by_key(|node| node.0);
    let mut index = HashMap::with_capacity(nodes.len());
    for (position, (name, _)) in nodes.iter().enumerate() {
        require(index.insert(*name, position).is_none(), "duplicate job id")?;
    }
    let mut children = vec![Vec::new(); nodes.len()];
    let mut indegrees = vec![0; nodes.len()];
    let mut edges = 0;
    for (position, (_, job)) in nodes.iter().enumerate() {
        let needs = job
            .get("needs")
            .and_then(Value::as_array)
            .ok_or("needs list")?;
        edges += needs.len();
        require(edges <= EDGE_LIMIT, "edge count")?;
        let mut seen = HashSet::with_capacity(needs.len());
        for parent in needs {
            let parent = parent.as_str().ok_or("dependency id")?;
            let parent = *index.get(parent).ok_or("unknown dependency")?;
            require(
                parent != position && seen.insert(parent),
                "self/duplicate dependency",
            )?;
            children[parent].push(position);
        }
        indegrees[position] = needs.len();
    }
    let mut ready: BinaryHeap<_> = indegrees
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(index, _)| Reverse(index))
        .collect();
    let mut output = Vec::with_capacity(nodes.len());
    while let Some(Reverse(position)) = ready.pop() {
        output.push(nodes[position].0.to_owned());
        for &child in &children[position] {
            indegrees[child] -= 1;
            if indegrees[child] == 0 {
                ready.push(Reverse(child));
            }
        }
    }
    require(output.len() == nodes.len(), "dependency cycle")?;
    Ok(output)
}
