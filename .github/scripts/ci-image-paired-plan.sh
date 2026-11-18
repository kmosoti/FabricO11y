#!/usr/bin/env bash
set -euo pipefail

plan=${PAIR_PLAN:-.github/ci-images/paired-qualification.json}
revision=$(jq -er '.revision | select(test("^[0-9a-f]{40}$"))' "$plan")
profile=$(jq -er '.profile | select(. == "verify" or . == "native-acceptance")' "$plan")
candidate=$(jq -er '.candidate | select(test("^ghcr[.]io/kmosoti/fabrico11y-ci@sha256:[0-9a-f]{64}$"))' "$plan")
mode=$(jq -er '.mode | select(. == "qualification" or . == "smoke" or . == "warm-diagnostic")' "$plan")
case "$plan:$mode" in
  .github/ci-images/paired-qualification.json:qualification|.github/ci-images/paired-qualification.json:smoke|\
    .github/ci-images/paired-warm-diagnostic.json:warm-diagnostic) ;;
  *) printf 'Unsupported plan and mode: %s (%s)\n' "$plan" "$mode" >&2; exit 1 ;;
esac
status=$(gh api "repos/$GITHUB_REPOSITORY/compare/develop...$revision" --jq .status)
case "$status" in identical|behind) ;; *) echo "Revision is outside develop history" >&2; exit 1 ;; esac

for path in .github/workflows/development-checks.yml     .github/actions/restore-ci-cache/action.yml .github/actions/save-ci-cache/action.yml; do
  gh api -H 'Accept: application/vnd.github.raw+json'     "repos/$GITHUB_REPOSITORY/contents/$path?ref=$revision" | cmp - "$path"
done

seed_run_id=
seed_attempt=
seed_source=
if [[ "$mode" == warm-diagnostic ]]; then
  seed_run_id=$(jq -er '.seed_run_id | strings | select(test("^[1-9][0-9]*$"))' "$plan")
  seed_attempt=$(jq -er '.seed_attempt | strings | select(test("^[1-9][0-9]*$"))' "$plan")
  seed_source=$(bash .github/scripts/ci-image-paired-seed.sh .github/ci-images/paired-qualification.json \
    "$seed_run_id" "$seed_attempt" "$revision" "$profile" "$candidate")
fi

DOCKER_CONFIG=$(mktemp -d "$RUNNER_TEMP/paired-config.XXXXXXXX")
export DOCKER_CONFIG
trap 'rm -rf -- "$DOCKER_CONFIG"' EXIT
flavor=common
if [[ "$profile" == native-acceptance ]]; then
  flavor=native
fi
published_tag="${candidate%@*}:$flavor-$revision"
published_digest=$(docker buildx imagetools inspect "$published_tag" --format '{{json .Manifest}}' |
  jq -er '.digest')
if [[ "$published_digest" != "${candidate#*@}" ]]; then
  printf '%s points to %s rather than %s\n' "$published_tag" "$published_digest" "${candidate#*@}" >&2
  exit 1
fi
config=$(docker buildx imagetools inspect "$candidate" --format '{{json .Image}}')
jq -e --arg revision "$revision" '
  .architecture == "amd64" and .os == "linux" and
  .config.Labels["org.opencontainers.image.source"] == "https://github.com/kmosoti/FabricO11y" and
  .config.Labels["org.opencontainers.image.revision"] == $revision
' <<< "$config"
compressed_bytes=$(docker buildx imagetools inspect "$candidate" --raw |
  jq -er '.layers | map(.size) | add | select(. > 0)')
printf 'revision=%s\nprofile=%s\ncandidate=%s\nmode=%s\n' "$revision" "$profile" "$candidate" "$mode" >> "$GITHUB_OUTPUT"
printf 'seed_run_id=%s\nseed_attempt=%s\nseed_source=%s\n' "$seed_run_id" "$seed_attempt" "$seed_source" >> "$GITHUB_OUTPUT"
{
  if [[ "$mode" == warm-diagnostic ]]; then
    printf '## Paired CI image warm diagnostic\n\n'
    printf 'Seed run %s attempt %s, cold pair 1.\n\n' "$seed_run_id" "$seed_attempt"
  else
    printf '## Paired CI image qualification\n\n'
  fi
  # Backticks delimit inline code in the step summary.
  # shellcheck disable=SC2016
  printf -- '- Mode: `%s`\n- Check profile: `%s`\n- Product revision: `%s`\n' "$mode" "$profile" "$revision"
  # shellcheck disable=SC2016
  printf -- '- Candidate: `%s` (%s compressed layer bytes)\n' "$candidate" "$compressed_bytes"
} >> "$GITHUB_STEP_SUMMARY"
