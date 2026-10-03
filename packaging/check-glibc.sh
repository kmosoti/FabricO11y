#!/bin/sh
# Fail if any given ELF binary needs a glibc symbol version newer than the
# Debian-family floor of the product contract (ADR-0025): GLIBC_2.34, the
# glibc of Ubuntu 22.04; Debian 12 ships 2.36. Override with MAX_GLIBC=2.xx.
# Usage: packaging/check-glibc.sh <BINARY>...
set -eu
max="${MAX_GLIBC:-2.34}"
status=0
for bin in "$@"; do
  need=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
  if [ -n "$need" ] && [ "$(printf '%s\n%s\n' "$need" "$max" | sort -V | tail -1)" != "$max" ]; then
    echo "$bin needs GLIBC_$need, above the supported floor GLIBC_$max" >&2
    status=1
  else
    echo "$bin: GLIBC_${need:-none} <= GLIBC_$max"
  fi
done
exit $status
