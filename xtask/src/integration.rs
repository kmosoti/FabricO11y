//! Read-only validation of facts collected by a trusted integration operator.
use serde::Deserialize;
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    repository: String,
    pr: u64,
    base: String,
    head: String,
    tree: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: u64,
    run_id: u64,
    attempt: u64,
    name: String,
    conclusion: String,
    checkout: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Actions {
    workflow: String,
    event: String,
    run_id: u64,
    attempt: u64,
    repository: String,
    pr: u64,
    head: String,
    jobs: Vec<Job>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    approved: bool,
    candidate: Candidate,
    author: String,
    reviewer: String,
    independent_context: String,
    receipt: String,
    semantic_scope: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinearCommit {
    sha: String,
    parents: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    version: u32,
    live: Candidate,
    reviewed: Review,
    base_ref: String,
    implementation_author: String,
    pr_open: bool,
    pr_draft: bool,
    conflict: bool,
    preview: String,
    preview_tree: String,
    head_tree: String,
    linear_commits: Vec<LinearCommit>,
    preview_parents: Vec<String>,
    changed_paths: Vec<String>,
    semantic_review_required: bool,
    actions: Actions,
}

fn protected(path: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "docs/contracts/",
        "registry/",
        "proofs/",
        "verification/",
        ".github/",
        ".codex/",
        "admission-kernel/",
        "schemas/",
        "spec/",
        "specs/",
    ];
    const FILES: &[&str] = &[
        "AGENTS.md",
        "CONTRIBUTING.md",
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "docs/architecture.md",
        "docs/direction.md",
        "xtask/src/integration.rs",
        "xtask/src/main.rs",
        "xtask/Cargo.toml",
    ];
    PREFIXES.iter().any(|p| path.starts_with(p))
        || FILES.contains(&path)
        || path.rsplit('/').next().is_some_and(|name| {
            ["auth", "schema", "policy", "spec", "contract", "gate"]
                .iter()
                .any(|word| name.to_ascii_lowercase().contains(word))
        })
        || path.split('/').any(|part| {
            matches!(
                part,
                "auth" | "authorization" | "schema" | "schemas" | "policy" | "policies"
            )
        })
}
fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.split('/').any(|s| s.is_empty() || s == "." || s == "..")
}
fn nonempty(s: &str) -> bool {
    !s.trim().is_empty()
}
fn validate(f: &Facts) -> Result<bool, String> {
    let fail = |s: &str| Err(s.to_owned());
    let c = &f.live;
    if f.version != 1
        || c.repository != "kmosoti/FabricO11y"
        || c.pr == 0
        || f.base_ref != "develop"
        || !f.pr_open
        || f.pr_draft
        || f.conflict
    {
        return fail("invalid repository, PR state, base or conflict");
    }
    if [&c.base, &c.head, &c.tree, &f.preview, &f.preview_tree]
        .iter()
        .any(|s| !xtask::valid_commit(s))
        || f.preview_tree != c.tree
        || f.preview_parents != [c.base.clone(), c.head.clone()]
    {
        return fail("invalid preview, tree or ordered parents");
    }
    if !xtask::valid_commit(&f.head_tree)
        || f.head_tree != f.preview_tree
        || f.linear_commits.is_empty()
    {
        return fail("head tree or linear chain missing or mismatched");
    }
    let mut previous = c.base.as_str();
    let mut seen = std::collections::HashSet::new();
    seen.insert(previous);
    for commit in &f.linear_commits {
        if !xtask::valid_commit(&commit.sha)
            || commit.parents.len() != 1
            || commit.parents[0] != previous
            || !seen.insert(&commit.sha)
        {
            return fail("invalid linear commit chain");
        }
        previous = &commit.sha;
    }
    if previous != c.head {
        return fail("linear chain does not end at head");
    }
    let r = &f.reviewed;
    if r.candidate.repository != c.repository
        || r.candidate.pr != c.pr
        || r.candidate.base != c.base
        || r.candidate.head != c.head
        || r.candidate.tree != c.tree
    {
        return fail("stale review candidate");
    }
    if !r.approved
        || !nonempty(&f.implementation_author)
        || !f
            .implementation_author
            .trim()
            .eq_ignore_ascii_case(r.author.trim())
        || !nonempty(&r.author)
        || !nonempty(&r.reviewer)
        || r.author.trim().eq_ignore_ascii_case(r.reviewer.trim())
        || !nonempty(&r.independent_context)
        || !nonempty(&r.receipt)
    {
        return fail("independent review required");
    }
    if f.changed_paths.is_empty() || f.changed_paths.iter().any(|p| !valid_path(p)) {
        return fail("invalid or empty changed paths");
    }
    let is_protected = f.changed_paths.iter().any(|p| protected(p));
    if (is_protected || f.semantic_review_required) && !nonempty(&r.semantic_scope) {
        return fail("protected changes require semantic review scope");
    }
    let a = &f.actions;
    if a.workflow != ".github/workflows/development-checks.yml"
        || a.event != "pull_request"
        || a.run_id == 0
        || a.attempt == 0
        || a.repository != c.repository
        || a.pr != c.pr
        || a.head != c.head
        || a.jobs.len() != 2
        || a.jobs[0].id == 0
        || a.jobs[0].id == a.jobs[1].id
    {
        return fail("invalid Actions run");
    }
    for name in ["verify", "native-acceptance"] {
        let matches: Vec<_> = a.jobs.iter().filter(|j| j.name == name).collect();
        if matches.len() != 1
            || matches[0].id == 0
            || matches[0].run_id != a.run_id
            || matches[0].attempt != a.attempt
            || matches[0].conclusion != "success"
            || matches[0].checkout != f.preview
        {
            return fail("missing, unsuccessful or mismatched Actions checkout");
        }
    }
    Ok(is_protected)
}
pub fn cli(file: &Path) -> Result<(), String> {
    let mut bytes = Vec::new();
    File::open(file)
        .map_err(|e| e.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("facts exceed 1 MiB".into());
    }
    let facts: Facts = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let protected = validate(&facts)?;
    println!("integration evidence valid; protected inventory match: {protected}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Facts {
        let c = Candidate {
            repository: "kmosoti/FabricO11y".into(),
            pr: 297,
            base: "a".repeat(40),
            head: "b".repeat(40),
            tree: "c".repeat(40),
        };
        Facts {
            version: 1,
            live: c.clone(),
            reviewed: Review {
                approved: true,
                candidate: c.clone(),
                author: "author".into(),
                reviewer: "reviewer".into(),
                independent_context: "fresh review session".into(),
                receipt: "review record".into(),
                semantic_scope: String::new(),
            },
            base_ref: "develop".into(),
            implementation_author: "author".into(),
            pr_draft: false,
            pr_open: true,
            conflict: false,
            preview: "d".repeat(40),
            preview_tree: c.tree.clone(),
            head_tree: c.tree.clone(),
            linear_commits: vec![LinearCommit {
                sha: c.head.clone(),
                parents: vec![c.base.clone()],
            }],
            preview_parents: vec![c.base.clone(), c.head.clone()],
            changed_paths: vec!["fabric-core/src/lib.rs".into()],
            semantic_review_required: false,
            actions: Actions {
                workflow: ".github/workflows/development-checks.yml".into(),
                event: "pull_request".into(),
                run_id: 1,
                attempt: 1,
                repository: c.repository,
                pr: c.pr,
                head: c.head,
                jobs: ["verify", "native-acceptance"]
                    .iter()
                    .enumerate()
                    .map(|(i, name)| Job {
                        id: i as u64 + 1,
                        run_id: 1,
                        attempt: 1,
                        name: (*name).into(),
                        conclusion: "success".into(),
                        checkout: "d".repeat(40),
                    })
                    .collect(),
            },
        }
    }
    #[test]
    fn implementation_and_protected() {
        let mut f = fixture();
        assert_eq!(validate(&f), Ok(false));
        f.changed_paths.push("proofs/Telemetry.lean".into());
        assert!(validate(&f).is_err());
        f.reviewed.semantic_scope = "changed theorem and assumptions".into();
        assert_eq!(validate(&f), Ok(true));
    }
    #[test]
    fn stale_and_same_author_rejected() {
        let f = fixture();
        let mut x = f.clone();
        x.live.base = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.live.head = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.live.tree = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.reviewed.reviewer = "AUTHOR".into();
        assert!(validate(&x).is_err());
        let mut x = f;
        x.conflict = true;
        assert!(validate(&x).is_err());
    }
    #[test]
    fn actions_proof_required() {
        let f = fixture();
        let mut x = f.clone();
        x.actions.jobs[0].checkout = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.actions.jobs[1].conclusion = "failure".into();
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.actions.jobs.pop();
        assert!(validate(&x).is_err());
        let mut x = f;
        x.preview_parents.reverse();
        assert!(validate(&x).is_err());
    }
    #[test]
    fn fixture_and_policy_boundaries() {
        let f: Facts =
            serde_json::from_str(include_str!("../tests/fixtures/integration-v1.json")).unwrap();
        assert_eq!(validate(&f), Ok(true));
        let mut renamed = fixture();
        renamed.changed_paths = vec!["proofs/old.lean".into(), "fabric-core/src/new.rs".into()];
        assert!(validate(&renamed).is_err());
        let mut wrong = f.clone();
        wrong.actions.workflow = "other.yml".into();
        assert!(validate(&wrong).is_err());
        let mut wrong = f.clone();
        wrong.actions.jobs[0].run_id += 1;
        assert!(validate(&wrong).is_err());
        let mut wrong = fixture();
        wrong.semantic_review_required = true;
        assert!(validate(&wrong).is_err());
        let mut wrong = f.clone();
        wrong.actions.run_id = 0;
        assert!(validate(&wrong).is_err());
        let mut wrong = f;
        wrong.reviewed.reviewer = wrong.reviewed.author.clone();
        wrong.reviewed.independent_context = "different context".into();
        assert!(validate(&wrong).is_err());
    }
    #[test]
    fn rebase_equivalence_and_approval_required() {
        let f = fixture();
        let mut multi = f.clone();
        multi.linear_commits.insert(
            0,
            LinearCommit {
                sha: "e".repeat(40),
                parents: vec![multi.live.base.clone()],
            },
        );
        multi.linear_commits[1].parents[0] = "e".repeat(40);
        assert_eq!(validate(&multi), Ok(false));
        let mut x = f.clone();
        x.head_tree = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.linear_commits[0].parents.push("e".repeat(40));
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.linear_commits.clear();
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.linear_commits[0].parents[0] = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f.clone();
        x.linear_commits[0].sha = "e".repeat(40);
        assert!(validate(&x).is_err());
        let mut x = f;
        x.reviewed.approved = false;
        assert!(validate(&x).is_err());
    }
}
