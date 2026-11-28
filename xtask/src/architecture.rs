use proc_macro2::{TokenStream, TokenTree};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};
const PORTABLE_QUERY_ALLOW: &[&str] = &[
    "fabric-query-contract",
    "fabric-ops",
    "serde",
    "serde_core",
    "serde_derive",
    "serde_json",
    "proc-macro2",
    "quote",
    "syn",
    "unicode-ident",
    "itoa",
    "memchr",
    "zmij",
];

#[derive(Deserialize)]
struct Policy {
    crates: BTreeMap<String, Vec<String>>,
    headless: HeadlessPolicy,
    optional_crates: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct HeadlessPolicy {
    roots: Vec<String>,
    renderer_families: Vec<String>,
    optional_packages: Vec<String>,
}
fn audit(tokens: TokenStream) -> Result<(), String> {
    let mut declaration = false;
    for token in tokens {
        match token {
            TokenTree::Group(g) => {
                if declaration
                    && matches!(
                        g.delimiter(),
                        proc_macro2::Delimiter::Brace | proc_macro2::Delimiter::Parenthesis
                    )
                {
                    for field_token in g.stream() {
                        if matches!(field_token, TokenTree::Ident(ref i) if i == "pub") {
                            return Err("kernel state fields must be private".into());
                        }
                    }
                    if g.delimiter() == proc_macro2::Delimiter::Brace {
                        declaration = false;
                    }
                }
                audit(g.stream())?;
            }
            TokenTree::Ident(ref i) if i == "struct" || i == "enum" => declaration = true,
            TokenTree::Punct(ref p) if p.as_char() == ';' => declaration = false,
            TokenTree::Ident(i)
                if [
                    "unsafe",
                    "assume",
                    "admit",
                    "external",
                    "external_body",
                    "assume_specification",
                    "external_fn_specification",
                ]
                .contains(&i.to_string().as_str()) =>
            {
                return Err(format!("unreviewed kernel token: {i}"));
            }
            _ => {}
        }
    }
    Ok(())
}
fn files(path: &Path) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            files(&path)?;
        } else if path.extension().is_some_and(|s| s == "rs") {
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            audit(
                text.parse()
                    .map_err(|e| format!("cannot parse {}: {e}", path.display()))?,
            )?;
        }
    }
    Ok(())
}
// These small Sans-I/O profiles are deliberately allowlisted transitively.
fn check_profile_graph(root_name: &str, graph: &str, allowed: &[&str]) -> Result<(), String> {
    for package in graph
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        if !allowed.contains(&package) {
            return Err(format!(
                "unreviewed transitive dependency for {root_name}: {package}"
            ));
        }
    }
    if !graph
        .lines()
        .any(|line| line.split_whitespace().next() == Some(root_name))
    {
        return Err(format!("missing profile root {root_name}"));
    }
    Ok(())
}

// Only workspace members are reviewed; metadata --no-deps never fetches or
// builds optional dependency closures. Lockfiles must already be present.
fn check_optional_metadata(
    metadata: &serde_json::Value,
    policy: &Policy,
    expected_members: &[&str],
    expected_defaults: &[&str],
) -> Result<(), String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("missing optional packages")?;
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or("missing optional members")?;
    let defaults = metadata["workspace_default_members"]
        .as_array()
        .ok_or("missing optional defaults")?;
    let names = |ids: &[serde_json::Value]| -> Result<Vec<String>, String> {
        ids.iter()
            .map(|id| {
                let package = packages
                    .iter()
                    .find(|p| p["id"] == *id)
                    .ok_or("unknown optional member")?;
                Ok(package["name"]
                    .as_str()
                    .ok_or("missing optional name")?
                    .to_owned())
            })
            .collect()
    };
    let mut actual = names(members)?;
    let mut expected: Vec<String> = expected_members.iter().map(|s| (*s).into()).collect();
    actual.sort();
    expected.sort();
    if actual != expected {
        return Err("optional workspace members differ from policy".into());
    }
    let mut actual = names(defaults)?;
    let mut expected: Vec<String> = expected_defaults.iter().map(|s| (*s).into()).collect();
    actual.sort();
    expected.sort();
    if actual != expected {
        return Err("optional workspace defaults differ from policy".into());
    }
    for name in names(members)? {
        let package = packages
            .iter()
            .find(|p| p["name"].as_str() == Some(name.as_str()))
            .ok_or("missing optional package")?;
        let allowed = policy
            .optional_crates
            .get(&name)
            .ok_or_else(|| format!("unreviewed optional crate: {name}"))?;
        for dep in package["dependencies"]
            .as_array()
            .ok_or("missing optional dependencies")?
        {
            let dep = dep["name"].as_str().ok_or("missing dependency name")?;
            if !allowed.iter().any(|a| a == dep) {
                return Err(format!("unreviewed dependency {name} -> {dep}"));
            }
        }
    }
    Ok(())
}

pub fn check(root: &Path) -> Result<(), String> {
    let policy: Policy = toml::from_str(
        &fs::read_to_string(root.join("verification/architecture.toml"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let metadata: serde_json::Value = serde_json::from_str(&super::output(
        "cargo",
        &["metadata", "--no-deps", "--format-version", "1", "--locked"],
        root,
    )?)
    .map_err(|e| e.to_string())?;
    for package in metadata["packages"].as_array().ok_or("missing packages")? {
        let name = package["name"].as_str().ok_or("missing name")?;
        let allowed = policy
            .crates
            .get(name)
            .ok_or_else(|| format!("unreviewed crate: {name}"))?;
        for dependency in package["dependencies"]
            .as_array()
            .ok_or("missing dependencies")?
        {
            let dep = dependency["name"]
                .as_str()
                .ok_or("missing dependency name")?;
            if !allowed.iter().any(|a| a == dep) {
                return Err(format!("unreviewed dependency {name} -> {dep}"));
            }
        }
    }
    for (manifest, members, defaults) in [
        (
            "frontend/Cargo.toml",
            &["fabric-app-core", "fabric-app-parity", "fabric-ui-dom"][..],
            &["fabric-app-core"][..],
        ),
        (
            "frontend/gateway/Cargo.toml",
            &["fabric-ui-gateway-core"][..],
            &["fabric-ui-gateway-core"][..],
        ),
    ] {
        let metadata: serde_json::Value = serde_json::from_str(&super::output(
            "cargo",
            &[
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--locked",
                "--offline",
                "--manifest-path",
                manifest,
            ],
            root,
        )?)
        .map_err(|e| e.to_string())?;
        check_optional_metadata(&metadata, &policy, members, defaults)?;
    }
    let mut args = vec![
        "tree",
        "--locked",
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ];
    for package in &policy.headless.roots {
        args.extend(["--package", package]);
    }
    let graph = super::output("cargo", &args, root)?;
    for package in graph
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        if policy
            .headless
            .optional_packages
            .iter()
            .any(|name| name == package)
            || policy.headless.renderer_families.iter().any(|family| {
                package == family
                    || package
                        .strip_prefix(family)
                        .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with('_'))
            })
        {
            return Err(format!(
                "renderer dependency in default headless graph: {package}"
            ));
        }
    }
    for (package, allowed) in [
        ("fabric-query-contract", PORTABLE_QUERY_ALLOW),
        ("fabric-ops", &["fabric-ops"][..]),
        ("fabric-colloquium", &["fabric-colloquium"][..]),
        (
            "fabric-corpus",
            &[
                "fabric-corpus",
                "sha2",
                "cfg-if",
                "cpufeatures",
                "libc",
                "digest",
                "block-buffer",
                "crypto-common",
                "generic-array",
                "typenum",
                "version_check",
            ][..],
        ),
    ] {
        for target in ["x86_64-unknown-linux-gnu", "wasm32-unknown-unknown"] {
            let graph = super::output(
                "cargo",
                &[
                    "tree",
                    "--locked",
                    "--offline",
                    "--edges",
                    "normal,build",
                    "--prefix",
                    "none",
                    "--format",
                    "{p}",
                    "--target",
                    target,
                    "--package",
                    package,
                ],
                root,
            )?;
            check_profile_graph(package, &graph, allowed)?;
        }
    }
    files(&root.join("admission-kernel/src"))?;
    println!("Architecture boundaries passed");
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn profile_graph_rejects_unreviewed_transitive_dependency() {
        assert!(
            super::check_profile_graph("fabric-ops", "fabric-ops v1\n", &["fabric-ops"]).is_ok()
        );
        assert!(
            super::check_profile_graph("fabric-ops", "fabric-ops v1\ntokio v1\n", &["fabric-ops"])
                .is_err()
        );
        assert!(super::check_profile_graph("fabric-ops", "", &["fabric-ops"]).is_err());
        assert!(
            super::check_profile_graph(
                "fabric-query-contract",
                "fabric-query-contract v1\nserde v1\n",
                super::PORTABLE_QUERY_ALLOW
            )
            .is_ok()
        );
        for forbidden in ["fabric", "tokio", "rusqlite", "schemars"] {
            let graph = format!("fabric-query-contract v1\n{forbidden} v1\n");
            assert!(
                super::check_profile_graph(
                    "fabric-query-contract",
                    &graph,
                    super::PORTABLE_QUERY_ALLOW
                )
                .is_err()
            );
        }
    }
    #[test]
    fn optional_metadata_rejects_dependency_and_default_drift() {
        let policy: super::Policy =
            toml::from_str(include_str!("../../verification/architecture.toml")).unwrap();
        let mut metadata = serde_json::json!({
            "packages": [
                {"id":"core", "name":"fabric-app-core", "dependencies":[{"name":"serde"}]},
                {"id":"dom", "name":"fabric-ui-dom", "dependencies":[{"name":"leptos"}]},
                {"id":"parity", "name":"fabric-app-parity", "dependencies":[]}
            ],
            "workspace_members":["core", "dom", "parity"],
            "workspace_default_members":["core"]
        });
        let members = ["fabric-app-core", "fabric-ui-dom", "fabric-app-parity"];
        assert!(
            super::check_optional_metadata(&metadata, &policy, &members, &["fabric-app-core"])
                .is_ok()
        );
        metadata["workspace_default_members"] = serde_json::json!(["core", "dom"]);
        assert!(
            super::check_optional_metadata(&metadata, &policy, &members, &["fabric-app-core"])
                .is_err()
        );
        metadata["workspace_default_members"] = serde_json::json!(["core"]);
        metadata["packages"][0]["dependencies"] = serde_json::json!([{"name":"tokio"}]);
        assert!(
            super::check_optional_metadata(&metadata, &policy, &members, &["fabric-app-core"])
                .is_err()
        );
        metadata["packages"][0]["dependencies"] = serde_json::json!([]);
        metadata["workspace_members"] = serde_json::json!(["core", "dom"]);
        assert!(
            super::check_optional_metadata(&metadata, &policy, &members, &["fabric-app-core"])
                .is_err()
        );
    }
    #[test]
    fn rejects_verification_escapes_not_comments() {
        assert!(super::audit("fn a(){ assume(false); }".parse().unwrap()).is_err());
        assert!(super::audit("// assume(false)\nfn a(){}".parse().unwrap()).is_ok());
        assert!(super::audit("#[verifier::external_body] fn a(){}".parse().unwrap()).is_err());
        assert!(super::audit("pub struct State { pub used: usize }".parse().unwrap()).is_err());
        assert!(super::audit("pub struct State(pub usize);".parse().unwrap()).is_err());
        assert!(super::audit("pub struct State { used: usize } impl State { pub fn used(&self) -> usize { self.used } }".parse().unwrap()).is_ok());
    }
}
