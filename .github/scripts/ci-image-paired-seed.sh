#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=.github/scripts/ci-image-paired-settings.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/ci-image-paired-settings.sh"

plan=$1
run_id=$2
attempt=$3
revision=$4
profile=$5
candidate=$6

fail() {
  printf 'Warm diagnostic seed %s attempt %s: %s\n' "$run_id" "$attempt" "$*" >&2
  exit 1
}

[[ "$run_id" =~ ^[1-9][0-9]*$ && "$attempt" =~ ^[1-9][0-9]*$ ]] || fail 'invalid run or attempt'
[[ "$GITHUB_REF" == refs/heads/ci/tool-image-paired-qualification/?* ]] || fail 'outside the hosted trial branches'
repo="repos/$GITHUB_REPOSITORY"
branch=${GITHUB_REF#refs/heads/}
run="$repo/actions/runs/$run_id"
workflow=.github/workflows/ci-image-paired-qualification.yml
head=$(gh api "$run/attempts/$attempt" | jq -er \
  --arg run "$run_id" --arg attempt "$attempt" --arg branch "$branch" --arg workflow "$workflow" \
  --arg repo "$GITHUB_REPOSITORY" '
  select(.path == $workflow and .head_branch == $branch and .event == "push" and .status == "completed" and
    (.id | tostring) == $run and (.run_attempt | tostring) == $attempt and
    .repository.full_name == $repo and .head_repository.full_name == $repo) |
  .head_sha | select(test("^[0-9a-f]{40}$"))
') || fail 'expected a completed paired workflow attempt on this repository and branch'

gh api -H 'Accept: application/vnd.github.raw+json' "$repo/contents/$plan?ref=$head" |
  jq -e --arg revision "$revision" --arg profile "$profile" --arg candidate "$candidate" '
    .mode == "qualification" and .revision == $revision and .profile == $profile and .candidate == $candidate
  ' >/dev/null || fail 'original qualification plan differs in source, profile or image'

gh api --paginate "$run/attempts/$attempt/jobs?per_page=100" | jq -se --arg profile "$profile" '
  [.[].jobs[] | select(.name == "cold (1) / pair-cold-1" and .conclusion == "success")] |
  length == 1 and (.[0].steps | map(select(.conclusion == "success") | .name) |
    (["Measure the first lane", "Measure the second lane", "Save baseline application cache",
      "Save candidate application cache", "Record final evidence and clean up owned daemons",
      "Run actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"] - . | length == 0) and
    ($profile != "native-acceptance" or index("Save baseline proof cache") != null))
' >/dev/null || fail 'cold pair 1 must have completed both workloads, cache saves and artifact upload'

work=$(mktemp -d "$RUNNER_TEMP/paired-seed.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
name="ci-image-pair-$attempt-cold-1"
metadata=$(gh api -X GET "$run/artifacts" -f name="$name" | jq -er \
  --arg run "$run_id" --arg branch "$branch" --arg head "$head" --arg name "$name" '
  [.artifacts[] | select(.name == $name and .expired == false and
    .workflow_run.head_branch == $branch and .workflow_run.head_sha == $head and
    (.workflow_run.id | tostring) == $run)] | select(length == 1) | .[0] |
  select((.id | tostring | test("^[0-9]+$")) and (.digest | test("^sha256:[0-9a-f]{64}$"))) |
  [.id, (.digest | sub("^sha256:"; ""))] | @tsv
') || fail 'missing unique, unexpired cold-pair artifact from the original attempt'
read -r artifact_id artifact_digest <<< "$metadata"
gh api "$repo/actions/artifacts/$artifact_id/zip" > "$work/evidence.zip"
printf '%s  %s\n' "$artifact_digest" "$work/evidence.zip" | sha256sum --check --status || fail 'cold artifact SHA-256 differs'

unzip -p "$work/evidence.zip" pair.json | jq -e --arg run "$run_id" --arg attempt "$attempt" \
  --arg revision "$revision" --arg profile "$profile" --arg candidate "$candidate" --arg baseline "$paired_baseline" '
  .run_id == $run and .attempt == $attempt and .profile == $profile and .revision == $revision and
  .candidate == $candidate and .baseline == $baseline and .number == 1 and
  .warm == false and .smoke == false and .first == "baseline" and .second == "candidate"
' >/dev/null || fail 'cold pair artifact metadata differs from the original qualification plan'

baseline_source=$(unzip -p "$work/evidence.zip" baseline/source.sha256)
candidate_source=$(unzip -p "$work/evidence.zip" candidate/source.sha256)
[[ "$baseline_source" =~ ^[0-9a-f]{64}$ && "$baseline_source" == "$candidate_source" ]] ||
  fail 'cold lanes must have matching source archive digests'

caches=$(gh api --paginate -X GET "$repo/actions/caches" -f per_page=100 \
  -f ref="$GITHUB_REF" -f key="ci-image-pair-v1-$run_id-$attempt-$profile-X64" | jq -sc '[.[].actions_caches[]]')
contains_cache() {
  jq -e --arg key "$1" --arg ref "$GITHUB_REF" \
    'any(.[]; .key == $key and .ref == $ref and .size_in_bytes > 0)' <<< "$caches" >/dev/null ||
    fail "missing exact cache: $1"
}
for role in baseline candidate; do
  image=$candidate
  if [[ "$role" == baseline ]]; then
    image=$paired_baseline
  fi
  key=$(paired_cache_key "$run_id" "$attempt" "$profile" "$role" "$image" "$revision")
  unzip -p "$work/evidence.zip" "$role/cache-key" | grep -Fxq "$key" || fail "$role evidence names the wrong cache"
  unzip -p "$work/evidence.zip" "$role/timings.tsv" | awk -F '\t' '
    $1 == "workload" && $2 ~ /^[0-9]+$/ && $2 > 0 { count++ }
    END { exit (count != 1) }
  ' || fail "$role evidence has no completed workload"
  contains_cache "$key-application"
  if [[ "$role:$profile" == baseline:native-acceptance ]]; then
    contains_cache "$key-proof"
  fi
done
printf 'Accepted warm diagnostic seed %s attempt %s, cold pair 1 artifact %s\n' "$run_id" "$attempt" "$artifact_id" >&2
printf '%s\n' "$baseline_source"
