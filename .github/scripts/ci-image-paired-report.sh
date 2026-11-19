#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=.github/scripts/ci-image-paired-settings.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/ci-image-paired-settings.sh"

root=$1
cold_times=()
warm_times=()
samples=3
smoke=false
# These are the hosted trial job budgets in milliseconds.
case "$MODE:$PROFILE:$VARIANT" in
  qualification:verify:common|qualification:verify:common-seeded) limit_ms=6000000 ;;
  qualification:native-acceptance:native) limit_ms=21600000 ;;
  smoke:verify:common|smoke:verify:common-seeded|smoke:native-acceptance:native)
    smoke=true; samples=1; limit_ms=1800000 ;;
  *) printf 'Invalid report mode, profile or variant: %s:%s:%s\n' "$MODE" "$PROFILE" "$VARIANT" >&2; exit 1 ;;
esac

if [[ "$COLD_RESULT" != success || "$WARM_RESULT" != success ]]; then
  printf 'Paired qualification incomplete (cold=%s, warm=%s).\n' "$COLD_RESULT" "$WARM_RESULT" |
    tee -a "$GITHUB_STEP_SUMMARY"
  exit 1
fi

time_ms() {
  awk -F '\t' -v phase="$2" -v limit="$limit_ms" '
    $1 == phase {
      count++
      if (NF != 2 || $2 !~ /^(0|[1-9][0-9]*)$/ || $2 > limit || (phase == "workload" && $2 == 0)) invalid = 1
      value = $2
    }
    END { if (invalid || count != 1) exit 1; print value }
  ' "$1/timings.tsv" || {
    printf 'Invalid or missing %s timing in %s\n' "$2" "$1" >&2
    return 1
  }
}

assert_pull() {
  local lane=$1
  for file in images-before content-before moby-content-before moby-snapshots-before; do
    [[ -f "$lane/$file.txt" && ! -s "$lane/$file.txt" ]]
  done
  test -s "$lane/empty-store.txt"
  jq -e 'type == "array" and length > 0' "$lane/layers.json" >/dev/null
  time_ms "$lane" daemon_setup >/dev/null
}

assert_workload() {
  local lane=$1 role=$2 workload=$3 phase duration subtotal=0
  local -a phases=(image_pull_and_start source)
  if [[ "$warm" == true ]]; then
    phases+=(application_restore)
    if [[ "$role:$PROFILE" == baseline:native-acceptance ]]; then
      phases+=(proof_restore)
    fi
  fi
  if [[ "$role:$VARIANT" == candidate:common-seeded ]]; then
    phases+=(compiler_seed_preflight)
  fi
  if [[ "$smoke" == true ]]; then
    phases+=(smoke_fixture)
  else
    phases+=(environment)
    case "$role:$PROFILE" in
      baseline:native-acceptance) phases+=(rust_bootstrap proof_bootstrap) ;;
      baseline:verify) phases+=(rust_bootstrap schema_bootstrap) ;;
      candidate:*) phases+=(image_preflight) ;;
    esac
    if [[ "$PROFILE" == native-acceptance ]]; then
      phases+=(initialize_jj native_verification engine fuzz_smoke)
    else
      phases+=(workspace_checks release_build restart_smoke)
    fi
  fi
  # These phases are disjoint, measured within workload using one monotonic
  # clock. Daemon setup precedes the workload and is checked separately.
  for phase in "${phases[@]}"; do
    duration=$(time_ms "$lane" "$phase") || return 1
    subtotal=$((subtotal + duration))
  done
  if ((subtotal > workload)); then
    printf 'Measured phases total %s ms, exceeding %s ms workload in %s\n' "$subtotal" "$workload" "$lane" >&2
    return 1
  fi
}

{
  printf '## %s paired CI image %s (%s)\n\n' "$PROFILE" "$MODE" "$VARIANT"
  printf '| State | Pair | First role | Hosted image | Baseline (ms) | Candidate (ms) | Candidate − baseline (ms) |\n'
  printf '| --- | ---: | --- | --- | ---: | ---: | ---: |\n'
} >> "$GITHUB_STEP_SUMMARY"

for state in cold warm; do
  warm=false
  if [[ "$state" == warm ]]; then
    warm=true
  fi
  for ((sample=1; sample <= samples; sample++)); do
    entry="$root/ci-image-pair-$GITHUB_RUN_ATTEMPT-$state-$sample"
    first=baseline
    second=candidate
    if [[ "$state:$sample" == cold:2 || "$state:$sample" == warm:1 || "$state:$sample" == warm:3 ]]; then
      first=candidate
      second=baseline
    fi
    jq -se --arg baseline "$paired_baseline" --arg revision "$REVISION" --arg candidate "$CANDIDATE" --arg profile "$PROFILE" \
      --arg variant "$VARIANT" --arg run "$GITHUB_RUN_ID" --arg attempt "$GITHUB_RUN_ATTEMPT" --arg first "$first" --arg second "$second" \
      --argjson warm "$warm" --argjson sample "$sample" --argjson smoke "$smoke" '
      length == 1 and (.[0] | .baseline == $baseline and .revision == $revision and .candidate == $candidate and .profile == $profile and .variant == $variant and
      .run_id == $run and .attempt == $attempt and .first == $first and .second == $second and
      .warm == $warm and .smoke == $smoke and .number == $sample and
      (.image_os | type == "string" and length > 0) and (.image_version | type == "string" and length > 0))
    ' "$entry/pair.json" >/dev/null
    image_version=$(jq -er '.image_os + "/" + .image_version' "$entry/pair.json")
    if [[ ! "$(<"$entry/baseline/source.sha256")" =~ ^[0-9a-f]{64}$ ]] ||
      ! cmp "$entry/baseline/source.sha256" "$entry/candidate/source.sha256"; then
      printf 'Invalid or unmatched source digest in %s\n' "$entry" >&2
      exit 1
    fi
    if [[ "$smoke" == false ]]; then
      test -s "$entry/baseline/environment.txt"
      cmp "$entry/baseline/environment.txt" "$entry/candidate/environment.txt"
    fi
    for role in baseline candidate; do
      lane="$entry/$role"
      assert_pull "$lane"
      image=$(jq -er --arg role "$role" '.[$role]' "$entry/pair.json")
      grep -Fqx "Digest: ${image#*@}" "$lane/pull.log"
      expected_key="ci-image-pair-v1-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-$PROFILE-X64-$role-${image#*@sha256:}-$REVISION"
      [[ "$(<"$lane/cache-key")" == "$expected_key" ]]
      if [[ "$state" == warm ]]; then
        [[ "$(<"$lane/application-hit")" == true ]]
        if [[ "$role:$PROFILE" == baseline:native-acceptance ]]; then
          [[ "$(<"$lane/proof-hit")" == true ]]
        fi
      fi
    done
    if [[ "$VARIANT" == common-seeded ]]; then
      bash .github/scripts/ci-image-paired-seed.sh revision \
        "$entry/candidate/compiler-seed-revision" "$REVISION"
      grep -Fxq 'compiler-seed ready' "$entry/candidate/compiler-seed-preflight.log"
      if [[ "$smoke" == false ]]; then
        hits=$(bash .github/scripts/ci-image-paired-seed.sh hits \
          "$entry/candidate/compiler-seed-stats.json" "$warm")
        printf '%s pair %s candidate Rust compiler cache hits: %s\n' "$state" "$sample" "$hits"
      fi
    fi
    baseline_ms=$(time_ms "$entry/baseline" workload)
    candidate_ms=$(time_ms "$entry/candidate" workload)
    assert_workload "$entry/baseline" baseline "$baseline_ms"
    assert_workload "$entry/candidate" candidate "$candidate_ms"
    printf '| %s | %s | %s | %s | %s | %s | %+d |\n' \
      "$state" "$sample" "$first" "$image_version" "$baseline_ms" "$candidate_ms" "$((candidate_ms - baseline_ms))" \
      | tee -a "$GITHUB_STEP_SUMMARY"
    row="$baseline_ms $candidate_ms $((candidate_ms - baseline_ms))"
    if [[ "$state" == cold ]]; then
      cold_times+=("$row")
    else
      warm_times+=("$row")
    fi
  done
done

if [[ "$smoke" == true ]]; then
  printf '\nPassed the isolated pull and exact cache round trip. Full workloads did not run.\n' >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi

statistics() {
  local column=$1
  shift
  printf '%s\n' "$@" | cut -d ' ' -f "$column" | sort -n | paste -sd ' '
}

read -r cold_base_min cold_base_median cold_base_max < <(statistics 1 "${cold_times[@]}")
read -r cold_candidate_min cold_candidate_median cold_candidate_max < <(statistics 2 "${cold_times[@]}")
read -r warm_base_min warm_base_median warm_base_max < <(statistics 1 "${warm_times[@]}")
read -r warm_candidate_min warm_candidate_median warm_candidate_max < <(statistics 2 "${warm_times[@]}")
read -r cold_delta_min cold_delta_median cold_delta_max < <(statistics 3 "${cold_times[@]}")
read -r warm_delta_min warm_delta_median warm_delta_max < <(statistics 3 "${warm_times[@]}")

{
  printf '\n| State | Baseline median (range), ms | Candidate median (range), ms | Paired delta median (range), ms |\n'
  printf '| --- | ---: | ---: | ---: |\n'
  printf '| Cold | %s (%s–%s) | %s (%s–%s) | %s (%s–%s) |\n' \
    "$cold_base_median" "$cold_base_min" "$cold_base_max" \
    "$cold_candidate_median" "$cold_candidate_min" "$cold_candidate_max" \
    "$cold_delta_median" "$cold_delta_min" "$cold_delta_max"
  printf '| Warm | %s (%s–%s) | %s (%s–%s) | %s (%s–%s) |\n' \
    "$warm_base_median" "$warm_base_min" "$warm_base_max" \
    "$warm_candidate_median" "$warm_candidate_min" "$warm_candidate_max" \
    "$warm_delta_median" "$warm_delta_min" "$warm_delta_max"
} >> "$GITHUB_STEP_SUMMARY"

if ((cold_candidate_median >= cold_base_median || warm_candidate_median > warm_base_max)); then
  printf '\nPaired cohort **missed** the original cold or warm threshold.\n' | tee -a "$GITHUB_STEP_SUMMARY"
  exit 1
fi
printf '\nPaired cohort passed both timing thresholds.\n' >> "$GITHUB_STEP_SUMMARY"
