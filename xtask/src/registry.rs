//! Pinned semantic registry validation and reproducible generated artifact checks.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
const BINARY_HASH: &str = "6c67e08ba8abb2c60cf8a58c299b5db460c0fbe7226f7453d4878dcc4f6c3867";
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
    println!("Pinned v2 and v3 registries and generated artifacts verified");
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
    let generated = tempfile::tempdir().map_err(|e| e.to_string())?;
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
        let temp = tempfile::tempdir().unwrap();
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
        let temp = tempfile::tempdir().unwrap();
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
    fn explicit_generation_cannot_redefine_v2() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let temp = tempfile::tempdir().unwrap();
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
