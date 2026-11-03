#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
receipt=.local/last-verification
if [ ! -f "$receipt" ]; then
    printf '%s\n' '{"systemMessage":"No local verification receipt. Run candidate capture and verify before publication."}'
    exit 0
fi
candidate=
result=
coverage=
coverage_manifest=
while IFS='=' read -r key value; do
    case "$key" in
        candidate) candidate=$value ;;
        result) result=$value ;;
        coverage) coverage=$value ;;
        coverage_manifest) coverage_manifest=$value ;;
    esac
done < "$receipt"
current=$(jj --ignore-working-copy log --no-graph -r @ -T commit_id 2>/dev/null) || current=
manifest=$(sha256sum verification/coverage.toml 2>/dev/null) || manifest=
manifest=${manifest%% *}
if [ "$result" = success ] && [ -n "$candidate" ] && [ "$current" = "$candidate" ]; then
    if [ "$coverage" = declared-required-tested ] && [ -n "$manifest" ] && [ "$manifest" = "$coverage_manifest" ]; then
        printf '%s\n' '{"systemMessage":"A successful receipt names this snapshot and the current coverage manifest; its declared required tests passed. This is scoped evidence, not completeness proof. Current working-file freshness is unchecked: unsnapshotted edits and external inputs require candidate verification before publication."}'
    else
        printf '%s\n' '{"systemMessage":"A successful receipt names this snapshot, but declared coverage is unspecified or its manifest changed. Current working-file freshness is unchecked. Run candidate capture and verify before publication."}'
    fi
else
    printf '%s\n' '{"systemMessage":"Last verification receipt is stale, unsuccessful or current identity is unavailable. Coverage is not established for current work. Run candidate capture and verify."}'
fi
