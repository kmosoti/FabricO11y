//! Checks guide references and publishes candidate-scoped evidence.

use crate::{coverage, evidence};
use fabric_core::guide::{
    CATALOG_PATH, Catalog, CheckOutcome, CheckStatus, Checker, Evidence, SCHEMA,
    is_candidate_revision,
};
use std::{collections::BTreeSet, fmt::Write, fs, path::Path};

const REPOSITORY: &str = "https://github.com/kmosoti/FabricO11y";

pub(super) fn validate_catalog<'a>(
    root: &Path,
    check_ids: impl IntoIterator<Item = &'a str>,
) -> Result<Catalog, String> {
    let catalog = Catalog::parse(
        &fs::read(root.join(CATALOG_PATH)).map_err(|error| format!("{CATALOG_PATH}: {error}"))?,
    )?;
    let check_ids: BTreeSet<_> = check_ids.into_iter().collect();
    for topic in &catalog.topics {
        for scenario in &topic.scenarios {
            for check in &scenario.checks {
                if !check_ids.contains(check.as_str()) {
                    return Err(format!(
                        "guide topic {} references unknown check {check}",
                        topic.id
                    ));
                }
            }
        }
        for path in &topic.documentation {
            if !root.join(path).is_file() {
                return Err(format!(
                    "guide topic {} references missing {path}",
                    topic.id
                ));
            }
        }
        for proof in &topic.proofs {
            let theorem = proof.theorem.rsplit('.').next().unwrap_or(&proof.theorem);
            let source = fs::read_to_string(root.join(&proof.path))
                .map_err(|error| format!("{}: {error}", proof.path))?;
            let declared = source.lines().any(|line| {
                line.trim().strip_prefix("theorem ").is_some_and(|rest| {
                    rest.strip_prefix(theorem)
                        .is_some_and(|suffix| suffix.starts_with([' ', ':', '(']))
                })
            });
            if !declared || !crate::proof::guide_checker(&proof.checker, &proof.path) {
                return Err(format!(
                    "guide proof {} has no matching theorem or checker in {}",
                    proof.theorem, proof.path
                ));
            }
        }
    }
    Ok(catalog)
}

pub(super) fn wiki(root: &Path, revision: &str) -> Result<(), String> {
    if !is_candidate_revision(revision) {
        return Err("guide wiki expects a complete lowercase commit ID".into());
    }
    let diff = crate::output(
        "jj",
        &["diff", "--from", revision, "--to", "@", "--summary"],
        root,
    )?;
    if !diff.is_empty() {
        return Err("guide wiki workspace differs from the requested revision".into());
    }
    let catalog = coverage::guide_catalog(root)?;
    print!("{}", render_wiki(&catalog, revision));
    Ok(())
}

fn render_wiki(catalog: &Catalog, revision: &str) -> String {
    let mut page = format!(
        "# Features and verification\n\nDevelopment snapshot [`{revision}`]({REPOSITORY}/commit/{revision}). It may describe unreleased functionality. Use `fabric guide <topic>` for offline instructions. Test and proof entries describe the scope of checks available at this revision.\n"
    );
    for topic in &catalog.topics {
        writeln!(page, "\n## {}\n\n{}\n", topic.title, topic.summary).unwrap();
        writeln!(page, "Run `fabric guide {}`.", topic.id).unwrap();
        for scenario in &topic.scenarios {
            writeln!(
                page,
                "\n- {}\n  `{}`\n  {}",
                scenario.description, scenario.command, scenario.expected
            )
            .unwrap();
        }
        writeln!(page, "\nDocumentation").unwrap();
        for path in &topic.documentation {
            writeln!(page, "\n- [`{path}`]({REPOSITORY}/blob/{revision}/{path})").unwrap();
        }
        if !topic.proofs.is_empty() {
            writeln!(page, "\nScoped proof references").unwrap();
        }
        for proof in &topic.proofs {
            writeln!(
                page,
                "\n- [`{}`]({REPOSITORY}/blob/{revision}/{}) — {} {}",
                proof.theorem, proof.path, proof.scope, proof.assumptions
            )
            .unwrap();
        }
    }
    page
}

/// Capture identities before and after acceptance so changes invalidate the
/// candidate alongside the repository's existing content check.
pub(super) fn checker_identities(root: &Path) -> Result<Vec<Checker>, String> {
    let mut checkers = vec![
        Checker {
            id: "rustc".into(),
            identity: crate::output("rustc", &["--version"], root)?,
        },
        Checker {
            id: "cargo".into(),
            identity: crate::output("cargo", &["--version"], root)?,
        },
    ];
    for (id, path) in crate::proof::checker_binaries()?
        .into_iter()
        .chain(crate::extraction::checker_binaries())
    {
        checkers.push(Checker {
            id: id.into(),
            identity: format!("sha256:{}", evidence::hash(&path)?),
        });
    }
    Ok(checkers)
}

pub(super) fn write_candidate_evidence(
    root: &Path,
    candidate: &str,
    checkers: Vec<Checker>,
) -> Result<(), String> {
    let mut checks = coverage::tested_outcomes(root)?;
    checks.push(CheckOutcome {
        id: "native-proof-suite".into(),
        status: CheckStatus::Passed,
    });
    checks.push(CheckOutcome {
        id: "native-proof-selftest".into(),
        status: CheckStatus::Passed,
    });
    let report = Evidence {
        schema: SCHEMA,
        candidate: candidate.to_owned(),
        catalog_sha256: evidence::hash(&root.join(CATALOG_PATH))?,
        manifest_sha256: coverage::digest(root)?,
        checks,
        checkers,
        artifacts: vec!["coverage.json".into()],
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    Evidence::parse(&bytes)?;
    evidence::write(&root.join(".local/guide-evidence.json"), &report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_links_are_revision_pinned() {
        let catalog = Catalog::parse(fabric_core::guide::CATALOG_BYTES).unwrap();
        let revision = "a".repeat(40);
        let page = render_wiki(&catalog, &revision);
        assert!(page.contains(&format!("/{revision}/docs/collection.md")));
        assert!(page.contains(&format!("/{revision}/proofs/Custody.lean")));
        assert!(page.contains("fabric guide fleet"));
    }
}
