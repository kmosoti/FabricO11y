//! The hexagonal dependency rule, checked against Cargo metadata.
//!
//! Every workspace member is assigned one layer in the policy. A dependency
//! from one member to another is allowed only when the policy lists the
//! target's layer for the source's layer. Every declared dependency counts:
//! normal, build and development; optional or not; for any target platform;
//! under any rename. With resolved metadata the check also follows paths that
//! leave the workspace and come back, such as `core -> helper -> adapter`.

use crate::Violation;
use crate::metadata::{DepKind, Metadata};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const UNASSIGNED_CRATE: &str = "LAYER_UNASSIGNED_CRATE";
pub const STALE_POLICY_ENTRY: &str = "LAYER_STALE_POLICY_ENTRY";
pub const FORBIDDEN_EDGE: &str = "LAYER_FORBIDDEN_EDGE";
pub const FORBIDDEN_TRANSITIVE_EDGE: &str = "LAYER_FORBIDDEN_TRANSITIVE_EDGE";

#[derive(Debug, Clone)]
pub struct Policy {
    /// layer -> layers it may depend on.
    pub layers: BTreeMap<String, BTreeSet<String>>,
    /// crate name -> layer.
    pub crates: BTreeMap<String, String>,
    /// `(from crate, to crate)` edges allowed despite the layer rule.
    pub exceptions: BTreeSet<(String, String)>,
}

impl Policy {
    pub fn parse(json: &Value) -> Result<Self, String> {
        let object = |key: &str| {
            json.get(key)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("layer policy: missing object `{key}`"))
        };
        let mut layers = BTreeMap::new();
        for (layer, allowed) in object("layers")? {
            let allowed = allowed
                .get("may_depend_on")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("layer policy: `{layer}` lacks may_depend_on"))?;
            layers.insert(
                layer.clone(),
                allowed
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            );
        }
        let mut crates = BTreeMap::new();
        for (name, layer) in object("crates")? {
            let layer = layer
                .as_str()
                .ok_or_else(|| format!("layer policy: layer of `{name}` is not a string"))?;
            if !layers.contains_key(layer) {
                return Err(format!(
                    "layer policy: `{name}` uses unknown layer `{layer}`"
                ));
            }
            crates.insert(name.clone(), layer.to_owned());
        }
        let mut exceptions = BTreeSet::new();
        for exception in json
            .get("exceptions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let field = |key: &str| {
                exception
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("layer policy: exception without `{key}`"))
            };
            // An exception without a reason is not documented, so refuse it.
            field("reason")?;
            exceptions.insert((field("from")?.to_owned(), field("to")?.to_owned()));
        }
        Ok(Self {
            layers,
            crates,
            exceptions,
        })
    }

    fn allows(&self, from: &str, to: &str) -> bool {
        if self.exceptions.contains(&(from.to_owned(), to.to_owned())) {
            return true;
        }
        match (self.crates.get(from), self.crates.get(to)) {
            (Some(from_layer), Some(to_layer)) => self
                .layers
                .get(from_layer)
                .is_some_and(|allowed| allowed.contains(to_layer)),
            _ => false,
        }
    }

    fn describe(&self, name: &str) -> String {
        format!(
            "{name} ({})",
            self.crates.get(name).map_or("unassigned", String::as_str)
        )
    }
}

pub fn check(metadata: &Metadata, policy: &Policy) -> Vec<Violation> {
    let mut violations = Vec::new();
    let members = metadata.member_names();
    for name in &members {
        if !policy.crates.contains_key(name) {
            violations.push(Violation::new(
                UNASSIGNED_CRATE,
                name,
                "workspace member has no layer in the policy",
            ));
        }
    }
    for name in policy.crates.keys() {
        if !members.contains(name) {
            violations.push(Violation::new(
                STALE_POLICY_ENTRY,
                name,
                "policy assigns a layer to a crate that is not a workspace member",
            ));
        }
    }
    for package in metadata.member_packages() {
        for dep in &package.dependencies {
            if dep.path.is_none() || !members.contains(&dep.name) {
                continue;
            }
            if !policy.allows(&package.name, &dep.name) {
                violations.push(Violation::new(
                    FORBIDDEN_EDGE,
                    format!(
                        "{} -> {}",
                        policy.describe(&package.name),
                        policy.describe(&dep.name)
                    ),
                    dep.qualifiers(),
                ));
            }
        }
    }
    if let Some(graph) = &metadata.resolve {
        for package in metadata.member_packages() {
            for (reached, path) in members_reached_outside(metadata, graph, &package.id) {
                if !policy.allows(&package.name, &reached) {
                    violations.push(Violation::new(
                        FORBIDDEN_TRANSITIVE_EDGE,
                        format!(
                            "{} -> {}",
                            policy.describe(&package.name),
                            policy.describe(&reached)
                        ),
                        format!("via {}", path.join(" -> ")),
                    ));
                }
            }
        }
    }
    violations
}

/// Workspace members that `start` reaches through at least one package that
/// is not a workspace member, with the path of package names. The first hop
/// may be of any kind; later hops follow normal and build edges, the ones a
/// dependency's own build links.
pub fn members_reached_outside(
    metadata: &Metadata,
    graph: &BTreeMap<String, Vec<crate::metadata::ResolvedEdge>>,
    start: &str,
) -> Vec<(String, Vec<String>)> {
    let name = |id: &str| {
        metadata
            .packages
            .get(id)
            .map_or_else(|| id.to_owned(), |p| p.name.clone())
    };
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    for edge in graph.get(start).map(Vec::as_slice).unwrap_or_default() {
        if !metadata.members.contains(&edge.to) && seen.insert(edge.to.clone()) {
            queue.push_back((edge.to.clone(), vec![name(&edge.to)]));
        }
    }
    while let Some((id, path)) = queue.pop_front() {
        for edge in graph.get(&id).map(Vec::as_slice).unwrap_or_default() {
            if !edge.kinds.iter().any(|k| *k != DepKind::Dev) {
                continue;
            }
            let mut next = path.clone();
            next.push(name(&edge.to));
            if metadata.members.contains(&edge.to) {
                found.push((name(&edge.to), next));
            } else if seen.insert(edge.to.clone()) {
                queue.push_back((edge.to.clone(), next));
            }
        }
    }
    found
}
