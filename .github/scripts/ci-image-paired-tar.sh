#!/usr/bin/env bash
set -euo pipefail

# The pinned Actions cache action uses GNU tar for its archive. Product volumes
# contain root-owned proof state. Keep sudo confined to the action's exact
# argument shapes and the role's cache paths on this disposable trusted runner.
if [[ $# == 1 && "$1" == --version ]]; then
  exec /usr/bin/tar --version
fi

case "${PAIR_CACHE_ROLE:-}" in baseline|candidate) ;; *) exit 1 ;; esac
role_prefix=".paired-product/$PAIR_CACHE_ROLE"
temp_prefix=$(realpath -- "$RUNNER_TEMP")
workspace=$(realpath -- "$GITHUB_WORKSPACE")
uuid='[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
validate_dir() {
  [[ "${1%/*}" == "$temp_prefix" && "${1##*/}" =~ ^$uuid$ && ! -L "$1" ]]
}

valid_archive() {
  [[ "$1" == cache.tzst || "$1" == cache.tgz ]]
}

case "$1" in
  --posix)
    validate_dir "$(pwd -P)"
    [[ $# -ge 10 && "$2" == -cf && "$4" == --exclude && "$5" == "$3" &&
      "$6" == -P && "$7" == -C && "$8" == "$workspace" && "$9" == --files-from &&
      "${10}" == manifest.txt ]]
    valid_archive "$3"
    archive=$3
    shift 10
    case "$*" in
      '-z'|'--use-compress-program zstdmt'|'--use-compress-program zstdmt --long=30') ;;
      *) exit 1 ;;
    esac
    count=0
    while IFS= read -r path || [[ -n "$path" ]]; do
      case "${path#"$role_prefix"/}" in
        workspace/target|home/.cargo/registry|home/.cargo/git/db|home/.cache/fabrico11y-fuzz-target|\
        home/.local/share/fabrico11y/schema-tools|home/.local/share/fabrico11y/engine-tools|\
        home/.local/share/fabrico11y/proof-tools)
          [[ "$path" == "$role_prefix/"* ]]
          ((count += 1))
          ;;
        *) printf 'Unexpected cache root %s\n' "$path" >&2; exit 1 ;;
      esac
    done < manifest.txt
    ((count > 0))
    printf 'Paired cache archive for %s contains %s permitted roots\n' "$PAIR_CACHE_ROLE" "$count"
    exec sudo -n /usr/bin/tar --posix -cf "$archive" --exclude "$archive" \
      -C "$workspace" --files-from manifest.txt "$@"
    ;;
  -xf|-tf)
    operation=$1
    archive=$2
    validate_dir "${archive%/*}"
    [[ ! -L "$archive" && "$3" == -P ]]
    valid_archive "${archive##*/}"
    shift 3
    if [[ "$operation" == -xf ]]; then
      [[ "$1" == -C && "$2" == "$workspace" ]]
      shift 2
    fi
    case "$*" in
      '-z'|'--use-compress-program unzstd'|'--use-compress-program unzstd --long=30') ;;
      *) exit 1 ;;
    esac
    if [[ "$operation" == -tf ]]; then
      exec /usr/bin/tar -tf "$archive" "$@"
    fi
    printf 'Paired cache restore is isolated to %s\n' "$role_prefix"
    # Omit -P. GNU tar then rejects traversal names. The prefix filter excludes
    # entries outside the same role even if an invalid seed reaches this reader.
    exec sudo -n /usr/bin/tar -xf "$archive" -C "$workspace" "$@" \
      --wildcards --wildcards-match-slash --anchored -- "$role_prefix/*"
    ;;
  *) printf 'Unexpected cache tar operation %s\n' "$1" >&2; exit 1 ;;
esac
