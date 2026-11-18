#!/bin/sh
set -eu

warm=$1
profile=$2
role=$3
application=target/.paired-cache-smoke
proof=$HOME/.local/share/fabrico11y/proof-tools/.paired-cache-smoke

fixture() {
    root=$1
    if [ "$warm" = false ]; then
        mkdir -p "$root/private"
        chmod 0700 "$root/private"
        printf '#!/bin/sh\nprintf "%%s\\n" "%s:%s"\n' "$role" "$root" > "$root/private/payload"
        chmod 0711 "$root/private/payload"
        ln "$root/private/payload" "$root/private/hardlink"
        ln -s private/payload "$root/symlink"
        return
    fi
    test "$(stat -c '%u:%g:%a' "$root/private")" = 0:0:700
    test "$(stat -c '%u:%g:%a' "$root/private/payload")" = 0:0:711
    test "$(readlink "$root/symlink")" = private/payload
    test "$(stat -c %i "$root/private/payload")" = "$(stat -c %i "$root/private/hardlink")"
    test "$("$root/symlink")" = "$role:$root"
    printf 'Restored fixture ownership, mode, bytes, symlink and hardlink for %s\n' "$root"
}

test ! -S /var/run/docker.sock
test -z "${ACTIONS_RUNTIME_TOKEN:-}${GH_TOKEN:-}${GITHUB_TOKEN:-}"
getent hosts github.com
timeout 15 bash -c 'exec 3<> /dev/tcp/github.com/443'
fixture "$application"
if [ "$profile:$role" = native-acceptance:baseline ]; then
    fixture "$proof"
fi
