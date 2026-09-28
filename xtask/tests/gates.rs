//! Positive and negative controls for the layer and core-purity gates.
//!
//! Each test copies the valid fixture workspace, injects one defect into a
//! manifest or source file, and asserts the violation categories the gate
//! reports. The valid workspace must pass both gates, so a gate that rejects
//! everything fails these tests as surely as one that accepts everything.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use xtask::metadata::Options;
use xtask::{Violation, check_core_purity, check_layers, layers, purity};

static NEXT: AtomicU32 = AtomicU32::new(0);

const DECLARED: Options = Options {
    resolve: false,
    offline: true,
};
const RESOLVED: Options = Options {
    resolve: true,
    offline: true,
};

struct Fixture(PathBuf);

impl Fixture {
    fn valid() -> Self {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/workspace");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "gate-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&target);
        copy(&source, &target);
        Fixture(target)
    }

    /// Add `line` under `[table]` of `krate`'s manifest.
    fn dep(self, krate: &str, table: &str, line: &str) -> Self {
        let path = self.0.join(krate).join("Cargo.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let header = format!("[{table}]\n");
        let text = if text.contains(&header) {
            text.replacen(&header, &format!("{header}{line}\n"), 1)
        } else {
            format!("{text}\n{header}{line}\n")
        };
        std::fs::write(path, text).unwrap();
        self
    }

    fn write(self, file: &str, text: &str) -> Self {
        std::fs::write(self.0.join(file), text).unwrap();
        self
    }

    fn layers(&self, options: Options) -> Vec<Violation> {
        check_layers(
            &self.0.join("Cargo.toml"),
            &self.0.join("layers.json"),
            options,
        )
        .expect("layer gate must run")
    }

    fn purity(&self, options: Options) -> Vec<Violation> {
        check_core_purity(
            &self.0.join("Cargo.toml"),
            &self.0.join("purity.json"),
            options,
        )
        .expect("purity gate must run")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_name() == "target" || entry.file_name() == "Cargo.lock" {
            continue;
        }
        if path.is_dir() {
            copy(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

fn codes(violations: &[Violation]) -> Vec<&'static str> {
    let mut codes: Vec<_> = violations.iter().map(|v| v.code).collect();
    codes.dedup();
    codes
}

fn only(violations: &[Violation], code: &str) -> Violation {
    assert_eq!(
        violations.len(),
        1,
        "expected exactly one {code}: {violations:#?}"
    );
    assert_eq!(violations[0].code, code, "{violations:#?}");
    violations[0].clone()
}

const ADAPTER: &str = r#"fx-adapter = { path = "../adapter" }"#;

#[test]
fn valid_workspace_passes_both_gates_in_both_modes() {
    let fixture = Fixture::valid();
    for options in [DECLARED, RESOLVED] {
        assert_eq!(fixture.layers(options), vec![]);
        assert_eq!(fixture.purity(options), vec![]);
    }
}

#[test]
fn core_to_adapter_fails_both_gates() {
    let fixture = Fixture::valid().dep("core", "dependencies", ADAPTER);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert_eq!(v.subject, "fx-core (core) -> fx-adapter (adapter)");
    assert!(v.detail.contains("kind=normal"), "{v}");
    assert_eq!(
        only(&fixture.purity(DECLARED), purity::WORKSPACE_EDGE).subject,
        "fx-core -> fx-adapter"
    );
}

#[test]
fn core_to_app_fails() {
    let fixture = Fixture::valid().dep("core", "dependencies", r#"fx-app = { path = "../app" }"#);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert_eq!(v.subject, "fx-core (core) -> fx-app (app)");
}

#[test]
fn port_to_adapter_fails() {
    let fixture = Fixture::valid().dep("ports", "dependencies", ADAPTER);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert_eq!(v.subject, "fx-ports (ports) -> fx-adapter (adapter)");
}

#[test]
fn app_to_concrete_adapter_fails() {
    let fixture = Fixture::valid().dep("app", "dependencies", ADAPTER);
    let v = only(&fixture.layers(RESOLVED), layers::FORBIDDEN_EDGE);
    assert_eq!(v.subject, "fx-app (app) -> fx-adapter (adapter)");
}

#[test]
fn renamed_forbidden_dependency_fails_under_its_real_name() {
    let fixture = Fixture::valid().dep(
        "app",
        "dependencies",
        r#"storage = { package = "fx-adapter", path = "../adapter" }"#,
    );
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert_eq!(v.subject, "fx-app (app) -> fx-adapter (adapter)");
    assert!(v.detail.contains("renamed=storage"), "{v}");
}

#[test]
fn optional_feature_activated_dependency_fails_even_when_inactive() {
    let fixture = Fixture::valid()
        .dep(
            "app",
            "dependencies",
            r#"fx-adapter = { path = "../adapter", optional = true }"#,
        )
        .dep("app", "features", r#"disk = ["dep:fx-adapter"]"#);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert!(v.detail.contains("optional=true"), "{v}");
}

#[test]
fn target_specific_forbidden_dependency_fails() {
    let fixture = Fixture::valid().dep("core", "target.'cfg(unix)'.dependencies", ADAPTER);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert!(v.detail.contains("target=cfg(unix)"), "{v}");
}

#[test]
fn build_forbidden_dependency_fails() {
    let fixture = Fixture::valid().dep("ports", "build-dependencies", ADAPTER);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert!(v.detail.contains("kind=build"), "{v}");
}

#[test]
fn dev_forbidden_dependency_fails() {
    let fixture = Fixture::valid().dep("app", "dev-dependencies", ADAPTER);
    let v = only(&fixture.layers(DECLARED), layers::FORBIDDEN_EDGE);
    assert!(v.detail.contains("kind=dev"), "{v}");
}

#[test]
fn forbidden_edge_through_a_non_member_is_caught_only_with_resolution() {
    let fixture = Fixture::valid().dep(
        "app",
        "dependencies",
        r#"fx-helper = { path = "../helper" }"#,
    );
    // Declared metadata sees only a crate outside the workspace.
    assert_eq!(fixture.layers(DECLARED), vec![]);
    let v = only(&fixture.layers(RESOLVED), layers::FORBIDDEN_TRANSITIVE_EDGE);
    assert_eq!(v.subject, "fx-app (app) -> fx-adapter (adapter)");
    assert_eq!(v.detail, "via fx-helper -> fx-adapter");
}

#[test]
fn denied_crate_reached_through_an_unreviewed_wrapper_is_caught_with_resolution() {
    // `fake-tokio` is a local path package named `tokio`, so the resolved
    // check runs offline; the gate matches the package name, as it would for
    // the registry crate.
    let fixture = Fixture::valid().dep(
        "core",
        "dependencies",
        r#"fx-wrapper = { path = "../wrapper" }"#,
    );
    only(&fixture.purity(DECLARED), purity::UNREVIEWED_DEPENDENCY);
    let resolved = fixture.purity(RESOLVED);
    assert_eq!(
        codes(&resolved),
        vec![purity::TRANSITIVE_DENIED, purity::UNREVIEWED_DEPENDENCY],
        "{resolved:#?}"
    );
    let denied = resolved
        .iter()
        .find(|v| v.code == purity::TRANSITIVE_DENIED)
        .unwrap();
    assert_eq!(
        denied.detail,
        "category=async-runtime; via fx-core -> fx-wrapper -> tokio"
    );
}

#[test]
fn unassigned_member_and_stale_policy_entry_fail() {
    let fixture = Fixture::valid().write(
        "layers.json",
        &std::fs::read_to_string(Fixture::valid().0.join("layers.json"))
            .unwrap()
            .replace(
                r#""fx-root": "composition-root""#,
                r#""fx-gone": "composition-root""#,
            ),
    );
    // The unassigned crate's own edges are forbidden too: no layer allows them.
    assert_eq!(
        codes(&fixture.layers(DECLARED)),
        vec![
            layers::FORBIDDEN_EDGE,
            layers::STALE_POLICY_ENTRY,
            layers::UNASSIGNED_CRATE
        ]
    );
}

#[test]
fn core_depending_on_randomness_is_denied_by_category() {
    let fixture = Fixture::valid().dep("core", "dependencies", r#"rand = "0.9""#);
    let v = only(&fixture.purity(DECLARED), purity::DENIED_DEPENDENCY);
    assert!(v.detail.starts_with("category=randomness"), "{v}");
}

#[test]
fn core_depending_on_async_runtime_is_denied_by_category() {
    let fixture = Fixture::valid().dep("core", "dependencies", r#"tokio = "1""#);
    let v = only(&fixture.purity(DECLARED), purity::DENIED_DEPENDENCY);
    assert!(v.detail.starts_with("category=async-runtime"), "{v}");
}

#[test]
fn core_depending_on_http_client_fails_even_renamed() {
    let fixture = Fixture::valid().dep(
        "core",
        "dependencies",
        r#"client = { package = "ureq", version = "3" }"#,
    );
    let v = only(&fixture.purity(DECLARED), purity::DENIED_DEPENDENCY);
    assert!(v.detail.starts_with("category=http-client"), "{v}");
}

#[test]
fn unreviewed_pure_dependency_fails_and_allowlisted_one_passes() {
    let unreviewed = Fixture::valid().dep("core", "dependencies", r#"serde = "1""#);
    only(&unreviewed.purity(DECLARED), purity::UNREVIEWED_DEPENDENCY);
    let allowed = Fixture::valid().dep("core", "dependencies", r#"libm = "0.2""#);
    assert_eq!(allowed.purity(DECLARED), vec![]);
    let dev = Fixture::valid().dep("core", "dev-dependencies", r#"libm = "0.2""#);
    only(&dev.purity(DECLARED), purity::UNREVIEWED_DEPENDENCY);
}

#[test]
fn core_build_script_fails() {
    let fixture = Fixture::valid().write("core/build.rs", "fn main() {}\n");
    only(&fixture.purity(DECLARED), purity::BUILD_SCRIPT);
}

#[test]
fn core_feature_fails() {
    let fixture = Fixture::valid().dep("core", "features", r#"std = []"#);
    only(&fixture.purity(DECLARED), purity::FEATURE);
}

#[test]
fn removing_no_std_or_reenabling_std_fails() {
    let lib = "#![forbid(unsafe_code)]\n\npub fn decide(x: u8) -> u8 {\n    x\n}\n";
    let missing = Fixture::valid().write("core/src/lib.rs", lib);
    only(&missing.purity(DECLARED), purity::MISSING_ATTRIBUTE);
    let reenabled = Fixture::valid().write(
        "core/src/lib.rs",
        &format!("#![no_std]\nextern crate std;\n{lib}"),
    );
    only(&reenabled.purity(DECLARED), purity::AMBIENT_SOURCE);
    let commented = Fixture::valid().write(
        "core/src/lib.rs",
        &format!("#![no_std]\n// extern crate std; is forbidden\n{lib}"),
    );
    assert_eq!(commented.purity(DECLARED), vec![]);
}

#[test]
fn global_mutable_state_fails() {
    let fixture = Fixture::valid().write(
        "core/src/state.rs",
        "use core::sync::atomic::AtomicU64;\npub static SEEN: AtomicU64 = AtomicU64::new(0);\n",
    );
    let violations = fixture.purity(DECLARED);
    assert_eq!(
        codes(&violations),
        vec![purity::AMBIENT_SOURCE],
        "{violations:#?}"
    );
    assert!(violations.iter().any(|v| v.detail.contains("state.rs:2")));
}

#[test]
fn a_check_that_cannot_run_is_an_error_not_a_pass() {
    let fixture = Fixture::valid().write("core/Cargo.toml", "this is not toml");
    assert!(
        check_layers(
            &fixture.0.join("Cargo.toml"),
            &fixture.0.join("layers.json"),
            DECLARED
        )
        .is_err()
    );
}

#[test]
fn repository_policies_parse_and_name_real_members() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    type Gate = fn(&Path, &Path, Options) -> Result<Vec<Violation>, String>;
    let gates: [(Gate, &str); 2] = [
        (check_layers, "docs/architecture/layers.json"),
        (check_core_purity, "docs/architecture/core-purity.json"),
    ];
    for (gate, policy) in gates {
        let violations = gate(&root.join("Cargo.toml"), &root.join(policy), DECLARED)
            .expect("repository gate must run");
        assert_eq!(violations, vec![], "{policy}");
    }
}

#[test]
fn a_dev_only_exception_does_not_excuse_a_normal_edge() {
    let policy = std::fs::read_to_string(Fixture::valid().0.join("layers.json"))
        .unwrap()
        .replace(
            r#""exceptions": []"#,
            r#""exceptions": [{"from": "fx-app", "to": "fx-adapter", "kinds": ["dev"], "reason": "fixture"}]"#,
        );
    let dev =
        Fixture::valid()
            .write("layers.json", &policy)
            .dep("app", "dev-dependencies", ADAPTER);
    assert_eq!(dev.layers(DECLARED), vec![]);
    let normal = Fixture::valid()
        .write("layers.json", &policy)
        .dep("app", "dependencies", ADAPTER);
    only(&normal.layers(DECLARED), layers::FORBIDDEN_EDGE);
}
