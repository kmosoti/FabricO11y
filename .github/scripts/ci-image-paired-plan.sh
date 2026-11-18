#!/usr/bin/env bash
set -euo pipefail

plan=.github/ci-images/paired-qualification.json
revision=$(jq -er '.revision | select(test("^[0-9a-f]{40}$"))' "$plan")
profile=$(jq -er '.profile | select(. == "verify" or . == "native-acceptance")' "$plan")
candidate=$(jq -er '.candidate | select(test("^ghcr[.]io/kmosoti/fabrico11y-ci@sha256:[0-9a-f]{64}$"))' "$plan")
mode=$(jq -er '.mode | select(. == "qualification" or . == "smoke")' "$plan")
variant=$(jq -er --arg profile "$profile" '
  if has("variant") then .variant
  elif $profile == "verify" then "common"
  else "native"
  end
' "$plan")
case "$profile:$variant" in
  verify:common|verify:common-seeded|native-acceptance:native) ;;
  *) printf 'Image variant %s cannot qualify for %s\n' "$variant" "$profile" >&2; exit 1 ;;
esac
status=$(gh api "repos/$GITHUB_REPOSITORY/compare/develop...$revision" --jq .status)
case "$status" in identical|behind) ;; *) echo "Revision is outside develop history" >&2; exit 1 ;; esac

for path in .github/workflows/development-checks.yml \
    .github/actions/restore-ci-cache/action.yml .github/actions/save-ci-cache/action.yml; do
  gh api -H 'Accept: application/vnd.github.raw+json' \
    "repos/$GITHUB_REPOSITORY/contents/$path?ref=$revision" | cmp - "$path"
done

DOCKER_CONFIG=$(mktemp -d "$RUNNER_TEMP/paired-config.XXXXXXXX")
export DOCKER_CONFIG
trap 'rm -rf -- "$DOCKER_CONFIG"' EXIT
published_tag="${candidate%@*}:$variant-$revision"
published_digest=$(docker buildx imagetools inspect "$published_tag" --format '{{json .Manifest}}' |
  jq -er '.digest')
if [[ "$published_digest" != "${candidate#*@}" ]]; then
  printf '%s points to %s rather than %s\n' "$published_tag" "$published_digest" "${candidate#*@}" >&2
  exit 1
fi
config=$(docker buildx imagetools inspect "$candidate" --format '{{json .Image}}')
jq -e --arg revision "$revision" --arg variant "$variant" '
  .architecture == "amd64" and .os == "linux" and
  .config.Labels["org.opencontainers.image.source"] == "https://github.com/kmosoti/FabricO11y" and
  .config.Labels["org.opencontainers.image.revision"] == $revision and
  ($variant != "common-seeded" or .config.Labels["io.fabrico11y.ci.variant"] == $variant)
' <<< "$config"
compressed_bytes=$(docker buildx imagetools inspect "$candidate" --raw |
  jq -er '.layers | map(.size) | add | select(. > 0)')
printf 'revision=%s\nprofile=%s\ncandidate=%s\nmode=%s\nvariant=%s\n' "$revision" "$profile" "$candidate" "$mode" "$variant" >> "$GITHUB_OUTPUT"
{
  printf '## Paired CI image qualification\n\n'
  # Backticks delimit inline code in the step summary.
  # shellcheck disable=SC2016
  printf -- '- Mode: `%s`\n- Check profile: `%s`\n- Candidate variant: `%s`\n- Product revision: `%s`\n' "$mode" "$profile" "$variant" "$revision"
  # shellcheck disable=SC2016
  printf -- '- Candidate: `%s` (%s compressed layer bytes)\n' "$candidate" "$compressed_bytes"
} >> "$GITHUB_STEP_SUMMARY"
