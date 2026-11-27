use proc_macro2::{TokenStream, TokenTree};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};
#[derive(Deserialize)]
struct Policy {
    crates: BTreeMap<String, Vec<String>>,
    headless: HeadlessPolicy,
}

#[derive(Deserialize)]
struct HeadlessPolicy {
    roots: Vec<String>,
    renderer_families: Vec<String>,
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
        if policy.headless.renderer_families.iter().any(|family| {
            package == family
                || package
                    .strip_prefix(family)
                    .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with('_'))
        }) {
            return Err(format!(
                "renderer dependency in default headless graph: {package}"
            ));
        }
    }
    for (package, allowed) in [
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
