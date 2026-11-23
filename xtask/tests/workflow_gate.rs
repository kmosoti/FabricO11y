use std::{fs, path::Path};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn single_branch_main_codeowners_and_ci_workflows_enforce_invariants() {
    let gate_path = repo_root().join(".github/workflows/main-human-gate.yml");
    assert!(
        !gate_path.exists(),
        "main-human-gate.yml must be removed under single-branch main workflow"
    );

    let codeowners = fs::read_to_string(repo_root().join(".github/CODEOWNERS"))
        .expect("read .github/CODEOWNERS");
    assert!(codeowners.lines().any(|line| line.trim() == "* @kmosoti"));

    // Legacy split promotion-source workflow must remain absent.
    let src_path = repo_root().join(".github/workflows/main-promotion-source.yml");
    assert!(
        !src_path.exists(),
        "main-promotion-source.yml must be removed"
    );

    // Development checks must target main only (avoiding duplicate main+develop triggers),
    // isolate fuzz target binaries outside the repo, and save caches only on main pushes.
    let dev_path = repo_root().join(".github/workflows/development-checks.yml");
    let dev = fs::read_to_string(&dev_path).expect("read development-checks.yml");
    assert!(dev.contains("pull_request:\n  push:\n    branches: [main]"));
    assert!(!dev.contains("branches: [main, develop]"));
    assert!(!dev.contains("branches: [develop]"));
    assert!(
        dev.contains("development-checks-${{ github.event.pull_request.number || github.ref }}")
    );
    assert!(dev.contains("cancel-in-progress: ${{ github.event_name == 'pull_request' }}"));
    assert!(dev.contains("CARGO_BUILD_JOBS: '4'"));
    assert!(dev.contains("LEAN_NUM_THREADS: '4'"));
    assert!(dev.contains("FABRIC_FUZZ_TARGET_DIR: /github/home/.cache/fabrico11y-fuzz-target"));
    assert!(dev.contains(
        "FABRIC_FUZZ_SMOKE_SECONDS: ${{ github.event_name == 'pull_request' && '15' || '60' }}"
    ));
    assert!(dev.contains("verify:"));
    assert!(dev.contains("native-acceptance:"));
    assert!(dev.contains("timeout-minutes:"));
    assert!(dev.contains("./.github/actions/restore-ci-cache"));
    assert!(dev.contains("./.github/actions/save-ci-cache"));
    assert_eq!(dev.matches("ref: ${{ github.sha }}").count(), 2);
    assert_eq!(dev.matches("persist-credentials: false").count(), 2);
    assert!(dev.contains("github.event_name == 'push' && github.ref == 'refs/heads/main'"));

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
        save.matches("github.event_name == 'push' && github.ref == 'refs/heads/main'")
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
fn image_qualification_runs_the_main_push_workload() {
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
        "if: github.repository == 'kmosoti/FabricO11y' && github.ref == 'refs/heads/main'"
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
    for job in ["fedora-rpm:", "debian-deb-and-tarball:", "release:"] {
        assert!(release.contains(job), "release workflow missing {job}");
    }
    let deb_job = release
        .split_once("\n  debian-deb-and-tarball:\n")
        .and_then(|(_, rest)| rest.split_once("\n  release:\n"))
        .map(|(deb, _)| deb)
        .expect("debian-deb-and-tarball job section");
    let apt_pos = deb_job
        .find("Install Debian 12 toolchain prerequisites")
        .expect("debian-deb-and-tarball missing apt prerequisites step");
    let checkout_pos = deb_job
        .find("actions/checkout@")
        .expect("debian-deb-and-tarball missing actions/checkout");
    let ancestry_pos = deb_job
        .find("Enforce protected-main ancestry on release tags")
        .expect("debian-deb-and-tarball missing protected-main ancestry step");
    assert!(
        apt_pos < checkout_pos && apt_pos < ancestry_pos && checkout_pos < ancestry_pos,
        "Install Debian 12 toolchain prerequisites must precede actions/checkout@ and Enforce protected-main ancestry on release tags in debian-deb-and-tarball"
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
    assert!(
        release.contains("git merge-base --is-ancestor \"${GITHUB_SHA}\" refs/remotes/origin/main"),
        "release workflow must verify protected-main ancestry on tags"
    );
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
