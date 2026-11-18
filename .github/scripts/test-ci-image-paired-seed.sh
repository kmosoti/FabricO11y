#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
script="$here/ci-image-paired-seed.sh"
fixture=$(mktemp -d "${TMPDIR:-/var/tmp}/ci-image-paired-seed.XXXXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
valid=0000000000000000000000000000000000000001
wrong=0000000000000000000000000000000000000002

reject() {
  if bash "$script" "$@" > "$fixture/rejected.log" 2>&1; then
    printf 'Unexpected seed evidence success for %s\n' "$*" >&2
    return 1
  fi
}

printf '%s\n' "$valid" > "$fixture/revision"
bash "$script" revision "$fixture/revision" "$valid"
reject revision "$fixture/revision" "$wrong"
printf '\n' >> "$fixture/revision"
reject revision "$fixture/revision" "$valid"

cat > "$fixture/healthy.json" <<'JSON'
{
  "version": "0.19.0",
  "effective_cache_mode": "ReadOnly",
  "stats": {
    "cache_writes": 0,
    "cache_read_errors": 0,
    "cache_write_errors": 0,
    "cache_errors": {"counts": {}},
    "cache_hits": {"counts": {"Rust": 285}}
  }
}
JSON
test "$(bash "$script" hits "$fixture/healthy.json" false)" = 285
for counter in cache_read_errors cache_write_errors; do
  jq --arg counter "$counter" '.stats[$counter] = 1' "$fixture/healthy.json" > "$fixture/$counter.json"
  reject hits "$fixture/$counter.json" false
  reject hits "$fixture/$counter.json" true
done
jq '.stats.cache_read_errors = "0"' "$fixture/healthy.json" > "$fixture/not-a-counter.json"
reject hits "$fixture/not-a-counter.json" false
jq '.stats.cache_hits.counts = {}' "$fixture/healthy.json" > "$fixture/no-hits.json"
test "$(bash "$script" hits "$fixture/no-hits.json" true)" = 0
reject hits "$fixture/no-hits.json" false
