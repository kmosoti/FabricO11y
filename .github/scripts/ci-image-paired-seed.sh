#!/usr/bin/env bash
set -euo pipefail

case "$1" in
  revision)
    [[ "$3" =~ ^[0-9a-f]{40}$ ]]
    observed=$(sha256sum < "$2")
    expected=$(printf '%s\n' "$3" | sha256sum)
    if [[ "$observed" != "$expected" ]]; then
      printf 'Compiler seed provenance does not match trusted revision %s\n' "$3" >&2
      exit 1
    fi
    ;;
  hits)
    jq -ser --argjson warm "$3" '
      select(length == 1) | .[0] | select(.version == "0.19.0" and .effective_cache_mode == "ReadOnly" and
        .stats.cache_writes == 0 and .stats.cache_read_errors == 0 and .stats.cache_write_errors == 0 and
        (.stats.cache_errors.counts | type == "object" and length == 0))
      | (.stats.cache_hits.counts.Rust // 0)
      | select(type == "number" and . == floor and . >= 0 and . <= 9007199254740991 and ($warm or . > 0))
    ' "$2" || {
      printf 'Invalid compiler seed cache statistics in %s\n' "$2" >&2
      exit 1
    }
    ;;
  *) exit 2 ;;
esac
