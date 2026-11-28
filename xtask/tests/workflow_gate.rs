use std::{fs, path::Path};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn main_human_gate_and_ci_workflows_enforce_promotion_invariants() {
    let gate_path = repo_root().join(".github/workflows/main-human-gate.yml");
    let gate = fs::read_to_string(&gate_path).expect("read main-human-gate.yml");

    for required in [
        "pull_request:",
        "branches: [main]",
        "opened, synchronize, reopened, ready_for_review, edited",
        "permissions:\n  contents: read\n  pull-requests: read",
        "name: verify-promotion-source",
        "name: main-human-approval",
        "needs: [verify-promotion-source]",
        "environment: main-release",
        "pull_request.base.sha",
        "pull_request.head.sha",
        "git/ref/heads/main",
        "git/ref/heads/develop",
        "git/commits/$GITHUB_SHA",
        "steps.freeze.outputs.candidate-digest",
        "needs.verify-promotion-source.outputs.candidate-digest",
        "PROMOTION_PHASE: approved",
        "approved) test \"$digest\" = \"$APPROVED_DIGEST\"",
        "run: *check-candidate",
    ] {
        assert!(gate.contains(required), "missing gate contract: {required}");
    }

    for forbidden in [
        "pull_request_target:",
        "push:",
        "workflow_dispatch:",
        "actions/checkout",
    ] {
        assert!(
            !gate.contains(forbidden),
            "unexpected gate behavior: {forbidden}"
        );
    }
    assert!(
        !repo_root()
            .join(".github/workflows/main-promotion-source.yml")
            .exists()
    );

    let dev_path = repo_root().join(".github/workflows/development-checks.yml");
    let dev = fs::read_to_string(&dev_path).expect("read development-checks.yml");
    for required in [
        "branches: [main, develop, 'stack/**']",
        "opened, synchronize, reopened, ready_for_review, edited",
        "push:\n    branches: [develop]",
        "development-checks-${{ github.event.pull_request.number || github.ref }}",
        "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
        "CARGO_BUILD_JOBS: '4'",
        "LEAN_NUM_THREADS: '4'",
        "FABRIC_FUZZ_TARGET_DIR: /github/home/.cache/fabrico11y-fuzz-target",
        "FABRIC_FUZZ_SMOKE_SECONDS: ${{ github.event_name == 'pull_request' && '15' || '60' }}",
        "verify:",
        "native-acceptance:",
        "timeout-minutes:",
        "./.github/actions/restore-ci-cache",
        "./.github/actions/save-ci-cache",
        "github.event_name == 'push' && github.ref == 'refs/heads/develop'",
        r#"GIT_CONFIG_GLOBAL="$config" git cat-file commit "$1""#,
        "predecessor_parents=$(parents_of",
        "PR_BASE_SHA",
        "PR_HEAD_SHA",
    ] {
        assert!(
            dev.contains(required),
            "missing development workflow contract: {required}"
        );
    }
    assert_eq!(dev.matches("ref: ${{ github.sha }}").count(), 2);
    assert_eq!(dev.matches("persist-credentials: false").count(), 2);
    assert_eq!(dev.matches("fetch-depth: 2").count(), 2);

    let restore =
        fs::read_to_string(repo_root().join(".github/actions/restore-ci-cache/action.yml"))
            .expect("read restore-ci-cache action");
    let save = fs::read_to_string(repo_root().join(".github/actions/save-ci-cache/action.yml"))
        .expect("read save-ci-cache action");
    const CACHE_ACTION: &str = "55cc8345863c7cc4c66a329aec7e433d2d1c52a9";
    assert_eq!(
        restore
            .matches(&format!("actions/cache/restore@{CACHE_ACTION}"))
            .count(),
        3
    );
    assert_eq!(
        save.matches(&format!("actions/cache/save@{CACHE_ACTION}"))
            .count(),
        3
    );
    assert_eq!(
        save.matches("github.event_name == 'push' && github.ref == 'refs/heads/develop'")
            .count(),
        3
    );

    for cached_path in [
        "/github/home/.cargo/registry",
        "/github/home/.cargo/git/db",
        "path: target",
        "/github/home/.cache/fabrico11y-fuzz-target",
    ] {
        assert!(
            restore.contains(cached_path),
            "restore-ci-cache missing {cached_path}"
        );
        assert!(
            save.contains(cached_path),
            "save-ci-cache missing {cached_path}"
        );
    }
    for tool_bundle in ["schema-tools", "engine-tools", "proof-tools"] {
        assert!(!restore.contains(&format!("/fabrico11y/{tool_bundle}")));
        assert!(!save.contains(&format!("/fabrico11y/{tool_bundle}")));
        assert!(!dev.contains(&format!("/fabrico11y/{tool_bundle}")));
    }
}

#[test]
fn development_jobs_partition_checks_and_preflight_their_image_pins() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/development-checks.yml"))
        .expect("read development-checks.yml");
    let (native, verify) = workflow
        .split_once("\n  native-acceptance:\n")
        .and_then(|(_, jobs)| jobs.split_once("\n  verify:\n"))
        .expect("both required jobs are present");

    assert!(!workflow.contains("xtask check --tier"));
    assert_eq!(workflow.matches("xtask check --group workspace").count(), 1);
    assert_eq!(workflow.matches("xtask check --group native").count(), 1);
    assert_eq!(workflow.matches("xtask check --group workflow").count(), 1);

    assert!(native.contains("xtask check --group native"));
    assert!(native.contains("jj git init --colocate"));
    assert!(native.contains("check-engine.sh"));
    assert!(native.contains("xtask fuzz smoke"));
    assert!(native.contains("xtask fuzz scheduled"));

    assert!(verify.contains("xtask check --group workspace"));
    assert!(verify.contains("seed=/opt/fabrico11y/compiler-seed"));
    assert!(verify.contains("exec \"$seed/release-build\""));
    assert!(verify.contains("exec cargo build --locked --release --workspace --bins --examples"));
    assert!(verify.contains("xtask collection smoke"));
    assert!(verify.contains(
        "if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'\n        run: cargo xtask check --group workflow"
    ));
    assert!(!verify.contains("jj git init --colocate"));
    assert!(!workflow.contains("run: sh .github/scripts/install-"));
    assert!(workflow.contains("RUSTUP_AUTO_INSTALL: '0'"));
    assert!(native.contains("CI_IMAGE_FLAVOR: native"));
    assert!(!native.contains("CI_IMAGE_SEED_REVISION:"));
    assert!(!workflow.contains("release-build --preflight"));
    assert!(verify.contains("CI_IMAGE_FLAVOR: common"));
    assert!(native.contains("run: &ci-image-preflight |"));
    assert!(verify.contains("run: *ci-image-preflight"));
    for (job, image) in [
        (native, "native-ci-image"),
        (verify, "common-seeded-ci-image"),
    ] {
        let digest = job
            .lines()
            .find_map(|line| {
                line.trim().strip_prefix(&format!(
                    "image: &{image} ghcr.io/kmosoti/fabrico11y-ci@sha256:"
                ))
            })
            .expect("pin the same image used by the job's cache");
        assert!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert_eq!(job.matches(&format!("image: *{image}")).count(), 2);
    }
    for source_pin in [
        "rust-toolchain.toml",
        ".github/scripts/install-toolchain.sh",
        ".github/scripts/install-schema-tools.sh",
        ".github/scripts/check-ci-image-tools.sh",
        "verification/aeneas.opam.lock",
        ".github/scripts/install-verification-tools.sh",
        ".github/scripts/install-engine-tools.sh",
    ] {
        assert!(
            native.contains(source_pin),
            "preflight missing {source_pin}"
        );
    }
    assert!(native.contains("sh \"$bootstrap/.github/scripts/check-ci-image-tools.sh\""));
    assert!(native.contains("safe.directory \"$GITHUB_WORKSPACE\""));
    let publisher_revision = verify
        .lines()
        .find_map(|line| line.trim().strip_prefix("CI_IMAGE_SEED_REVISION: "))
        .expect("seeded check pins the publisher revision independently of the checkout");
    assert!(
        publisher_revision.len() == 40
            && publisher_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert!(native.contains(
        "ci-image-paired-seed.sh revision \"$seed/provenance/revision\" \"$CI_IMAGE_SEED_REVISION\""
    ));
    assert!(native.contains("sha256sum --check --strict --status provenance/tools.sha256"));
}

#[cfg(unix)]
#[test]
fn seeded_adoption_builds_without_the_seed_after_policy_changes() {
    use std::os::unix::fs::PermissionsExt;

    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/development-checks.yml")).unwrap();
    let release = workflow
        .split_once("      - name: Build release collection executables\n")
        .unwrap()
        .1
        .split_once("        run: |\n")
        .unwrap()
        .1
        .split_once("      - name:")
        .unwrap()
        .0;
    let release = release.replace(
        "seed=/opt/fabrico11y/compiler-seed",
        "seed=\"$TEST_SEED_ROOT\"",
    );

    let scratch = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = scratch.path();
    let bin = root.join("bin");
    let seed = root.join("seed");
    let checkout = root.join(".github/ci-images");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&seed).unwrap();
    fs::create_dir_all(&checkout).unwrap();

    let launcher = b"#!/bin/sh\nprintf 'seed launcher was executed\\n'\n";
    let wrapper = b"#!/bin/sh\nexit 1\n";
    fs::write(seed.join("release-build"), launcher).unwrap();
    fs::set_permissions(
        seed.join("release-build"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(seed.join("registry-rustc"), wrapper).unwrap();
    fs::write(checkout.join("compiler-seed-release.sh"), launcher).unwrap();
    fs::write(checkout.join("compiler-seed-registry-rustc.sh"), wrapper).unwrap();
    fs::write(
        bin.join("cargo"),
        b"#!/bin/sh\nprintf 'plain cargo: %s\\n' \"$*\"\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();

    let mut policy_pins = Vec::new();
    for (name, file) in [
        ("CI_IMAGE_SEED_BUILDER_SHA256", "compiler-seed-build.sh"),
        ("CI_IMAGE_SEED_PACKAGES_SHA256", "compiler-seed-packages.py"),
    ] {
        let prefix = format!("{name}: ");
        let digest = workflow
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
            .expect("publisher pins the exact policy file");
        policy_pins.push((name, digest));
        fs::copy(
            repo_root().join(".github/ci-images").join(file),
            checkout.join(file),
        )
        .unwrap();
    }
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let probe = || {
        let result = std::process::Command::new("sh")
            .args(["-c", &release])
            .current_dir(root)
            .env("PATH", &path)
            .env("TEST_SEED_ROOT", &seed)
            .envs(policy_pins.iter().copied())
            .output()
            .unwrap();
        assert!(result.status.success(), "{:?}", result);
        result
    };

    assert_eq!(probe().stdout, b"seed launcher was executed\n");
    for file in [
        "compiler-seed-build.sh",
        "compiler-seed-packages.py",
        "compiler-seed-release.sh",
        "compiler-seed-registry-rustc.sh",
    ] {
        let source = checkout.join(file);
        let original = fs::read(&source).unwrap();
        let mut changed = original.clone();
        changed.extend_from_slice(b"\n# Changed checked-out seed policy\n");
        fs::write(&source, changed).unwrap();
        let result = probe();
        assert_eq!(
            result.stdout, b"plain cargo: build --locked --release --workspace --bins --examples\n",
            "{file}"
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains(file));
        fs::write(&source, original).unwrap();
    }
}

#[test]
fn image_qualification_runs_the_develop_push_workload() {
    let workflows = repo_root().join(".github/workflows");
    let development = fs::read_to_string(workflows.join("development-checks.yml")).unwrap();
    let paired_lane =
        fs::read_to_string(repo_root().join(".github/scripts/ci-image-paired-lane.sh")).unwrap();
    let paired_store =
        fs::read_to_string(repo_root().join(".github/scripts/ci-image-paired-store.sh")).unwrap();
    let seeded_launcher =
        fs::read_to_string(repo_root().join(".github/ci-images/compiler-seed-release.sh")).unwrap();
    let paired_cache =
        fs::read_to_string(repo_root().join(".github/actions/ci-image-paired-lane/action.yml"))
            .unwrap();

    for command in [
        "jj git init --colocate",
        "cargo xtask check --group native",
        "sh .github/scripts/check-engine.sh",
        "cargo xtask fuzz smoke /var/tmp/fabric-fuzz-smoke",
        "cargo xtask check --group workspace",
        "cargo build --locked --release --workspace --bins --examples",
        "target/release/xtask collection smoke",
    ] {
        assert!(
            paired_lane
                .lines()
                .any(|line| line.trim_end().ends_with(command)),
            "paired trial missing shared workload command: {command}"
        );
        if command == "cargo build --locked --release --workspace --bins --examples" {
            assert!(development.contains("exec \"$seed/release-build\""));
            assert!(seeded_launcher.lines().any(|line| line.trim() == command));
            continue;
        }
        assert!(
            development
                .lines()
                .filter_map(|line| line.trim().strip_prefix("run: "))
                .any(|line| line == command),
            "development checks missing shared workload command: {command}"
        );
    }
    for setting in [
        "runs-on: ubuntu-24.04",
        "CARGO_BUILD_JOBS: '4'",
        "CARGO_INCREMENTAL: '0'",
        "CARGO_PROFILE_DEV_DEBUG: '0'",
        "CARGO_PROFILE_TEST_DEBUG: '0'",
        "LEAN_NUM_THREADS: '4'",
        "TMPDIR: /var/tmp",
        "FABRIC_FUZZ_TARGET_DIR: /github/home/.cache/fabrico11y-fuzz-target",
    ] {
        assert!(
            development.contains(setting),
            "development checks missing {setting}"
        );
    }
    for setting in [
        "CARGO_BUILD_JOBS=4",
        "CARGO_INCREMENTAL=0",
        "CARGO_PROFILE_DEV_DEBUG=0",
        "CARGO_PROFILE_TEST_DEBUG=0",
        "LEAN_NUM_THREADS=4",
        "TMPDIR=/var/tmp",
        "FABRIC_FUZZ_TARGET_DIR=/github/home/.cache/fabrico11y-fuzz-target",
    ] {
        assert!(
            paired_store.contains(setting),
            "paired trial missing {setting}"
        );
    }
    assert!(paired_lane.contains("FABRIC_FUZZ_SMOKE_SECONDS=60"));
    assert_eq!(paired_cache.matches("fail-on-cache-miss: true").count(), 2);
    assert!(!paired_cache.contains("restore-keys:"));
}

#[test]
fn compiler_seed_publication_and_qualification_keep_the_trusted_boundaries() {
    let workflows = repo_root().join(".github/workflows");
    let publisher = fs::read_to_string(workflows.join("ci-images.yml")).unwrap();
    let qualification =
        fs::read_to_string(workflows.join("ci-image-paired-qualification.yml")).unwrap();
    let lane =
        fs::read_to_string(repo_root().join(".github/scripts/ci-image-paired-lane.sh")).unwrap();

    assert!(publisher.contains("options: [standard, common-seeded, all]"));
    assert!(publisher.contains("default: standard"));
    assert!(publisher.contains("github.event_name == 'push' && 'standard'"));
    assert!(!publisher.contains("      - 'Cargo.lock'"));
    assert!(publisher.contains(
        "if: github.repository == 'kmosoti/FabricO11y' && github.ref == 'refs/heads/develop'"
    ));
    for standard_step in [
        "Build common and native tool images",
        "Verify the installed tools offline",
        "Publish verified images and record their digests",
    ] {
        assert!(publisher.contains(&format!(
            "- name: {standard_step}\n        if: env.PUBLISH_VARIANT != 'common-seeded'"
        )));
    }
    let seeded = publisher
        .split_once("- name: Build the common image with a release compiler seed")
        .unwrap()
        .1;
    let (seed_steps, _) = seeded
        .split_once("- name: Publish the verified common compiler-seed image")
        .unwrap();
    assert!(seed_steps.contains("--target common-seeded"));
    assert!(seed_steps.contains("docker run --rm --network none"));
    assert!(seed_steps.contains("release-build --preflight"));
    assert!(seed_steps.contains("grep -Fxq 'compiler-seed ready'"));
    assert!(qualification.contains("permissions:\n  contents: read"));
    assert_eq!(
        qualification
            .matches("variant: ${{ needs.plan.outputs.variant }}")
            .count(),
        2
    );
    assert!(lane.contains("candidate:common-seeded"));
    assert!(lane.contains("measure release_build measure_seeded_release"));
    assert!(
        lane.contains(
            "measure workspace_checks in_container 0 cargo xtask check --group workspace"
        )
    );
    assert!(
        lane.contains(
            "measure restart_smoke in_container 120 target/release/xtask collection smoke"
        )
    );
}

#[test]
fn release_workflow_and_deb_packaging_enforce_glibc_slsa_and_pins() {
    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("read .github/workflows/release.yml");
    let (_, package_jobs) = release
        .split_once("\n  fedora-rpm:\n")
        .expect("Fedora package job section");
    let (rpm_job, package_jobs) = package_jobs
        .split_once("\n  debian-deb-and-tarball:\n")
        .expect("Debian package job section");
    let (deb_job, package_jobs) = package_jobs
        .split_once("\n  optional-web-preflight:\n")
        .expect("optional web preflight job section");
    let (web_job, package_jobs) = package_jobs
        .split_once("\n  qualify-bundle:\n")
        .expect("qualification bundle job section");
    let (bundle_job, _) = package_jobs
        .split_once("\n  verify-tag:\n")
        .expect("tag verification job section");
    let apt_pos = deb_job
        .find("Install Debian 12 toolchain prerequisites")
        .expect("debian-deb-and-tarball missing apt prerequisites step");
    let checkout_pos = deb_job
        .find("actions/checkout@")
        .expect("debian-deb-and-tarball missing actions/checkout");
    assert!(
        apt_pos < checkout_pos,
        "install Debian prerequisites before checkout"
    );
    assert!(release.contains("actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8"));
    assert!(release.contains("actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"));
    assert!(release.contains("actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333"));
    assert!(
        release
            .contains("actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8")
    );
    assert!(
        release.contains("softprops/action-gh-release@8a0cd651e5b2365bd640854a3fa5dfc70e4875f3")
    );
    assert!(release.contains("MAX_GLIBC: '2.36'"));
    assert!(release.contains("packaging/deb/build-deb.sh"));
    assert!(release.contains("packaging/deb/build-tarball.sh"));
    assert!(release.contains("packaging/rpm/fabrico11y.spec"));
    let (qualification, publication) = release.split_once("\n  release:\n").unwrap();
    assert!(
        qualification.contains(
            "github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main' && inputs.mode == 'qualify'"
        )
    );
    assert!(qualification.contains("git/ref/heads/main"));
    assert!(qualification.contains(".head_sha == $sha"));
    assert!(qualification.contains(".conclusion == \"success\""));
    let preflight = qualification
        .split_once("\n  package-source:\n")
        .unwrap()
        .1
        .split_once("\n  fedora-rpm:\n")
        .unwrap()
        .0;
    assert!(preflight.contains("if: github.event_name == 'workflow_dispatch'"));
    assert!(preflight.contains("qualify:refs/heads/main|preflight:refs/heads/*"));
    assert!(preflight.contains("test \"$live\" = \"$GITHUB_SHA\""));
    let requires_source = "    needs: [package-source]";
    for job in [rpm_job, deb_job, web_job] {
        assert!(job.lines().any(|line| line == requires_source));
    }
    assert_eq!(
        qualification
            .lines()
            .filter(|line| *line == requires_source)
            .count(),
        3
    );
    assert!(web_job.lines().any(|line| {
        line == "    if: github.event_name == 'workflow_dispatch' && inputs.mode == 'preflight' && github.ref == 'refs/heads/develop'"
    }));
    assert!(web_job.lines().all(|line| {
        !line.starts_with("    environment:") && !line.starts_with("    permissions:")
    }));
    assert!(
        bundle_job.lines().any(|line| {
            line == "    needs: [package-source, fedora-rpm, debian-deb-and-tarball]"
        })
    );
    assert!(publication.contains("git/ref/tags/$GITHUB_REF_NAME"));
    assert!(publication.contains("EXPECTED_TAG_OID"));
    assert!(qualification.contains("verify-tag:"));
    assert!(publication.contains("needs: [verify-tag]"));
    assert!(
        publication
            .contains("github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')")
    );
    assert!(publication.contains("environment: main-release"));
    assert!(!publication.contains("actions/checkout"));
    assert!(publication.contains("run-id: ${{ needs.verify-tag.outputs.run-id }}"));
    assert!(publication.contains("gh attestation verify"));
    assert!(publication.contains("sha256sum --strict --check SHA256SUMS"));
    assert!(!qualification.contains("contents: write"));
    assert!(
        release.contains("--define \"_pkg_ver ${pkg_ver}\""),
        "release workflow must parameterize RPM pkg_ver dynamically"
    );
    assert!(
        !release.contains("https://sh.rustup.rs"),
        "release workflow must not pipe unverified sh.rustup.rs"
    );
    assert!(
        release.contains("static.rust-lang.org/rustup/archive/")
            && release.contains("sha256sum -c -"),
        "release workflow must verify pinned rustup-init SHA-256 digest"
    );
    let dev_checks =
        fs::read_to_string(repo_root().join(".github/workflows/development-checks.yml"))
            .expect("read .github/workflows/development-checks.yml");
    assert!(
        dev_checks.contains("ghcr.io/kmosoti/fabrico11y-ci@sha256:"),
        "development-checks.yml must pin immutable ghcr.io/kmosoti/fabrico11y-ci@sha256: images"
    );
    assert!(
        dev_checks.contains("bootstrap=/opt/fabrico11y/bootstrap")
            && dev_checks.contains(".github/scripts/install-toolchain.sh")
            && dev_checks.contains("sha256sum < \"$1\"")
            && dev_checks.contains("sha256sum < \"$2\"")
            && dev_checks.contains("require_image_input \"$path\" \"$bootstrap/$path\""),
        "development-checks.yml must verify /opt/fabrico11y/bootstrap/.github/scripts/install-toolchain.sh via sha256sum"
    );
    let without_vartmp = release.replace("/var/tmp", "");
    assert!(
        !without_vartmp.contains("/tmp"),
        "release workflow must not reference /tmp paths"
    );
    for line in release.lines() {
        let trimmed = line.trim();
        if let Some(action) = trimmed
            .strip_prefix("uses: ")
            .or_else(|| trimmed.strip_prefix("- uses: "))
            && !action.starts_with("./")
        {
            let (_, rest) = action
                .split_once('@')
                .expect("release action must be pinned");
            let sha = rest.split_whitespace().next().unwrap_or("");
            assert!(
                sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "unpinned release action: {action}"
            );
        }
    }

    for script in [
        "packaging/deb/check-glibc.sh",
        "packaging/deb/stage-root.sh",
        "packaging/deb/build-deb.sh",
        "packaging/deb/build-tarball.sh",
        "packaging/deb/debian/postinst",
        "packaging/deb/debian/prerm",
        "packaging/deb/debian/postrm",
    ] {
        let path = repo_root().join(script);
        assert!(path.is_file(), "missing {script}");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("patchelf"), "{script} must not use patchelf");
        let output = std::process::Command::new("sh")
            .args(["-n", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "sh -n failed for {script}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
