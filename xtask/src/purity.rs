//! Mechanically checkable purity properties of the semantic core.
//!
//! `#![no_std]` already removes the standard library's clock, environment,
//! filesystem, network, process and thread APIs from the core. This gate
//! checks what the compiler alone does not: which crates the core may link
//! (an explicit allowlist, plus named categories for clear failures), that it
//! has no build script and no unreviewed features, that the no_std and
//! forbid-unsafe attributes stay, and a few syntactic escape hatches such as
//! `extern crate std`. It does not prove a function is deterministic.

use crate::Violation;
use crate::metadata::{DepKind, Metadata, Package};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

pub const WORKSPACE_EDGE: &str = "PURITY_WORKSPACE_EDGE";
pub const DENIED_DEPENDENCY: &str = "PURITY_DENIED_DEPENDENCY";
pub const UNREVIEWED_DEPENDENCY: &str = "PURITY_UNREVIEWED_DEPENDENCY";
pub const TRANSITIVE_DENIED: &str = "PURITY_TRANSITIVE_DENIED";
pub const BUILD_SCRIPT: &str = "PURITY_BUILD_SCRIPT";
pub const FEATURE: &str = "PURITY_FEATURE";
pub const MISSING_ATTRIBUTE: &str = "PURITY_MISSING_ATTRIBUTE";
pub const AMBIENT_SOURCE: &str = "PURITY_AMBIENT_SOURCE";
pub const MISSING_CORE: &str = "PURITY_MISSING_CORE_CRATE";

#[derive(Debug, Clone)]
pub struct Policy {
    pub core_crates: BTreeSet<String>,
    pub allowed_dependencies: BTreeSet<String>,
    pub allowed_dev_dependencies: BTreeSet<String>,
    pub allowed_features: BTreeSet<String>,
    /// crate name -> category, e.g. `tokio -> async-runtime`.
    pub denied: BTreeMap<String, String>,
    pub required_attributes: Vec<String>,
    /// source pattern -> reason.
    pub forbidden_patterns: BTreeMap<String, String>,
}

fn names(json: &Value, key: &str) -> Result<BTreeSet<String>, String> {
    match json.get(key) {
        None => Ok(BTreeSet::new()),
        Some(Value::Array(items)) => Ok(items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()),
        // `{ "name": "reason" }`: the reason is required documentation.
        Some(Value::Object(items)) => {
            for (name, reason) in items {
                if reason.as_str().is_none_or(str::is_empty) {
                    return Err(format!("purity policy: `{key}.{name}` needs a reason"));
                }
            }
            Ok(items.keys().cloned().collect())
        }
        Some(_) => Err(format!("purity policy: `{key}` must be an array or object")),
    }
}

impl Policy {
    pub fn parse(json: &Value) -> Result<Self, String> {
        let mut denied = BTreeMap::new();
        if let Some(categories) = json.get("denied").and_then(Value::as_object) {
            for (category, crates) in categories {
                for name in crates.as_array().map(Vec::as_slice).unwrap_or_default() {
                    if let Some(name) = name.as_str() {
                        denied.insert(name.to_owned(), category.clone());
                    }
                }
            }
        }
        let mut forbidden_patterns = BTreeMap::new();
        if let Some(patterns) = json
            .get("forbidden_source_patterns")
            .and_then(Value::as_object)
        {
            for (pattern, reason) in patterns {
                forbidden_patterns.insert(
                    pattern.clone(),
                    reason.as_str().unwrap_or_default().to_owned(),
                );
            }
        }
        let core_crates = names(json, "core_crates")?;
        if core_crates.is_empty() {
            return Err("purity policy: `core_crates` is empty".into());
        }
        Ok(Self {
            core_crates,
            allowed_dependencies: names(json, "allowed_dependencies")?,
            allowed_dev_dependencies: names(json, "allowed_dev_dependencies")?,
            allowed_features: names(json, "allowed_features")?,
            denied,
            required_attributes: names(json, "required_attributes")?.into_iter().collect(),
            forbidden_patterns,
        })
    }
}

pub fn check(metadata: &Metadata, policy: &Policy) -> Vec<Violation> {
    let mut violations = Vec::new();
    let members = metadata.member_names();
    for core in &policy.core_crates {
        let Some(package) = metadata.by_name(core) else {
            violations.push(Violation::new(
                MISSING_CORE,
                core,
                "core crate named by the policy is not a workspace member",
            ));
            continue;
        };
        check_dependencies(package, policy, &members, &mut violations);
        if package.has_target_kind("custom-build") {
            violations.push(Violation::new(
                BUILD_SCRIPT,
                core,
                "a build script runs arbitrary code with ambient access at build time",
            ));
        }
        for feature in &package.features {
            if !policy.allowed_features.contains(feature) {
                violations.push(Violation::new(
                    FEATURE,
                    core,
                    format!("feature `{feature}` is not in allowed_features"),
                ));
            }
        }
        check_source(package, policy, &mut violations);
        if let Some(graph) = &metadata.resolve {
            check_closure(metadata, graph, package, policy, &mut violations);
        }
    }
    violations
}

fn check_dependencies(
    package: &Package,
    policy: &Policy,
    members: &BTreeSet<String>,
    violations: &mut Vec<Violation>,
) {
    for dep in &package.dependencies {
        let subject = format!("{} -> {}", package.name, dep.name);
        if dep.path.is_some() && members.contains(&dep.name) {
            if !policy.core_crates.contains(&dep.name) {
                violations.push(Violation::new(WORKSPACE_EDGE, subject, dep.qualifiers()));
            }
            continue;
        }
        if dep.kind == DepKind::Dev {
            if !policy.allowed_dev_dependencies.contains(&dep.name) {
                violations.push(Violation::new(
                    UNREVIEWED_DEPENDENCY,
                    subject,
                    format!("{}; not in allowed_dev_dependencies", dep.qualifiers()),
                ));
            }
            continue;
        }
        if let Some(category) = policy.denied.get(&dep.name) {
            violations.push(Violation::new(
                DENIED_DEPENDENCY,
                subject,
                format!("category={category}; {}", dep.qualifiers()),
            ));
        } else if !policy.allowed_dependencies.contains(&dep.name) {
            violations.push(Violation::new(
                UNREVIEWED_DEPENDENCY,
                subject,
                format!("{}; not in allowed_dependencies", dep.qualifiers()),
            ));
        }
    }
}

fn check_source(package: &Package, policy: &Policy, violations: &mut Vec<Violation>) {
    let Some(lib_root) = package.lib_root() else {
        violations.push(Violation::new(
            MISSING_ATTRIBUTE,
            &package.name,
            "core crate has no library target",
        ));
        return;
    };
    let root_text = std::fs::read_to_string(lib_root).unwrap_or_default();
    for attribute in &policy.required_attributes {
        if !code_lines(&root_text).any(|line| line.trim() == attribute) {
            violations.push(Violation::new(
                MISSING_ATTRIBUTE,
                &package.name,
                format!("{} lacks `{attribute}`", lib_root.display()),
            ));
        }
    }
    let Some(src_dir) = lib_root.parent() else {
        return;
    };
    let mut files = Vec::new();
    rust_files(src_dir, &mut files);
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (number, line) in code_lines(&text).enumerate() {
            for (pattern, reason) in &policy.forbidden_patterns {
                if line.contains(pattern.as_str()) {
                    violations.push(Violation::new(
                        AMBIENT_SOURCE,
                        &package.name,
                        format!("{}:{}: `{pattern}`: {reason}", file.display(), number + 1),
                    ));
                }
            }
        }
    }
}

/// Lines with `//` comments removed. Block comments and strings are not
/// parsed: this is a syntactic guard, documented as such.
fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every package the core links through normal and build edges, checked
/// against the denied categories and against workspace members.
fn check_closure(
    metadata: &Metadata,
    graph: &BTreeMap<String, Vec<crate::metadata::ResolvedEdge>>,
    package: &Package,
    policy: &Policy,
    violations: &mut Vec<Violation>,
) {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([(package.id.clone(), vec![package.name.clone()])]);
    while let Some((id, path)) = queue.pop_front() {
        for edge in graph.get(&id).map(Vec::as_slice).unwrap_or_default() {
            if !edge.kinds.iter().any(|k| *k != DepKind::Dev) || !seen.insert(edge.to.clone()) {
                continue;
            }
            let Some(target) = metadata.packages.get(&edge.to) else {
                continue;
            };
            let mut next = path.clone();
            next.push(target.name.clone());
            let direct = next.len() == 2;
            if metadata.members.contains(&edge.to) {
                if !policy.core_crates.contains(&target.name) && !direct {
                    violations.push(Violation::new(
                        WORKSPACE_EDGE,
                        format!("{} -> {}", package.name, target.name),
                        format!("transitively via {}", next.join(" -> ")),
                    ));
                }
                // A member's own dependencies are checked on their own.
                continue;
            } else if let Some(category) = policy.denied.get(&target.name) {
                if !direct {
                    violations.push(Violation::new(
                        TRANSITIVE_DENIED,
                        format!("{} -> {}", package.name, target.name),
                        format!("category={category}; via {}", next.join(" -> ")),
                    ));
                }
            }
            queue.push_back((edge.to.clone(), next));
        }
    }
}
