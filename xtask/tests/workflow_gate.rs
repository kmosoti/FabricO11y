use std::{fs, path::Path};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn main_human_gate_and_ci_workflows_enforce_promotion_invariants() {
    let gate_path = repo_root().join(".github/workflows/main-human-gate.yml");
    let gate = fs::read_to_string(&gate_path).expect("read main-human-gate.yml");

    // Human approval and promotion source checks run exclusively on PRs targeting main.
    assert!(gate.contains("pull_request:"));
    assert!(gate.contains("- main"));
    assert!(!gate.contains("pull_request_target:"));
    assert!(!gate.contains("push:"));
    assert!(!gate.contains("workflow_dispatch:"));
    assert!(!gate.contains("actions/checkout"));
    assert!(gate.contains("permissions:\n  contents: read"));
    assert!(gate.contains("concurrency:"));
    assert!(gate.contains("main-human-gate-${{ github.event.pull_request.number || github.ref }}"));
    assert!(gate.contains("verify-promotion-source:"));
    assert!(gate.contains("name: verify-promotion-source"));
    assert!(gate.contains("if: github.event.pull_request.base.ref == 'main'"));
    assert!(gate.contains("github.base_ref"));
    assert!(gate.contains("github.head_ref"));
    assert!(gate.contains("github.event.pull_request.head.repo.full_name"));
    assert!(gate.contains("BASE_REF\" != \"main\""));
    assert!(gate.contains("RELEASE_HEAD_REF\" != \"develop\""));
    assert!(gate.contains("HEAD_REPO\" != \"$BASE_REPO\""));
    assert!(gate.contains("main-human-approval:"));
    assert!(gate.contains("name: main-human-approval"));
    assert!(gate.contains("needs: [verify-promotion-source]"));
    assert!(gate.contains("environment: main-release"));
    assert!(gate.contains("RELEASE_HEAD_SHA"));

    // Legacy split promotion-source workflow must be removed so develop PRs/pushes stay clean.
    let src_path = repo_root().join(".github/workflows/main-promotion-source.yml");
    assert!(
        !src_path.exists(),
        "main-promotion-source.yml must be removed"
    );

    // Development checks must target develop only (avoiding duplicate main+develop triggers),
    // isolate fuzz target binaries outside the repo, and save caches only on develop pushes.
    let dev_path = repo_root().join(".github/workflows/development-checks.yml");
    let dev = fs::read_to_string(&dev_path).expect("read development-checks.yml");
    assert!(dev.contains("pull_request:\n  push:\n    branches: [develop]"));
    assert!(!dev.contains("branches: [main, develop]"));
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
    assert!(dev.contains("actions/cache/restore@v4"));
    assert!(dev.contains("actions/cache/save@v4"));
    assert!(!dev.contains("uses: actions/cache@v4"));
    assert!(dev.contains("github.event_name == 'push' && github.ref == 'refs/heads/develop'"));

    let restore_path = repo_root().join(".github/actions/restore-ci-cache/action.yml");
    let restore = fs::read_to_string(&restore_path).expect("read restore-ci-cache action");
    assert!(restore.contains("actions/cache/restore@v4"));
    assert!(restore.contains("profile:"));
    assert!(restore.contains("key:"));
    assert!(restore.contains("hit:"));

    let save_path = repo_root().join(".github/actions/save-ci-cache/action.yml");
    let save = fs::read_to_string(&save_path).expect("read save-ci-cache action");
    assert!(save.contains("actions/cache/save@v4"));
    assert!(save.contains("if: github.event_name == 'push' && github.ref == 'refs/heads/develop'"));

    for cached_path in [
        "/github/home/.cargo/registry",
        "/github/home/.cargo/git/db",
        "/github/home/.local/share/fabrico11y/schema-tools",
        "/github/home/.local/share/fabrico11y/engine-tools",
        "target",
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
}

#[test]
fn development_jobs_partition_checks_and_install_only_their_required_tools() {
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
    assert!(native.contains("install-verification-tools.sh"));
    assert!(native.contains("check-engine.sh"));
    assert!(native.contains("xtask fuzz smoke"));
    assert!(native.contains("xtask fuzz scheduled"));
    assert!(!native.contains("install-schema-tools.sh"));

    assert!(verify.contains("install-schema-tools.sh"));
    assert!(verify.contains("xtask check --group workspace"));
    assert!(verify.contains("cargo build --locked --release --workspace --bins --examples"));
    assert!(verify.contains("xtask collection smoke"));
    assert!(verify.contains(
        "if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'\n        run: /github/home/.cargo/bin/cargo xtask check --group workflow"
    ));
    assert!(!verify.contains("jj git init --colocate"));
    assert!(!verify.contains("install-verification-tools.sh"));
}

#[test]
fn image_qualification_runs_the_develop_push_workload() {
    let workflows = repo_root().join(".github/workflows");
    let development = fs::read_to_string(workflows.join("development-checks.yml")).unwrap();
    let trial = fs::read_to_string(workflows.join("ci-image-trial.yml")).unwrap();
    let paired_lane =
        fs::read_to_string(repo_root().join(".github/scripts/ci-image-paired-lane.sh")).unwrap();
    let paired_store =
        fs::read_to_string(repo_root().join(".github/scripts/ci-image-paired-store.sh")).unwrap();
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
        for workflow in [&development, &trial] {
            assert!(
                workflow
                    .lines()
                    .filter_map(|line| line.trim().strip_prefix("run: "))
                    .any(|line| line
                        .strip_prefix("/github/home/.cargo/bin/")
                        .unwrap_or(line)
                        == command),
                "missing shared workload command: {command}"
            );
        }
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
        assert!(development.contains(setting) && trial.contains(setting));
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
    assert!(trial.contains("FABRIC_FUZZ_SMOKE_SECONDS: '60'"));
    assert_eq!(trial.matches("fail-on-cache-miss: true").count(), 2);
    assert!(!trial.contains("restore-keys:"));
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
