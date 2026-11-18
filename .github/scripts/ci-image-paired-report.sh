#!/usr/bin/env bash
set -euo pipefail

root=$1
samples=3
smoke=false
if [[ "$MODE" == smoke ]]; then
  smoke=true
  samples=1
fi

if [[ "$COLD_RESULT" != success || "$WARM_RESULT" != success ]]; then
  printf 'Paired qualification incomplete (cold=%s, warm=%s).\n' "$COLD_RESULT" "$WARM_RESULT" |
    tee -a "$GITHUB_STEP_SUMMARY"
  exit 1
fi

time_ms() {
  awk -F '\t' -v phase="$2" '
    $1 == phase && $2 ~ /^[0-9]+$/ { value=$2; count++ }
    END { if (count != 1) exit 1; print value }
  ' "$1/timings.tsv"
}

assert_pull() {
  local lane=$1
  for file in images-before content-before moby-content-before moby-snapshots-before; do
    [[ -f "$lane/$file.txt" && ! -s "$lane/$file.txt" ]]
  done
  test -s "$lane/empty-store.txt"
  jq -e 'type == "array" and length > 0' "$lane/layers.json" >/dev/null
  time_ms "$lane" daemon_setup >/dev/null
  time_ms "$lane" image_pull_and_start >/dev/null
  time_ms "$lane" source >/dev/null
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
    jq -e --arg revision "$REVISION" --arg candidate "$CANDIDATE" --arg profile "$PROFILE" \
      --arg variant "$VARIANT" --arg run "$GITHUB_RUN_ID" --arg attempt "$GITHUB_RUN_ATTEMPT" --arg first "$first" --arg second "$second" \
      --argjson warm "$warm" --argjson sample "$sample" --argjson smoke "$smoke" '
      .revision == $revision and .candidate == $candidate and .profile == $profile and .variant == $variant and
      .run_id == $run and .attempt == $attempt and .first == $first and .second == $second and
      .warm == $warm and .smoke == $smoke and .number == $sample and
      (.image_os | length > 0) and (.image_version | length > 0)
    ' "$entry/pair.json" >/dev/null
    image_version=$(jq -er '.image_os + "/" + .image_version' "$entry/pair.json")
    cmp "$entry/baseline/source.sha256" "$entry/candidate/source.sha256"
    if [[ "$smoke" == false ]]; then
      test -s "$entry/baseline/environment.txt"
      cmp "$entry/baseline/environment.txt" "$entry/candidate/environment.txt"
    fi
    for role in baseline candidate; do
      lane="$entry/$role"
      assert_pull "$lane"
      image=$(jq -er --arg role "$role" '.[$role]' "$entry/pair.json")
      grep -Fqx "Digest: ${image#*@}" "$lane/pull.log"
      grep -Fqx "ci-image-pair-v1-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-$PROFILE-X64-$role-${image#*@sha256:}-$REVISION" \
        "$lane/cache-key"
      if [[ "$state" == warm ]]; then
        grep -Fxq true "$lane/application-hit"
        time_ms "$lane" application_restore >/dev/null
        if [[ "$role:$PROFILE" == baseline:native-acceptance ]]; then
          grep -Fxq true "$lane/proof-hit"
          time_ms "$lane" proof_restore >/dev/null
        fi
      fi
    done
    if [[ "$VARIANT" == common-seeded ]]; then
      bash .github/scripts/ci-image-paired-seed.sh revision \
        "$entry/candidate/compiler-seed-revision" "$REVISION"
      grep -Fxq 'compiler-seed ready' "$entry/candidate/compiler-seed-preflight.log"
      time_ms "$entry/candidate" compiler_seed_preflight >/dev/null
      if [[ "$smoke" == false ]]; then
        hits=$(bash .github/scripts/ci-image-paired-seed.sh hits \
          "$entry/candidate/compiler-seed-stats.json" "$warm")
        printf '%s pair %s candidate Rust compiler cache hits: %s\n' "$state" "$sample" "$hits"
      fi
    fi
    baseline_ms=$(time_ms "$entry/baseline" workload)
    candidate_ms=$(time_ms "$entry/candidate" workload)
    printf '| %s | %s | %s | %s | %s | %s | %+d |\n' \
      "$state" "$sample" "$first" "$image_version" "$baseline_ms" "$candidate_ms" "$((candidate_ms - baseline_ms))" \
      | tee -a "$GITHUB_STEP_SUMMARY"
    printf '%s %s %s\n' "$baseline_ms" "$candidate_ms" "$((candidate_ms - baseline_ms))" >> "$root/$state.tsv"
  done
done

if [[ "$smoke" == true ]]; then
  printf '\nPassed the isolated pull and exact cache round trip. Full workloads did not run.\n' >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi

statistics() {
  local state=$1 column=$2
  cut -d ' ' -f "$column" "$root/$state.tsv" | sort -n | tr '\n' ' '
}

read -r cold_base_min cold_base_median cold_base_max < <(statistics cold 1; echo)
read -r cold_candidate_min cold_candidate_median cold_candidate_max < <(statistics cold 2; echo)
read -r warm_base_min warm_base_median warm_base_max < <(statistics warm 1; echo)
read -r warm_candidate_min warm_candidate_median warm_candidate_max < <(statistics warm 2; echo)
read -r cold_delta_min cold_delta_median cold_delta_max < <(statistics cold 3; echo)
read -r warm_delta_min warm_delta_median warm_delta_max < <(statistics warm 3; echo)

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
