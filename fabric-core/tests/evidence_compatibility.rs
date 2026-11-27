//! Compile through old external paths and require identical shared types.
use fabric_core::{application_result as old_app, evidence_manifest as old_manifest};
use fabric_evidence::{application_result as new_app, evidence_manifest as new_manifest};

fn to_new(value: fabric_core::ProvenancedTime) -> new_manifest::ProvenancedTime {
    value
}
fn to_old(value: new_manifest::OriginalArtifactDigest) -> old_manifest::OriginalArtifactDigest {
    value
}
fn app_type(value: old_app::ApplicationResult) -> new_app::ApplicationResult {
    value
}
fn app_back(value: new_app::ApplicationResult) -> fabric_core::ApplicationResult {
    value
}

#[test]
fn old_paths_share_new_types_and_original_bytes() {
    let clock = old_manifest::ProvenancedTime {
        unix_ms: 42,
        clock: old_manifest::ClockProvenance {
            clock_id: "clock".into(),
            source: "producer".into(),
            uncertainty_ms: None,
        },
    };
    let scope: new_app::ReportScope =
        fabric_core::ReportScope::new("origin".into(), "producer".into()).unwrap();
    let source = b"{\"label\":\"caf\\u00e9\"}\n";
    let report = fabric_core::ApplicationResult::new(
        scope,
        "aaaaaaaa-0000-4000-8000-000000000001".into(),
        None,
        "example.result.v1".into(),
        to_new(clock),
        source,
    )
    .unwrap();
    let report = app_back(app_type(report));
    let digest = to_old(report.original_artifact_digest().clone());
    assert_eq!(
        digest.0.hex,
        "426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d"
    );
    assert_eq!(
        report.application_payload_digest().0.hex,
        "2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075"
    );
    digest.verify_exact_bytes(source).unwrap();
}
