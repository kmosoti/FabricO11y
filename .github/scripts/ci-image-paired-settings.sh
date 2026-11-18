#!/usr/bin/env bash

paired_baseline=registry.fedoraproject.org/fedora@sha256:d68cae9185777603262e0a7094f792ffed9326b22e1a03e2d2a1731fc3f94733

paired_cache_key() {
  local run=$1 attempt=$2 profile=$3 role=$4 image=$5 revision=$6
  printf 'ci-image-pair-v1-%s-%s-%s-X64-%s-%s-%s\n' "$run" "$attempt" "$profile" "$role" "${image#*@sha256:}" "$revision"
}
