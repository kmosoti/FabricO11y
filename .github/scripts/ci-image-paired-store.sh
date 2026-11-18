#!/usr/bin/env bash
# shellcheck disable=SC2154
# Sourced by ci-image-paired-lane.sh on a disposable hosted Ubuntu runner.

docker_lane() {
  sudo -n /usr/bin/docker --config "$run_root/anonymous" --host "unix://$daemon_root/d.sock" "$@"
}

ctr_lane() {
  sudo -n /usr/bin/ctr --address "$daemon_root/c.sock" "$@"
}

start_daemon() {
  local stock_driver stock_backend socket
  local -a driver feature
  read -r stock_driver stock_backend < "$run_root/storage"
  driver=(--storage-driver "$stock_driver")
  feature=()
  if [[ "$stock_backend" == snapshotter ]]; then
    driver=()
    feature=(--feature containerd-snapshotter=true)
  fi
  if [[ -e "$daemon_root" ]]; then
    echo "Docker state already exists for $PAIR_ROLE" >&2
    return 1
  fi

  mkdir "$daemon_root"
  printf '{}\n' > "$daemon_root/daemon.json"
  # The host runner owns the evidence and writes the two daemon logs.
  # shellcheck disable=SC2024
  sudo -n bash -c 'printf "%s\n" "$$" > "$1"; shift; exec "$@"' bash "$daemon_root/containerd.pid" \
    /usr/bin/containerd --root "$daemon_root/content" --state "$daemon_root/runtime" \
    --address "$daemon_root/c.sock" > "$evidence/containerd.log" 2>&1 &
  wait_until_ready ctr_lane version
  # shellcheck disable=SC2024
  sudo -n /usr/bin/dockerd --config-file "$daemon_root/daemon.json" \
    --data-root "$daemon_root/images" --exec-root "$daemon_root/exec" \
    --pidfile "$daemon_root/dockerd.pid" --host "unix://$daemon_root/d.sock" \
    --containerd "$daemon_root/c.sock" "${driver[@]}" "${feature[@]}" \
    > "$evidence/dockerd.log" 2>&1 &
  wait_until_ready docker_lane info
  socket=$(docker_lane info --format '{{.DockerRootDir}}')
  [[ "$socket" == "$daemon_root/images" ]]
  docker_lane info --format '{{.Driver}} {{json .DriverStatus}}' > "$evidence/private-storage.txt"
  docker_lane version --format '{{json .Server}}' > "$evidence/private-docker.json"
  ctr_lane version > "$evidence/private-containerd.txt"
  if [[ "$(cat "$run_root/evidence/docker-storage.txt")" != "$(<"$evidence/private-storage.txt")" ]]; then
    echo "Private Docker storage differs from the stock hosted runner" >&2
    return 1
  fi
  docker_lane image ls -aq > "$evidence/images-before.txt"
  ctr_lane content ls -q > "$evidence/content-before.txt"
  ctr_lane --namespace moby content ls -q > "$evidence/moby-content-before.txt"
  ctr_lane --namespace moby snapshots ls -q > "$evidence/moby-snapshots-before.txt"
  [[ ! -s "$evidence/images-before.txt" && ! -s "$evidence/content-before.txt" &&
     ! -s "$evidence/moby-content-before.txt" && ! -s "$evidence/moby-snapshots-before.txt" ]]
  printf 'daemon=%s\ncontainerd=%s\nimages=%s\ncontent=%s\n' \
    "$daemon_root/d.sock" "$daemon_root/c.sock" "$daemon_root/images" "$daemon_root/content" \
    > "$evidence/empty-store.txt"
  df -Pk "$daemon_root" "$host_workspace" > "$evidence/disk-before.tsv"
}

wait_until_ready() {
  local attempt
  for ((attempt = 0; attempt < 30; attempt++)); do
    if "$@" >/dev/null 2>&1; then
      return
    fi
    sleep 1
  done
  echo "Timed out waiting for $*" >&2
  return 1
}

pull_image() {
  docker_lane pull --platform linux/amd64 "$image" | tee "$evidence/pull.log"
  if grep -qi 'Already exists' "$evidence/pull.log"; then
    echo "The supposedly empty image store reused a layer" >&2
    return 1
  fi
  grep -Fqx "Digest: ${image#*@}" "$evidence/pull.log"
  ctr_lane --namespace moby content ls -q > "$evidence/moby-content-after.txt"
  docker_lane image inspect --format '{{json .RootFS.Layers}}' "$image" > "$evidence/layers.json"
  docker_lane run --pull never --platform linux/amd64 --detach --name workload --init \
    --mount "type=bind,src=$host_workspace,dst=$workspace" \
    --mount "type=bind,src=$host_home,dst=/github/home" \
    --mount "type=bind,src=$host_temp,dst=/__w/_temp" \
    --workdir "$workspace" --entrypoint /bin/sh \
    --env HOME=/github/home --env "GITHUB_WORKSPACE=$workspace" --env RUNNER_TEMP=/__w/_temp \
    --env CI=true --env GITHUB_ACTIONS=true --env TMPDIR=/var/tmp \
    --env CARGO_BUILD_JOBS=4 --env CARGO_INCREMENTAL=0 \
    --env CARGO_PROFILE_DEV_DEBUG=0 --env CARGO_PROFILE_TEST_DEBUG=0 --env LEAN_NUM_THREADS=4 \
    --env FABRIC_FUZZ_TARGET_DIR=/github/home/.cache/fabrico11y-fuzz-target \
    "$image" -c 'exec sleep infinity' > "$evidence/container-id.txt"
}

stop_process() {
  local pidfile=$1 pid attempt
  [[ -f "$pidfile" ]] || return 0
  pid=$(<"$pidfile")
  [[ "$pid" =~ ^[1-9][0-9]{1,7}$ ]]
  if ! sudo -n kill -0 "$pid" 2>/dev/null; then
    return 0
  fi
  sudo -n kill -TERM "$pid"
  for ((attempt = 0; attempt < 30; attempt++)); do
    if ! sudo -n kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  printf 'Private daemon PID %s did not stop\n' "$pid" >&2
  return 1
}

cleanup_daemon() {
  if [[ ! -d "$daemon_root" ]]; then
    return
  fi
  if [[ -S "$daemon_root/d.sock" ]]; then
    docker_lane rm --force workload >/dev/null 2>&1 || true
  fi
  stop_process "$daemon_root/dockerd.pid" || return
  stop_process "$daemon_root/containerd.pid" || return
  sudo -n rm -rf -- "$daemon_root"
}
