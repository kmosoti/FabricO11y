#!/usr/bin/env bash
set -euo pipefail

run_root="$RUNNER_TEMP/fabric-ci-pair"
workspace=/__w/FabricO11y/FabricO11y
product_root="$GITHUB_WORKSPACE/.paired-product"
# shellcheck source=.github/scripts/ci-image-paired-settings.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/ci-image-paired-settings.sh"
cache_run_id=${PAIR_SEED_RUN_ID:-$GITHUB_RUN_ID}
cache_attempt=${PAIR_SEED_ATTEMPT:-$GITHUB_RUN_ATTEMPT}
timeout_seconds=2700
if [[ "$PAIR_PROFILE" == native-acceptance ]]; then
  timeout_seconds=18000
fi

# shellcheck source=.github/scripts/ci-image-paired-store.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/ci-image-paired-store.sh"

select_role() {
  case "$1" in
    baseline) image=$paired_baseline ;;
    candidate) image=$PAIR_CANDIDATE ;;
    *) echo "Invalid lane role: $1" >&2; exit 1 ;;
  esac
  [[ "$image" =~ ^(registry[.]fedoraproject[.]org/fedora|ghcr[.]io/kmosoti/fabrico11y-ci)@sha256:[0-9a-f]{64}$ ]]
  evidence="$run_root/evidence/$1"
  daemon_root="$run_root/$1"
  host_workspace="$product_root/$1/workspace"
  host_home="$product_root/$1/home"
  host_temp="$product_root/$1/state"
  cache_role=".paired-product/$1"
  proof_path="$cache_role/home/.local/share/fabrico11y/proof-tools"
}

clock_ms() {
  # /proc/uptime is monotonic across the job's separate steps and processes.
  awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}

begin() {
  clock_ms > "$evidence/$1.start"
}

finish() {
  local elapsed
  elapsed=$(($(clock_ms) - $(<"$evidence/$1.start")))
  printf '%s\t%s\n' "$1" "$elapsed" >> "$evidence/timings.tsv"
  rm -- "$evidence/$1.start"
  printf '%s %s: %s ms\n' "$PAIR_ROLE" "$1" "$elapsed"
}

measure() {
  local phase=$1
  shift
  begin "$phase"
  "$@"
  finish "$phase"
}

prepare() {
  [[ "$PAIR_ARCH" == X64 ]]
  [[ "$PAIR_PROFILE" == verify || "$PAIR_PROFILE" == native-acceptance ]]
  [[ "$PAIR_REVISION" =~ ^[0-9a-f]{40}$ ]]
  [[ "$PAIR_SAMPLE" =~ ^[123]$ ]]
  [[ "$PAIR_WARM" == true || "$PAIR_WARM" == false ]]
  [[ "$PAIR_SMOKE" == true || "$PAIR_SMOKE" == false ]]
  if [[ -n "${PAIR_SEED_RUN_ID:-}${PAIR_SEED_ATTEMPT:-}${PAIR_SEED_SOURCE:-}" ]]; then
    [[ "$PAIR_WARM:$PAIR_SMOKE:$PAIR_SAMPLE" == true:false:1 ]]
    [[ "$PAIR_SEED_RUN_ID" =~ ^[1-9][0-9]*$ && "$PAIR_SEED_ATTEMPT" =~ ^[1-9][0-9]*$ &&
      "$PAIR_SEED_SOURCE" =~ ^[0-9a-f]{64}$ ]]
  fi
  [[ ! -e "$run_root" && ! -e "$product_root" ]]

  mkdir -p "$run_root/evidence" "$run_root/anonymous" "$run_root/archive-bin"
  mkdir -p "$product_root"/{baseline,candidate}/{workspace,home,state}
  install -m 0755 .github/scripts/ci-image-paired-tar.sh "$run_root/archive-bin/tar"
  printf 'archive_path=%s:%s\n' "$run_root/archive-bin" "$PATH" >> "$GITHUB_OUTPUT"

  local first=baseline second=candidate
  if [[ "$PAIR_WARM" == true && "$PAIR_SAMPLE" != 2 ]] ||
     [[ "$PAIR_WARM" == false && "$PAIR_SAMPLE" == 2 ]]; then
    first=candidate
    second=baseline
  fi
  printf 'first=%s\nsecond=%s\n' "$first" "$second" >> "$GITHUB_OUTPUT"
  jq -n --arg revision "$PAIR_REVISION" --arg profile "$PAIR_PROFILE" --arg candidate "$PAIR_CANDIDATE" \
    --arg baseline "$paired_baseline" --arg first "$first" --arg second "$second" \
    --arg image_os "${ImageOS:?Hosted runner did not expose ImageOS}" \
    --arg image_version "${ImageVersion:?Hosted runner did not expose ImageVersion}" \
    --argjson number "$PAIR_SAMPLE" --argjson warm "$PAIR_WARM" --argjson smoke "$PAIR_SMOKE" \
    --arg run_id "$GITHUB_RUN_ID" --arg attempt "$GITHUB_RUN_ATTEMPT" \
    --arg cache_run_id "$cache_run_id" --arg cache_attempt "$cache_attempt" \
    '$ARGS.named' > "$run_root/evidence/pair.json"
  {
    uname -a
    lscpu
    cat /proc/sys/kernel/random/boot_id
    awk '/^MemTotal:/ {print}' /proc/meminfo
    tar --version
    zstd --version
  } > "$run_root/evidence/host.txt"
  docker info --format '{{.Driver}} {{json .DriverStatus}}' > "$run_root/evidence/docker-storage.txt"
  docker version --format '{{json .Server}}' > "$run_root/evidence/docker-version.json"
  local driver backend=classic
  driver=$(docker info --format '{{.Driver}}')
  if grep -Fq 'io.containerd.snapshotter.v1' "$run_root/evidence/docker-storage.txt"; then
    backend=snapshotter
  fi
  [[ "$driver" == overlay2 || "$driver" == overlayfs ]]
  printf '%s %s\n' "$driver" "$backend" > "$run_root/storage"
  sudo -n systemctl stop docker.socket docker.service
}

start() {
  mkdir -p "$evidence"
  local key
  key=$(paired_cache_key "$cache_run_id" "$cache_attempt" "$PAIR_PROFILE" "$PAIR_ROLE" "$image" "$PAIR_REVISION")
  {
    printf 'key=%s\nproof_path=%s\n' "$key" "$proof_path"
    printf 'application_paths<<PATHS\n'
    printf '%s\n' "$cache_role/home/.cargo/registry" "$cache_role/home/.cargo/git/db" "$cache_role/workspace/target"
    if [[ "$PAIR_PROFILE" == native-acceptance || "$PAIR_ROLE" == baseline ]]; then
      printf '%s\n' "$cache_role/home/.cache/fabrico11y-fuzz-target"
    fi
    if [[ "$PAIR_ROLE" == baseline ]]; then
      printf '%s\n' "$cache_role/home/.local/share/fabrico11y/schema-tools" "$cache_role/home/.local/share/fabrico11y/engine-tools"
    fi
    printf 'PATHS\n'
  } >> "$GITHUB_OUTPUT"
  printf '%s\n' "$key" > "$evidence/cache-key"
  measure daemon_setup start_daemon
  begin workload
  measure image_pull_and_start pull_image
  begin source
}

materialize_source() {
  local archive="$host_temp/source.tar.gz"
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 \
    "https://api.github.com/repos/$GITHUB_REPOSITORY/tarball/$PAIR_REVISION" -o "$archive"
  gzip -d "$archive"
  [[ "$(git get-tar-commit-id < "${archive%.gz}")" == "$PAIR_REVISION" ]]
  sha256sum < "${archive%.gz}" | cut -d ' ' -f 1 > "$evidence/source.sha256"
  if [[ -n "${PAIR_SEED_SOURCE:-}" && "$(<"$evidence/source.sha256")" != "$PAIR_SEED_SOURCE" ]]; then
    printf '%s source archive digest differs from the original cold pair\n' "$PAIR_ROLE" >&2
    return 1
  fi
  tar --extract --file "${archive%.gz}" --directory "$host_workspace" --strip-components=1 --no-same-owner
  rm -- "${archive%.gz}"
  finish source
  if [[ "$PAIR_WARM" == true ]]; then
    begin application_restore
  fi
}

in_container() {
  local max_time=$1
  shift
  local elapsed remaining
  elapsed=$((($(clock_ms) - $(<"$evidence/workload.start")) / 1000))
  remaining=$((timeout_seconds - elapsed))
  ((remaining > 0)) || return 124
  if ((max_time == 0 || remaining < max_time)); then
    max_time=$remaining
  fi

  # The workload shell expands its own HOME and PATH inside the container.
  # shellcheck disable=SC2016
  timeout "$max_time" sudo -n /usr/bin/docker --config "$run_root/anonymous" --host "unix://$daemon_root/d.sock" \
    exec --interactive --env "PAIR_LANE_ROLE=$PAIR_ROLE" workload /bin/sh -ec '
      if [ "$PAIR_LANE_ROLE" = baseline ]; then
        export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
      fi
      exec "$@"
    ' shell "$@"
}

toolchain() {
  if [[ "$PAIR_WARM" == true ]]; then
    if [[ "$APPLICATION_HIT" != true ]]; then
      printf '%s did not restore its exact application cache\n' "$PAIR_ROLE" >&2
      return 1
    fi
    finish application_restore
    if ! sudo -n test -d "$host_workspace/target"; then
      printf '%s application archive lacks target/\n' "$PAIR_ROLE" >&2
      return 1
    fi
    if [[ "$PAIR_SMOKE" == false ]]; then
      local profile=release
      if [[ "$PAIR_PROFILE" == native-acceptance ]]; then
        profile=debug
        if ! sudo -n test -d "$host_home/.cache/fabrico11y-fuzz-target"; then
          printf '%s application archive lacks native fuzz artifacts\n' "$PAIR_ROLE" >&2
          return 1
        fi
      fi
      sudo -n bash .github/scripts/ci-image-paired-witness.sh cargo "$host_workspace/target/$profile"
    fi
    printf 'true\n' > "$evidence/application-hit"
  fi
  if [[ "$PAIR_ROLE" == baseline && "$PAIR_SMOKE" == false ]]; then
    measure rust_bootstrap in_container 0 sh .github/scripts/install-toolchain.sh
  fi
  if [[ "$PAIR_WARM" == true && "$PAIR_ROLE" == baseline && "$PAIR_PROFILE" == native-acceptance ]]; then
    begin proof_restore
  fi
}

verification() {
  if [[ "$PAIR_WARM" == true && "$PAIR_ROLE" == baseline && "$PAIR_PROFILE" == native-acceptance ]]; then
    if [[ "$PROOF_HIT" != true ]]; then
      printf 'baseline did not restore its exact proof cache\n' >&2
      return 1
    fi
    finish proof_restore
    if ! sudo -n test -d "$GITHUB_WORKSPACE/$proof_path"; then
      printf 'baseline proof archive lacks the proof-tool directory\n' >&2
      return 1
    fi
    if [[ "$PAIR_SMOKE" == false ]]; then
      sudo -n bash .github/scripts/ci-image-paired-witness.sh proof "$GITHUB_WORKSPACE/$proof_path" \
        "$host_workspace/verification/aeneas.opam.lock"
    fi
    printf 'true\n' > "$evidence/proof-hit"
  fi
  if [[ "$PAIR_SMOKE" == true ]]; then
    measure smoke_fixture in_container 0 sh -s "$PAIR_WARM" "$PAIR_PROFILE" "$PAIR_ROLE" \
      < .github/scripts/ci-image-paired-fixture.sh
  else
    local flavor=common
    if [[ "$PAIR_PROFILE" == native-acceptance ]]; then
      flavor=native
    fi
    if [[ "$PAIR_ROLE" == candidate ]]; then
      measure image_preflight in_container 0 sh .github/scripts/check-ci-image-tools.sh "$flavor"
    elif [[ "$PAIR_PROFILE" == native-acceptance ]]; then
      measure proof_bootstrap in_container 0 sh .github/scripts/install-verification-tools.sh
    else
      measure schema_bootstrap in_container 0 sh .github/scripts/install-schema-tools.sh
    fi
    begin environment
    in_container 0 sh -s "$PAIR_PROFILE" < .github/scripts/ci-image-paired-environment.sh |
      tee "$evidence/environment.txt"
    finish environment

    if [[ "$PAIR_PROFILE" == native-acceptance ]]; then
      # The mounted archive belongs to the host runner. Scope Git trust to it.
      # shellcheck disable=SC2016
      measure initialize_jj in_container 0 sh -ec '
        git config --global --add safe.directory "$GITHUB_WORKSPACE"
        jj git init --colocate
      '
      measure native_verification in_container 0 cargo xtask check --group native
      measure engine in_container 600 sh .github/scripts/check-engine.sh
      measure fuzz_smoke in_container 0 env FABRIC_FUZZ_SMOKE_SECONDS=60 \
        cargo xtask fuzz smoke /var/tmp/fabric-fuzz-smoke
    else
      measure workspace_checks in_container 0 cargo xtask check --group workspace
      measure release_build in_container 0 cargo build --locked --release --workspace --bins --examples
      measure restart_smoke in_container 120 target/release/xtask collection smoke
    fi
  fi
  finish workload
  df -Pk "$host_workspace" > "$evidence/disk-after.tsv"
}

case "$1" in
  prepare) prepare ;;
  finalize)
    for PAIR_ROLE in baseline candidate; do
      select_role "$PAIR_ROLE"
      cleanup_daemon
    done
    df -Pk "$GITHUB_WORKSPACE"
    ;;
  *)
    select_role "$PAIR_ROLE"
    case "$1" in
      start) start ;;
      source) materialize_source ;;
      toolchain) toolchain ;;
      verify) verification ;;
      cleanup) cleanup_daemon ;;
      *) printf 'Unknown phase: %s\n' "$1" >&2; exit 1 ;;
    esac
    ;;
esac
