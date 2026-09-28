//! A small typed view of `cargo metadata` output.
//!
//! Declared metadata (`--no-deps`) is what each manifest says, including
//! optional, renamed, target-specific, build and development dependencies,
//! whether or not a feature activates them. Resolved metadata is Cargo's
//! dependency graph after feature and version resolution; the gates use it to
//! see edges that only exist transitively.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

impl DepKind {
    fn parse(value: &Value) -> Self {
        match value.as_str() {
            Some("dev") => DepKind::Dev,
            Some("build") => DepKind::Build,
            _ => DepKind::Normal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DepKind::Normal => "normal",
            DepKind::Dev => "dev",
            DepKind::Build => "build",
        }
    }
}

/// One dependency as a manifest declares it.
#[derive(Debug, Clone)]
pub struct Declared {
    /// The depended-on package's real name, even when renamed.
    pub name: String,
    pub rename: Option<String>,
    pub kind: DepKind,
    pub optional: bool,
    pub target: Option<String>,
    pub path: Option<PathBuf>,
}

impl Declared {
    /// Human-readable qualifiers that the failure message carries.
    pub fn qualifiers(&self) -> String {
        format!(
            "kind={}, optional={}, target={}, renamed={}",
            self.kind.label(),
            self.optional,
            self.target.as_deref().unwrap_or("-"),
            self.rename.as_deref().unwrap_or("-"),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<Declared>,
    pub features: BTreeSet<String>,
    /// `(kind, src_path)` for every target, e.g. `("lib", ".../src/lib.rs")`.
    pub targets: Vec<(Vec<String>, PathBuf)>,
}

impl Package {
    pub fn has_target_kind(&self, kind: &str) -> bool {
        self.targets
            .iter()
            .any(|(kinds, _)| kinds.iter().any(|k| k == kind))
    }

    pub fn lib_root(&self) -> Option<&Path> {
        self.targets
            .iter()
            .find(|(kinds, _)| kinds.iter().any(|k| k == "lib"))
            .map(|(_, path)| path.as_path())
    }
}

/// A resolved edge from one package id to another, with its kinds.
#[derive(Debug, Clone)]
pub struct ResolvedEdge {
    pub to: String,
    pub kinds: BTreeSet<DepKind>,
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub packages: BTreeMap<String, Package>,
    pub members: BTreeSet<String>,
    /// Present only when metadata was resolved.
    pub resolve: Option<BTreeMap<String, Vec<ResolvedEdge>>>,
}

impl Metadata {
    pub fn member_packages(&self) -> impl Iterator<Item = &Package> {
        self.members.iter().filter_map(|id| self.packages.get(id))
    }

    pub fn member_names(&self) -> BTreeSet<String> {
        self.member_packages().map(|p| p.name.clone()).collect()
    }

    pub fn by_name(&self, name: &str) -> Option<&Package> {
        self.member_packages().find(|p| p.name == name)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Also ask Cargo for the resolved graph.
    pub resolve: bool,
    /// Pass `--offline`; the fixtures use it so tests never touch a registry.
    pub offline: bool,
}

pub fn load(manifest: &Path, options: Options) -> Result<Metadata, String> {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest);
    if !options.resolve {
        command.arg("--no-deps");
    } else if manifest
        .parent()
        .is_some_and(|dir| dir.join("Cargo.lock").is_file())
    {
        // Never let a check rewrite the lockfile it is checking.
        command.arg("--locked");
    }
    if options.offline {
        command.arg("--offline");
    }
    let output = command
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cargo metadata printed invalid JSON: {e}"))?;
    parse(&json)
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("cargo metadata: missing string `{key}`"))
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("cargo metadata: missing array `{key}`"))
}

pub fn parse(json: &Value) -> Result<Metadata, String> {
    let mut packages = BTreeMap::new();
    for package in array(json, "packages")? {
        let mut dependencies = Vec::new();
        for dep in array(package, "dependencies")? {
            dependencies.push(Declared {
                name: text(dep, "name")?,
                rename: dep.get("rename").and_then(Value::as_str).map(str::to_owned),
                kind: DepKind::parse(dep.get("kind").unwrap_or(&Value::Null)),
                optional: dep
                    .get("optional")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                target: dep.get("target").and_then(Value::as_str).map(str::to_owned),
                path: dep.get("path").and_then(Value::as_str).map(PathBuf::from),
            });
        }
        let features = package
            .get("features")
            .and_then(Value::as_object)
            .map(|f| f.keys().cloned().collect())
            .unwrap_or_default();
        let mut targets = Vec::new();
        for target in array(package, "targets")? {
            let kinds = array(target, "kind")?
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            targets.push((kinds, PathBuf::from(text(target, "src_path")?)));
        }
        let id = text(package, "id")?;
        packages.insert(
            id.clone(),
            Package {
                id,
                name: text(package, "name")?,
                manifest_path: PathBuf::from(text(package, "manifest_path")?),
                dependencies,
                features,
                targets,
            },
        );
    }
    let members = array(json, "workspace_members")?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let resolve = match json.get("resolve") {
        Some(Value::Object(resolve)) => {
            let mut graph = BTreeMap::new();
            for node in resolve
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or("cargo metadata: resolve without nodes")?
            {
                let mut edges = Vec::new();
                for dep in array(node, "deps")? {
                    let kinds = array(dep, "dep_kinds")?
                        .iter()
                        .map(|k| DepKind::parse(k.get("kind").unwrap_or(&Value::Null)))
                        .collect();
                    edges.push(ResolvedEdge {
                        to: text(dep, "pkg")?,
                        kinds,
                    });
                }
                graph.insert(text(node, "id")?, edges);
            }
            Some(graph)
        }
        _ => None,
    };
    Ok(Metadata {
        packages,
        members,
        resolve,
    })
}
