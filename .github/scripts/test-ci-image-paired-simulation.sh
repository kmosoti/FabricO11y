#!/usr/bin/env bash
set -euo pipefail

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$repo"
scratch_base=$(realpath -- "${1:-/data/tmp}")
case "$scratch_base/" in
  /tmp/*|/dev/shm/*) printf 'Use disk-backed scratch for paired simulation\n' >&2; exit 1 ;;
esac
scratch=$(mktemp -d "$scratch_base/paired-ci-simulation.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch" RUNNER_TEMP="$scratch"
export GITHUB_REPOSITORY=kmosoti/FabricO11y GITHUB_RUN_ID=4815 GITHUB_RUN_ATTEMPT=2
export GITHUB_OUTPUT="$scratch/outputs" GITHUB_STEP_SUMMARY="$scratch/summary"
export REVISION=1234567890123456789012345678901234567890
export CANDIDATE=ghcr.io/kmosoti/fabrico11y-ci@sha256:abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd
export VARIANT=common-seeded PROFILE=verify MODE=qualification COLD_RESULT=success WARM_RESULT=success
baseline=registry.fedoraproject.org/fedora@sha256:d68cae9185777603262e0a7094f792ffed9326b22e1a03e2d2a1731fc3f94733
export SIM_REPO="$repo"

assert_status() {
  local expected=$1 description=$2 status=0
  shift 2
  : > "$GITHUB_OUTPUT"
  : > "$GITHUB_STEP_SUMMARY"
  "$@" > "$scratch/log" 2>&1 || status=$?
  if [[ "$status" != "$expected" ]]; then
    printf 'Unexpected status %s (expected %s) for %s\n' "$status" "$expected" "$description" >&2
    cat "$scratch/log" >&2
    exit 1
  fi
  if [[ "$expected" != 0 ]] && grep -Fq 'passed both timing thresholds' "$GITHUB_STEP_SUMMARY"; then
    printf 'Rejected %s was reported as passed\n' "$description" >&2
    exit 1
  fi
  printf 'Verified %s\n' "$description"
}

# The real entry point receives deterministic responses from its two external
# commands. The shims reject unrecognized commands and cannot contact a service.
mkdir -p "$scratch/bin" "$scratch/plan/.github/ci-images" "$scratch/plan/.github/actions/"{restore-ci-cache,save-ci-cache}
mkdir -p "$scratch/plan/.github/workflows"
for path in .github/workflows/development-checks.yml \
  .github/actions/restore-ci-cache/action.yml .github/actions/save-ci-cache/action.yml; do
  cp "$path" "$scratch/plan/$path"
done
cat > "$scratch/bin/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
if [[ $# == 4 && "$1:$2:$3:$4" == "api:repos/$GITHUB_REPOSITORY/compare/develop...$REVISION:--jq:.status" ]]; then
  printf '%s\n' "${SIM_HISTORY:-behind}"
elif [[ $# == 4 && "$1:$2:$3" == 'api:-H:Accept: application/vnd.github.raw+json' ]]; then
  path=${4#repos/$GITHUB_REPOSITORY/contents/}
  [[ "${path#*\?ref=}" == "$REVISION" ]]
  case "${path%\?ref=*}" in
    .github/workflows/development-checks.yml|.github/actions/restore-ci-cache/action.yml|.github/actions/save-ci-cache/action.yml)
      if [[ -n "${SIM_PROTECTED_DRIFT:-}" ]]; then printf 'different workflow\n'; else cat -- "$SIM_REPO/${path%\?ref=*}"; fi
      ;;
    *) exit 98 ;;
  esac
else
  exit 98
fi
SHIM
cat > "$scratch/bin/docker" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1:$2:$3" == buildx:imagetools:inspect ]]
case "$4:$5:${6:-}" in
  "${CANDIDATE%@*}:$VARIANT-$REVISION:--format:{{json .Manifest}}")
    jq -nc --arg digest "${SIM_IMAGE_DIGEST:-${CANDIDATE#*@}}" '{digest:$digest}'
    ;;
  "$CANDIDATE:--format:{{json .Image}}")
    jq -nc --arg revision "${SIM_IMAGE_REVISION:-$REVISION}" --arg variant "$VARIANT" '{architecture:"amd64",os:"linux",
      config:{Labels:{"org.opencontainers.image.source":"https://github.com/kmosoti/FabricO11y",
      "org.opencontainers.image.revision":$revision,"io.fabrico11y.ci.variant":$variant}}}'
    ;;
  "$CANDIDATE:--raw:")
    printf '{"layers":[{"size":%s},{"size":7}]}\n' "${SIM_LAYER_SIZE:-340}"
    ;;
  *) exit 98 ;;
esac
SHIM
chmod +x "$scratch/bin/"{gh,docker}
plan_file="$scratch/plan/.github/ci-images/paired-qualification.json"
jq -nc --arg revision "$REVISION" --arg profile "$PROFILE" --arg candidate "$CANDIDATE" \
  --arg mode "$MODE" --arg variant "$VARIANT" '$ARGS.named' > "$plan_file"
run_plan() (
  cd "$scratch/plan"
  PATH="$scratch/bin:$PATH" bash "$repo/.github/scripts/ci-image-paired-plan.sh"
)
assert_status 0 'valid offline plan' run_plan
grep -Fxq 'variant=common-seeded' "$GITHUB_OUTPUT"
grep -Fq '347 compressed layer bytes' "$GITHUB_STEP_SUMMARY"
SIM_HISTORY=diverged assert_status 1 'source outside develop history' run_plan
SIM_PROTECTED_DRIFT=1 assert_status 1 'protected workload drift' run_plan
SIM_IMAGE_DIGEST=sha256:wrong assert_status 1 'moved image tag' run_plan
SIM_IMAGE_REVISION=wrong assert_status 1 'mismatched image source label' run_plan
SIM_LAYER_SIZE=9007199254740992 assert_status 1 'oversized image byte total' run_plan
SIM_LAYER_SIZE=-1 assert_status 1 'negative layer size despite a positive total' run_plan

case_dir="$scratch/case"
entry_path() { printf '%s/ci-image-pair-%s-%s-%s' "$1" "$GITHUB_RUN_ATTEMPT" "$2" "$3"; }

make_report() {
  local root=$1 state warm sample first second entry role image phase times workload
  local samples=3 smoke=false
  [[ "$MODE" != smoke ]] || { samples=1; smoke=true; }
  for state in cold warm; do
    warm=false
    [[ "$state" != warm ]] || warm=true
    for ((sample=1; sample<=samples; sample++)); do
      first=baseline second=candidate
      case "$state:$sample" in cold:2|warm:1|warm:3) first=candidate second=baseline ;; esac
      entry=$(entry_path "$root" "$state" "$sample")
      mkdir -p "$entry/"{baseline,candidate}
      jq -nc --arg revision "$REVISION" --arg candidate "$CANDIDATE" --arg variant "$VARIANT" --arg profile "$PROFILE" \
        --arg baseline "$baseline" --arg run_id "$GITHUB_RUN_ID" --arg attempt "$GITHUB_RUN_ATTEMPT" \
        --arg first "$first" --arg second "$second" --argjson warm "$warm" --argjson smoke "$smoke" \
        --argjson number "$sample" '$ARGS.named + {image_os:"ubuntu24",image_version:"simulation"}' > "$entry/pair.json"
      for role in baseline candidate; do
        image=$baseline
        [[ "$role" != candidate ]] || image=$CANDIDATE
        phase="$entry/$role"
        case "$state:$role" in
          cold:baseline) times=(0 100 130 160) ;; cold:candidate) times=(0 90 129 180) ;;
          warm:baseline) times=(0 50 90 80) ;; warm:candidate) times=(0 85 90 120) ;;
        esac
        workload=${times[sample]}
        touch "$phase/"{images-before,content-before,moby-content-before,moby-snapshots-before}.txt
        printf 'empty\n' > "$phase/empty-store.txt"
        printf '[{"digest":"sha256:fixture"}]\n' > "$phase/layers.json"
        printf 'simulated compiler environment\n' > "$phase/environment.txt"
        printf '%064d\n' 1 > "$phase/source.sha256"
        printf 'Digest: %s\n' "${image#*@}" > "$phase/pull.log"
        printf 'ci-image-pair-v1-%s-%s-%s-X64-%s-%s-%s\n' "$GITHUB_RUN_ID" "$GITHUB_RUN_ATTEMPT" \
          "$PROFILE" "$role" "${image#*@sha256:}" "$REVISION" > "$phase/cache-key"
        printf 'daemon_setup\t0\nimage_pull_and_start\t5\nsource\t0\nworkload\t%s\n' "$workload" > "$phase/timings.tsv"
        if [[ "$state" == warm ]]; then
          printf 'true\n' > "$phase/application-hit"
          printf 'application_restore\t5\n' >> "$phase/timings.tsv"
          if [[ "$role:$PROFILE" == baseline:native-acceptance ]]; then
            printf 'true\n' > "$phase/proof-hit"
            printf 'proof_restore\t5\n' >> "$phase/timings.tsv"
          fi
        fi
        if [[ "$role:$VARIANT" == candidate:common-seeded ]]; then
          printf '%s\n' "$REVISION" > "$phase/compiler-seed-revision"
          printf 'compiler-seed ready\n' > "$phase/compiler-seed-preflight.log"
          printf 'compiler_seed_preflight\t0\n' >> "$phase/timings.tsv"
          local hits=106
          [[ "$state" != warm ]] || hits=0
          jq -nc --argjson hits "$hits" '{version:"0.19.0",effective_cache_mode:"ReadOnly",stats:{cache_writes:0,
            cache_read_errors:0,cache_write_errors:0,cache_errors:{counts:{}},cache_hits:{counts:{Rust:$hits}}}}' \
            > "$phase/compiler-seed-stats.json"
        fi
      done
    done
  done
}

run_report() { bash .github/scripts/ci-image-paired-report.sh "$case_dir"; }
reset_case() {
  rm -rf -- "$case_dir"
  cp -a -- "$scratch/valid" "$case_dir"
}
set_workload() { sed -i "s/^workload\s.*/workload\t$2/" "$1/timings.tsv"; }
change_stats() { jq "$2" "$1" > "$scratch/stats" && mv -- "$scratch/stats" "$1"; }

make_report "$scratch/valid"
cold_one=$(entry_path "$case_dir" cold 1)
cold_two=$(entry_path "$case_dir" cold 2)
warm_one=$(entry_path "$case_dir" warm 1)
warm_two=$(entry_path "$case_dir" warm 2)
reset_case
assert_status 0 'matched verify cohort with warm median equal to baseline maximum' run_report
grep -Fq '| Cold | 130 (100–160) | 129 (90–180) | -1 (-10–20) |' "$GITHUB_STEP_SUMMARY"
grep -Fq '| Warm | 80 (50–90) | 90 (85–120) | 35 (0–40) |' "$GITHUB_STEP_SUMMARY"
assert_status 0 'same evidence can be reported again without stale aggregate rows' run_report

set_workload "$cold_two/candidate" 130
assert_status 1 'a tie in the cold median fails' run_report
grep -Fq 'missed' "$GITHUB_STEP_SUMMARY"
printf '800 1 -799\n900 2 -898\n1000 3 -997\n' > "$case_dir/cold.tsv"
assert_status 1 'stale median rows cannot cause a false pass' run_report
grep -Fq 'missed' "$GITHUB_STEP_SUMMARY"
reset_case
set_workload "$warm_two/candidate" 91
assert_status 1 'warm median above baseline maximum fails even if cold passes' run_report
grep -Fq 'missed' "$GITHUB_STEP_SUMMARY"

for invalid in 6000001 18446744073709551616 08 0; do
  reset_case
  set_workload "$cold_one/candidate" "$invalid"
  assert_status 1 "invalid workload duration $invalid" run_report
  grep -Fq 'Invalid or missing workload timing' "$scratch/log"
done
reset_case
printf 'workload\tbogus\n' >> "$cold_one/candidate/timings.tsv"
assert_status 1 'duplicate malformed workload line' run_report
reset_case
: > "$cold_one/baseline/source.sha256"
: > "$cold_one/candidate/source.sha256"
assert_status 1 'matching empty source digests' run_report
reset_case
printf '%064d\n' 2 > "$cold_one/candidate/source.sha256"
assert_status 1 'unequal source digests' run_report
reset_case
sed -i 's#registry.fedoraproject.org/fedora#registry.example/fedora#' "$cold_one/pair.json"
assert_status 1 'substituted baseline image' run_report
reset_case
printf 'false\ntrue\n' > "$warm_one/candidate/application-hit"
assert_status 1 'ambiguous application cache miss' run_report
reset_case
sed -i 's/pair-v1-4815-2/pair-v1-4815-1/' "$warm_one/baseline/cache-key"
assert_status 1 'wrong cache seed attempt' run_report
reset_case
printf '%040d\n' 1 > "$cold_one/candidate/compiler-seed-revision"
assert_status 1 'mismatched compiler seed provenance' run_report
for invalid in '.stats.cache_read_errors = 1' '.stats.cache_hits.counts.Rust = 0' \
  '.stats.cache_hits.counts.Rust = 0.5' '.stats.cache_hits.counts.Rust = 1e100'; do
  reset_case
  change_stats "$cold_one/candidate/compiler-seed-stats.json" "$invalid"
  assert_status 1 "invalid seeded cold evidence ($invalid)" run_report
done
reset_case
WARM_RESULT=failure assert_status 1 'incomplete warm cohort' run_report

PROFILE=native-acceptance VARIANT=native make_report "$scratch/native"
rm -rf -- "$case_dir"
cp -a -- "$scratch/native" "$case_dir"
PROFILE=native-acceptance VARIANT=native assert_status 0 'valid native exact proof and application restore' run_report
rm -- "$warm_one/baseline/proof-hit"
PROFILE=native-acceptance VARIANT=native assert_status 1 'baseline native proof cache miss' run_report
MODE=smoke PROFILE=native-acceptance VARIANT=native make_report "$scratch/smoke"
rm -rf -- "$case_dir"
cp -a -- "$scratch/smoke" "$case_dir"
MODE=smoke PROFILE=native-acceptance VARIANT=native assert_status 0 'one-pair smoke report without timing eligibility' run_report
grep -Fq 'Full workloads did not run' "$GITHUB_STEP_SUMMARY"
! grep -Fq 'passed both timing thresholds' "$GITHUB_STEP_SUMMARY"
printf 'Paired simulation passed. All jobs, cache events and timings were fixtures.\n'
