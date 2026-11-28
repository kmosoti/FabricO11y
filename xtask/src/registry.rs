//! Pinned semantic registry validation and reproducible generated artifact checks.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
const BINARY_HASH: &str = "6c67e08ba8abb2c60cf8a58c299b5db460c0fbe7226f7453d4878dcc4f6c3867";
const V4_FIXTURES_HASH: &str = "f6dc07a4bbf45a4055c58634a890821504716e63a4e5c4b8cfb0834a380d3030";
const V5_FIXTURES_HASH: &str = "5d17ba82aa58cafa3173e8105d8de3d545c4cf4d99a656caceca0ca3395f91f7";
pub fn check(root: &Path, generate: bool) -> Result<(), String> {
    let binary = env::var_os("FABRIC_WEAVER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let base = env::var_os("FABRIC_SCHEMA_TOOLS")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env::var_os("HOME").unwrap_or_default())
                        .join(".local/share/fabrico11y/schema-tools")
                });
            base.join("weaver-0.28.0")
        });
    if !binary.is_file() {
        return Err("pinned Weaver missing: run sh .github/scripts/install-schema-tools.sh".into());
    }
    if super::evidence::hash(&binary)? != BINARY_HASH {
        return Err("unrecognized Weaver binary; expected pinned Linux x86_64 0.28.0".into());
    }
    let binary = binary.to_str().ok_or("non-UTF8 Weaver path")?;
    if super::output(binary, &["--version"], root)? != "weaver 0.28.0" {
        return Err("Weaver version mismatch".into());
    }
    // Revision 2 is already an immutable custody schema. Explicit generation
    // must never redefine it under the retained registry identity.
    for (path, expected) in V2_IMMUTABLE {
        if super::evidence::hash(&root.join(path))? != *expected {
            return Err(format!("immutable v2 registry artifact drift: {path}"));
        }
    }
    if super::evidence::hash(&root.join("registry/v4/fixtures.json"))? != V4_FIXTURES_HASH {
        return Err(
            "v4 wire fixtures drifted; review fixture updates and change their pinned hash".into(),
        );
    }
    if super::evidence::hash(&root.join("registry/v5/fixtures.json"))? != V5_FIXTURES_HASH {
        return Err(
            "v5 wire fixtures drifted; review fixture updates and change their pinned hash".into(),
        );
    }
    verify_revision(
        binary,
        root,
        "registry/model",
        "registry/templates",
        "registry",
        &["vocabulary.rs", "descriptor.json"],
        false,
    )?;
    verify_revision(
        binary,
        root,
        "registry/v3/model",
        "registry/v3/templates",
        "registry/v3",
        &["descriptor.json"],
        generate,
    )?;
    let v3_descriptor_hash = super::evidence::hash(&root.join("registry/v3/descriptor.json"))?;
    let v4_template =
        fs::read_to_string(root.join("registry/v4/templates/registry/rust/descriptor.json.j2"))
            .map_err(|e| e.to_string())?;
    if !v4_template.contains(&format!(
        "\"extends_descriptor_sha256\":\"{v3_descriptor_hash}\""
    )) {
        return Err("v4 descriptor template must extend the exact v3 descriptor".into());
    }
    verify_revision(
        binary,
        root,
        "registry/v4/model",
        "registry/v4/templates",
        "registry/v4",
        &["descriptor.json"],
        generate,
    )?;
    let v5_template =
        fs::read_to_string(root.join("registry/v5/templates/registry/rust/descriptor.json.j2"))
            .map_err(|e| e.to_string())?;
    if !v5_template.contains(&format!(
        "\"extends_descriptor_sha256\":\"{v3_descriptor_hash}\""
    )) {
        return Err("v5 descriptor template must extend the exact v3 descriptor".into());
    }
    verify_revision(
        binary,
        root,
        "registry/v5/model",
        "registry/v5/templates",
        "registry/v5",
        &["descriptor.json"],
        generate,
    )?;
    println!("Pinned v2 and generated v3/v4/v5 registries and artifacts verified");
    Ok(())
}

const V2_IMMUTABLE: &[(&str, &str)] = &[
    (
        "registry/model/manifest.yaml",
        "b0da9c8d29bc163c6c23abd1c0f8231402b4ef67b6f34a33f59a546a94931f6c",
    ),
    (
        "registry/model/observations.yaml",
        "912c37f64eac2dc620be26a7f688b596fef3145b2f9fee152e975f08f722b628",
    ),
    (
        "registry/vocabulary.rs",
        "7863ded48669bd3f2ef5ca3d709ae0e1adf77566525c744f47a302e0ee84007e",
    ),
    (
        "registry/descriptor.json",
        "8676053c09e04c426e130a883a529e4fc42dea96e3f850bc5ee545f374eb5f11",
    ),
    (
        "registry/templates/registry/rust/descriptor.json.j2",
        "9c7bb3ac1f0ffbd2d76b6932e836dc061f0f855275af9136e38fb81c5cc72b28",
    ),
    (
        "registry/templates/registry/rust/vocabulary.rs.j2",
        "b581de37bd9a2d21a2d3a3829666a00c922dcb08b332568d62700bd13930da26",
    ),
    (
        "registry/templates/registry/rust/weaver.yaml",
        "8fa469b92c500e4099f68afd361661e7a91a2dbf6beb994d03f5ad3fc5a87121",
    ),
];
fn verify_revision(
    binary: &str,
    root: &Path,
    model: &str,
    templates: &str,
    target: &str,
    names: &[&str],
    generate: bool,
) -> Result<(), String> {
    super::run(binary, &["registry", "check", "-r", model], root)?;
    let generated = super::evidence::non_tmp_tempdir()?;
    super::run(
        binary,
        &[
            "registry",
            "generate",
            "rust",
            generated.path().to_str().ok_or("non-UTF8 output")?,
            "-r",
            model,
            "-t",
            templates,
        ],
        root,
    )?;
    for name in names {
        if name.ends_with(".rs") {
            super::run(
                "rustfmt",
                &[
                    "--edition",
                    "2024",
                    generated
                        .path()
                        .join(name)
                        .to_str()
                        .ok_or("non-UTF8 generated source")?,
                ],
                root,
            )?;
        }
        let bytes = fs::read(generated.path().join(name)).map_err(|e| e.to_string())?;
        let target = root.join(target).join(name);
        if generate {
            fs::write(target, bytes).map_err(|e| e.to_string())?;
        } else if fs::read(&target).map_err(|e| e.to_string())? != bytes {
            return Err(format!(
                "registry artifact drift: {}; run cargo xtask registry generate",
                target.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn copy(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    #[test]
    fn rejects_drift_and_invalid_registry() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        check(temp.path(), false).unwrap();
        fs::write(temp.path().join("registry/vocabulary.rs"), "// drift\n").unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        fs::copy(
            original.join("registry/vocabulary.rs"),
            temp.path().join("registry/vocabulary.rs"),
        )
        .unwrap();
        fs::write(
            temp.path().join("registry/model/observations.yaml"),
            "groups: [{id: broken, type: not_a_group}]\n",
        )
        .unwrap();
        assert!(check(temp.path(), false).is_err());
    }
    #[test]
    fn v3_artifact_and_valid_model_drift_are_rejected() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        let descriptor = temp.path().join("registry/v3/descriptor.json");
        let original_descriptor = fs::read(&descriptor).unwrap();
        let mut corrupted = original_descriptor.clone();
        corrupted.push(b' ');
        fs::write(&descriptor, &corrupted).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        check(temp.path(), true).unwrap();
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
        let model = temp.path().join("registry/v3/model/observations.yaml");
        let original_model = fs::read_to_string(&model).unwrap();
        let changed = original_model.replace(
            "cross-process run correlation.",
            "changed cross-process run correlation.",
        );
        assert_ne!(changed, original_model);
        fs::write(&model, changed).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
        fs::write(&model,format!("{original_model}\n  - id: registry.fabric.v3.extra\n    type: attribute_group\n    brief: Additional semantics.\n    stability: development\n    attributes:\n      - id: fabric.extra\n        type: boolean\n        brief: Additional field.\n        stability: development\n")).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        fs::write(&model, "groups: [{id: broken, type: not_a_group}]\n").unwrap();
        assert!(check(temp.path(), true).is_err());
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
    }

    #[test]
    fn v4_descriptor_extends_the_exact_v3_contract_and_bounds() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let v3_descriptor = original.join("registry/v3/descriptor.json");
        let digest = crate::evidence::hash(&v3_descriptor).unwrap();
        assert_eq!(
            digest,
            "266e82b0a89f40f0136aa988fb642c48ce0cc7b916b9c0dd0959596c4542fd5e"
        );
        let descriptor: serde_json::Value = serde_json::from_slice(
            &fs::read(original.join("registry/v4/descriptor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(descriptor["registry"], "fabrico11y/4");
        assert_eq!(descriptor["version"], 4);
        assert_eq!(descriptor["extends"], "fabrico11y/3");
        assert_eq!(descriptor["extends_descriptor_sha256"], digest);
        assert_eq!(
            descriptor["context"],
            serde_json::json!({
                "required": ["run_id"],
                "run_id": "globally unique nonzero lowercase UUID",
                "data_run_id": "agent_run and tool_execution data.run_id must equal context.run_id"
            })
        );
        assert_eq!(descriptor["wire"]["max_bytes"], 65536);
        assert_eq!(descriptor["wire"]["max_depth"], 16);
        assert_eq!(
            descriptor["span"]["required"],
            serde_json::json!(["trace_id", "span_id"])
        );
        assert_eq!(
            descriptor["span"]["data_kinds"],
            serde_json::json!(["service_request", "agent_run", "tool_execution"])
        );
        assert_eq!(
            descriptor["span"]["service_request_trace_id"],
            "if present, data.trace_id must equal span.trace_id"
        );
    }

    #[test]
    fn v4_artifact_and_valid_model_drift_are_rejected() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        check(temp.path(), false).unwrap();

        let descriptor = temp.path().join("registry/v4/descriptor.json");
        let original_descriptor = fs::read(&descriptor).unwrap();
        let mut corrupted = original_descriptor.clone();
        corrupted.push(b' ');
        fs::write(&descriptor, &corrupted).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        check(temp.path(), true).unwrap();
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);

        let model = temp.path().join("registry/v4/model/observations.yaml");
        let original_model = fs::read_to_string(&model).unwrap();
        let changed =
            original_model.replace("at most one parent", "exactly one parent when present");
        assert_ne!(changed, original_model);
        fs::write(&model, changed).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
    }

    #[test]
    fn v4_fixture_drift_is_rejected_before_generation() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        let descriptor = temp.path().join("registry/v4/descriptor.json");
        let original_descriptor = fs::read(&descriptor).unwrap();
        let fixtures = temp.path().join("registry/v4/fixtures.json");
        let mut changed = fs::read(&fixtures).unwrap();
        changed.push(b' ');
        fs::write(&fixtures, changed).unwrap();
        assert!(
            check(temp.path(), true)
                .unwrap_err()
                .contains("v4 wire fixtures drifted")
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
    }

    #[test]
    fn v5_descriptor_extends_exact_v3_metric_contract_and_bounds() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let v3: serde_json::Value = serde_json::from_slice(
            &fs::read(original.join("registry/v3/descriptor.json")).unwrap(),
        )
        .unwrap();
        let v5: serde_json::Value = serde_json::from_slice(
            &fs::read(original.join("registry/v5/descriptor.json")).unwrap(),
        )
        .unwrap();
        let digest = crate::evidence::hash(&original.join("registry/v3/descriptor.json")).unwrap();
        assert_eq!(v5["registry"], "fabrico11y/5");
        assert_eq!(v5["version"], 5);
        assert_eq!(v5["extends"], "fabrico11y/3");
        assert_eq!(v5["extends_descriptor_sha256"], digest);
        assert_eq!(v5["context"], v3["context"]);
        assert_eq!(v5["generation"], v3["generation"]);
        assert_eq!(v5["wire"], v3["wire"]);
        assert_eq!(v5["metric_kinds"], serde_json::json!(["metric"]));
        assert_eq!(
            v5["labels"],
            serde_json::json!({
                "field": "data.labels",
                "required": false,
                "accepted_when_omitted_or_empty": true,
                "type": "object mapping decoded string keys to decoded UTF-8 string values",
                "max_entries": 8,
                "key": {
                    "min_decoded_ascii_bytes": 1,
                    "max_decoded_ascii_bytes": 64,
                    "pattern": "^[A-Za-z_][A-Za-z0-9_.-]*$",
                    "ascii_only": true,
                },
                "value": {
                    "min_decoded_utf8_bytes": 0,
                    "max_decoded_utf8_bytes": 128,
                    "reject_unicode_control": true,
                    "reject_nul": true,
                },
                "duplicates": "reject duplicate decoded keys, including JSON escape aliases",
                "reserved_keys": ["producer", "generation", "sequence", "metric_unit", "metric_type", "reset", "context_run_id", "version", "labels"],
                "projection": "preserve labels as the nested data.labels field; do not flatten keys into sibling fields",
            })
        );
    }

    #[test]
    fn v5_artifact_and_valid_model_drift_are_rejected() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        check(temp.path(), false).unwrap();

        let descriptor = temp.path().join("registry/v5/descriptor.json");
        let original_descriptor = fs::read(&descriptor).unwrap();
        let mut corrupted = original_descriptor.clone();
        corrupted.push(b' ');
        fs::write(&descriptor, &corrupted).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        check(temp.path(), true).unwrap();
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);

        let model = temp.path().join("registry/v5/model/observations.yaml");
        let original_model = fs::read_to_string(&model).unwrap();
        let changed = original_model.replace("At most eight entries", "At most seven entries");
        assert_ne!(changed, original_model);
        fs::write(&model, changed).unwrap();
        assert!(
            check(temp.path(), false)
                .unwrap_err()
                .contains("artifact drift")
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
    }

    #[test]
    fn v5_fixture_drift_is_rejected_before_generation() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        let descriptor = temp.path().join("registry/v5/descriptor.json");
        let original_descriptor = fs::read(&descriptor).unwrap();
        let fixtures = temp.path().join("registry/v5/fixtures.json");
        let mut changed = fs::read(&fixtures).unwrap();
        changed.push(b' ');
        fs::write(&fixtures, changed).unwrap();
        assert!(
            check(temp.path(), true)
                .unwrap_err()
                .contains("v5 wire fixtures drifted")
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
    }

    #[test]
    fn explicit_generation_cannot_redefine_v2() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = crate::evidence::non_tmp_tempdir().unwrap();
        copy(&original.join("registry"), &temp.path().join("registry"));
        let model = temp.path().join("registry/model/observations.yaml");
        let changed = fs::read_to_string(&model).unwrap().replace(
            "Versioned local observations and custody identities.",
            "Changed custody semantics.",
        );
        fs::write(&model, changed).unwrap();
        assert!(
            check(temp.path(), true)
                .unwrap_err()
                .contains("immutable v2")
        );
        assert_eq!(
            fs::read(temp.path().join("registry/descriptor.json")).unwrap(),
            fs::read(original.join("registry/descriptor.json")).unwrap()
        );
    }
}
