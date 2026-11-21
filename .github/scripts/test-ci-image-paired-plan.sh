#!/usr/bin/env bash
set -euo pipefail

source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
fixture=$(mktemp -d "${TMPDIR:-/var/tmp}/ci-image-paired-plan.XXXXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
cd "$fixture"
mkdir -p bin .github/ci-images .github/workflows .github/actions/{restore-ci-cache,save-ci-cache}
for path in .github/workflows/development-checks.yml \
    .github/actions/restore-ci-cache/action.yml .github/actions/save-ci-cache/action.yml; do
  printf 'trusted %s\n' "$path" > "$path"
done
export GITHUB_REPOSITORY=kmosoti/FabricO11y RUNNER_TEMP=$fixture
export GITHUB_OUTPUT=$fixture/outputs GITHUB_STEP_SUMMARY=$fixture/summary
export PATH="$fixture/bin:$PATH"
export REVISION=0000000000000000000000000000000000000001
export DIGEST=sha256:1111111111111111111111111111111111111111111111111111111111111111
export IMAGE=ghcr.io/kmosoti/fabrico11y-ci
export EXPECTED_TAG=common-seeded MANIFEST_DIGEST=$DIGEST OCI_VARIANT=common-seeded ANCESTRY=behind
cat > bin/gh <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]]
if [[ "$2" == "repos/$GITHUB_REPOSITORY/compare/main...$REVISION" ]]; then
  printf '%s\n' "$ANCESTRY"
else
  request=${4#repos/"$GITHUB_REPOSITORY"/contents/}
  [[ "$request" == *"?ref=$REVISION" ]]
  cat -- "${request%\?ref=*}"
fi
MOCK
cat > bin/docker <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1 $2 $3" == "buildx imagetools inspect" ]]
[[ -d "$DOCKER_CONFIG" && -z "$(ls -A "$DOCKER_CONFIG")" ]]
case "$5" in
  --format)
    case "$6" in
      '{{json .Manifest}}')
        [[ "$4" == "$IMAGE:$EXPECTED_TAG-$REVISION" ]]
        jq -n --arg digest "$MANIFEST_DIGEST" '{digest: $digest}'
        ;;
      '{{json .Image}}')
        [[ "$4" == "$IMAGE@$DIGEST" ]]
        jq -n --arg revision "$REVISION" --arg variant "$OCI_VARIANT" '{
          os: "linux", architecture: "amd64", config: {Labels: {
            "org.opencontainers.image.source": "https://github.com/kmosoti/FabricO11y",
            "org.opencontainers.image.revision": $revision,
            "io.fabrico11y.ci.variant": $variant
          }}
        }'
        ;;
      *) exit 1 ;;
    esac
    ;;
  --raw) printf '{"layers":[{"size":123}]}\n' ;;
  *) exit 1 ;;
esac
MOCK
chmod +x bin/{docker,gh}

plan() {
  jq -n --arg revision "$REVISION" --arg profile "$1" --arg candidate "$IMAGE@$DIGEST" --arg variant "${2:-}" '
    {revision: $revision, profile: $profile, candidate: $candidate, mode: "qualification"} +
    (if $variant == "" then {} else {variant: $variant} end)
  ' > .github/ci-images/paired-qualification.json
  : > "$GITHUB_OUTPUT"
}
reject() {
  if "$@" > "$fixture/rejected.log" 2>&1; then
    printf 'Unexpected qualification-plan success for %s\n' "$*" >&2
    return 1
  fi
  test ! -s "$GITHUB_OUTPUT"
}

plan verify common-seeded
bash "$source_dir/ci-image-paired-plan.sh"
grep -Fx 'variant=common-seeded' "$GITHUB_OUTPUT"
plan verify common-seeded
reject env OCI_VARIANT=common bash "$source_dir/ci-image-paired-plan.sh"
reject env MANIFEST_DIGEST=sha256:unexpected bash "$source_dir/ci-image-paired-plan.sh"
reject env ANCESTRY=diverged bash "$source_dir/ci-image-paired-plan.sh"
plan native-acceptance common-seeded
reject bash "$source_dir/ci-image-paired-plan.sh"
plan verify
EXPECTED_TAG=common OCI_VARIANT='' bash "$source_dir/ci-image-paired-plan.sh"
grep -Fx 'variant=common' "$GITHUB_OUTPUT"
plan native-acceptance
EXPECTED_TAG=native OCI_VARIANT='' bash "$source_dir/ci-image-paired-plan.sh"
grep -Fx 'variant=native' "$GITHUB_OUTPUT"
