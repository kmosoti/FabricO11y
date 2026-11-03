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
    super::run(binary, &["registry", "check", "-r", "registry/model"], root)?;
    let generated = tempfile::tempdir().map_err(|e| e.to_string())?;
    super::run(
        binary,
        &[
            "registry",
            "generate",
            "rust",
            generated.path().to_str().ok_or("non-UTF8 output")?,
            "-r",
            "registry/model",
            "-t",
            "registry/templates",
        ],
        root,
    )?;
    super::run(
        "rustfmt",
        &[
            "--edition",
            "2024",
            generated
                .path()
                .join("vocabulary.rs")
                .to_str()
                .ok_or("non-UTF8 output")?,
        ],
        root,
    )?;
    for name in ["vocabulary.rs", "descriptor.json"] {
        let bytes = fs::read(generated.path().join(name)).map_err(|e| e.to_string())?;
        let target = root.join("registry").join(name);
        if generate {
            fs::write(target, bytes).map_err(|e| e.to_string())?;
        } else if fs::read(&target).map_err(|e| e.to_string())? != bytes {
            return Err(format!(
                "registry artifact drift: {}; run cargo xtask registry generate",
                target.display()
            ));
        }
    }
    println!("Pinned registry and generated artifacts verified");
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
}
