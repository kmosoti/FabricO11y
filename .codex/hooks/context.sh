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
while IFS='=' read -r key value; do
    case "$key" in
        candidate) candidate=$value ;;
        result) result=$value ;;
    esac
done < "$receipt"
current=$(jj --ignore-working-copy log --no-graph -r @ -T commit_id 2>/dev/null) || current=
if [ "$result" = success ] && [ -n "$candidate" ] && [ "$current" = "$candidate" ]; then
    printf '%s\n' '{"systemMessage":"A successful receipt names this snapshot. Unsnapshotted edits and external inputs are not checked by this hook; run candidate verify before publication."}'
else
    printf '%s\n' '{"systemMessage":"Last verification receipt is stale, unsuccessful or current identity is unavailable. Run candidate capture and verify."}'
fi
