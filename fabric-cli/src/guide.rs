use clap::Args;
use fabric_core::guide::{CATALOG_BYTES, Catalog, Evidence, MAX_GUIDE_BYTES, ProofKind, Topic};
use rustix::fs::{Mode, OFlags, open};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
pub(super) struct GuideArgs {
    /// Limit the guide to one topic. Omit to list available topics.
    topic: Option<String>,
    /// Show recorded results from a bounded candidate verification JSON file.
    #[arg(long, value_name = "FILE")]
    evidence: Option<PathBuf>,
}

#[derive(Serialize)]
struct Report<'a> {
    catalog_schema: u32,
    catalog_sha256: String,
    build_revision: Option<&'static str>,
    topics: Vec<TopicReport<'a>>,
    evidence: Option<EvidenceReport>,
}

#[derive(Serialize)]
struct TopicReport<'a> {
    id: &'a str,
    title: &'a str,
    summary: &'a str,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    detail: Option<TopicDetail<'a>>,
}

#[derive(Serialize)]
struct TopicDetail<'a> {
    scenarios: &'a [fabric_core::guide::Scenario],
    documentation: &'a [String],
    proofs: &'a [fabric_core::guide::Proof],
}

#[derive(Serialize)]
struct EvidenceReport {
    candidate_matches_build: Option<bool>,
    catalog_matches_build: bool,
    recorded: Evidence,
}

fn load_evidence(path: &Path) -> Result<Evidence, String> {
    let input = open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("open guide evidence {}: {error}", path.display()))?;
    let input = File::from(input);
    let metadata = input.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("guide evidence must be a regular file".into());
    }
    if metadata.len() > MAX_GUIDE_BYTES as u64 {
        return Err("guide evidence exceeds 256 KiB".into());
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_GUIDE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Evidence::parse(&bytes)
}

fn compare_evidence(
    recorded: Evidence,
    build_revision: Option<&str>,
    digest: &str,
) -> EvidenceReport {
    EvidenceReport {
        candidate_matches_build: build_revision.map(|id| id == recorded.candidate),
        catalog_matches_build: digest == recorded.catalog_sha256,
        recorded,
    }
}

pub(super) fn run(args: &GuideArgs, format: super::OutputFormat) -> Result<(), String> {
    if format == super::OutputFormat::Arrow {
        return Err("fabric guide supports JSON and text output".into());
    }
    let catalog = Catalog::parse(CATALOG_BYTES)?;
    let selected = args
        .topic
        .as_ref()
        .map(|id| {
            catalog
                .topics
                .iter()
                .find(|topic| &topic.id == id)
                .ok_or_else(|| {
                    format!("unknown guide topic '{id}'; run 'fabric guide' to list topics")
                })
        })
        .transpose()?;
    let digest = format!("{:x}", Sha256::digest(CATALOG_BYTES));
    let build_revision = option_env!("FABRIC_BUILD_REVISION")
        .filter(|id| fabric_core::guide::is_candidate_revision(id));
    let evidence = args
        .evidence
        .as_deref()
        .map(load_evidence)
        .transpose()?
        .map(|recorded| compare_evidence(recorded, build_revision, &digest));
    let report = Report {
        catalog_schema: catalog.schema,
        catalog_sha256: digest,
        build_revision,
        topics: if let Some(topic) = selected {
            vec![topic_report(topic, true)]
        } else {
            catalog
                .topics
                .iter()
                .map(|topic| topic_report(topic, false))
                .collect()
        },
        evidence,
    };
    if format == super::OutputFormat::Json {
        super::emit_json("guide", &report)
    } else {
        write_text(std::io::stdout().lock(), &report).map_err(|error| error.to_string())
    }
}

fn topic_report(topic: &Topic, detail: bool) -> TopicReport<'_> {
    TopicReport {
        id: &topic.id,
        title: &topic.title,
        summary: &topic.summary,
        detail: detail.then_some(TopicDetail {
            scenarios: &topic.scenarios,
            documentation: &topic.documentation,
            proofs: &topic.proofs,
        }),
    }
}

fn write_text(mut out: impl Write, report: &Report<'_>) -> std::io::Result<()> {
    for topic in &report.topics {
        writeln!(out, "{}  {}", topic.id, topic.title)?;
        writeln!(out, "  {}", topic.summary)?;
        if let Some(detail) = &topic.detail {
            for scenario in detail.scenarios {
                writeln!(out, "\n  {}", scenario.description)?;
                writeln!(out, "    {}", scenario.command)?;
                writeln!(out, "    {}", scenario.expected)?;
                writeln!(out, "    Declared checks: {}", scenario.checks.join(", "))?;
            }
            for proof in detail.proofs {
                let kind = match proof.kind {
                    ProofKind::ExtractedRust => "Extracted Rust",
                    ProofKind::IndependentModel => "Independent model",
                };
                writeln!(out, "\n  {kind}: {} ({})", proof.theorem, proof.path)?;
                writeln!(out, "    {}", proof.scope)?;
                writeln!(out, "    Assumptions: {}", proof.assumptions)?;
                writeln!(out, "    Reproduce: cargo xtask prove {}", proof.checker)?;
            }
            writeln!(
                out,
                "\n  Documentation: {}",
                detail.documentation.join(", ")
            )?;
            writeln!(out, "  Run declared tests: cargo xtask coverage check")?;
        }
    }
    if let Some(evidence) = &report.evidence {
        use fabric_core::guide::CheckStatus;

        let candidate_relation = match evidence.candidate_matches_build {
            Some(true) => "matches the stated build revision",
            Some(false) => "differs from the stated build revision",
            None => "build revision unavailable",
        };
        let catalog_relation = if evidence.catalog_matches_build {
            "matches"
        } else {
            "differs"
        };
        writeln!(
            out,
            "\nRecorded results for {} ({candidate_relation}, catalog {catalog_relation})",
            evidence.recorded.candidate
        )?;
        for check in &evidence.recorded.checks {
            let status = match check.status {
                CheckStatus::Passed => "passed",
                CheckStatus::Failed => "failed",
            };
            writeln!(out, "  {}  {}", check.id, status)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::guide::{CheckOutcome, CheckStatus, Checker, SCHEMA};

    #[test]
    fn evidence_distinguishes_missing_build_revision_from_mismatch() {
        let evidence = || Evidence {
            schema: SCHEMA,
            candidate: "a".repeat(40),
            catalog_sha256: "b".repeat(64),
            manifest_sha256: "c".repeat(64),
            checks: vec![CheckOutcome {
                id: "lifecycle".into(),
                status: CheckStatus::Passed,
            }],
            checkers: vec![Checker {
                id: "rustc".into(),
                identity: "rustc 1.100.0".into(),
            }],
            artifacts: vec!["coverage.json".into()],
        };
        let matching = compare_evidence(evidence(), Some(&"a".repeat(40)), &"b".repeat(64));
        assert_eq!(matching.candidate_matches_build, Some(true));
        assert!(matching.catalog_matches_build);
        let stale = compare_evidence(evidence(), Some(&"f".repeat(40)), &"b".repeat(64));
        assert_eq!(stale.candidate_matches_build, Some(false));
        let unknown = compare_evidence(evidence(), None, &"f".repeat(64));
        assert_eq!(unknown.candidate_matches_build, None);
        assert!(!unknown.catalog_matches_build);
    }

    #[test]
    fn guide_index_omits_detail_and_topic_includes_proof_scope() {
        let catalog = Catalog::parse(CATALOG_BYTES).unwrap();
        let topic = catalog
            .topics
            .iter()
            .find(|topic| topic.id == "local")
            .unwrap();
        let index = serde_json::to_value(topic_report(topic, false)).unwrap();
        assert!(index.get("scenarios").is_none());
        let detail = serde_json::to_value(topic_report(topic, true)).unwrap();
        assert_eq!(detail["proofs"][0]["kind"], "extracted_rust");
        assert!(
            !detail["proofs"][0]["assumptions"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
}
