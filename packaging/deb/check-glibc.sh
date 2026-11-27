#!/bin/sh
# Fail if any given ELF binary requires a glibc symbol version newer than the
# Debian 12 (bookworm) floor (GLIBC_2.36). Debian 13 (trixie) ships GLIBC_2.41.
# Usage: packaging/deb/check-glibc.sh <BINARY>...
set -eu
[ "$#" -ge 1 ] || {
  echo "usage: check-glibc.sh <BINARY>..." >&2
  exit 1
}
max="${MAX_GLIBC:-2.36}"
status=0
for bin in "$@"; do
  [ -f "$bin" ] || {
    echo "missing binary: $bin" >&2
    exit 1
  }
  need=$(
    objdump -T "$bin" \
      | sed -n 's/.*\(GLIBC_[0-9][0-9.]*\).*/\1/p' \
      | sed 's/^GLIBC_//' \
      | sort -uV \
      | tail -n 1
  )
  if [ -n "$need" ] && [ "$(printf '%s\n%s\n' "$need" "$max" | sort -V | tail -n 1)" != "$max" ]; then
    echo "$bin requires GLIBC_$need, exceeding Debian floor GLIBC_$max" >&2
    status=1
  else
    echo "$bin: GLIBC_${need:-none} <= GLIBC_$max"
  fi
done
exit "$status"
