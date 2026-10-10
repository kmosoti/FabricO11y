#!/bin/bash
# Running-installation acceptance, executed as root INSIDE a disposable
# Debian 13 systemd container or VM (see run.sh and run-qemu.py). Never run it
# on a real host: it
# creates and deletes the fabricolly account, installs and purges the package
# and starts services. Protocol:
# docs/experiments/formal/installation-acceptance-protocol.md.
#
# Usage: acceptance.sh <DEB> [--self-spindle-ca <CA-PATH>]
# Prints one line per check, "ACCEPT <ID> PASS|FAIL|NOT-RUN <detail>". Exits 1
# if any check failed, 3 if none failed but one could not run in this
# environment, and 0 only when every check ran and passed.
set -u
DEB=${1:?usage: acceptance.sh <DEB> [--self-spindle-ca <CA-PATH>]}
SELF_SPINDLE_CA=
if [ "${2:-}" = --self-spindle-ca ] && [ -n "${3:-}" ] && [ "$#" -eq 3 ]; then
  SELF_SPINDLE_CA=$3
  case "$SELF_SPINDLE_CA" in
    /*) ;;
    *) echo "self-spindle CA path must be absolute" >&2; exit 2 ;;
  esac
  case "$SELF_SPINDLE_CA" in
    *[!A-Za-z0-9_./-]*) echo "self-spindle CA path contains unsupported characters" >&2; exit 2 ;;
  esac
elif [ "$#" -ne 1 ]; then
  echo "usage: acceptance.sh <DEB> [--self-spindle-ca <CA-PATH>]" >&2
  exit 2
fi
FAILS=0
NOTRUN=0
UNIFIED=0; [ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ] && UNIFIED=1
W=/root/accept
mkdir -p "$W"
LOGDIR=/var/log/fabric-accept

pass() { echo "ACCEPT $1 PASS ${*:2}"; }
fail() { echo "ACCEPT $1 FAIL ${*:2}"; FAILS=$((FAILS + 1)); }
notrun() { echo "ACCEPT $1 NOT-RUN ${*:2}"; NOTRUN=$((NOTRUN + 1)); }
wait_for() { # wait_for <seconds> <command...>
  local limit=$1; shift
  local end=$((SECONDS + limit))
  until "$@" >/dev/null 2>&1; do
    [ $SECONDS -ge $end ] && return 1
    sleep 1
  done
}
admin() { fabricctl admin "$W/admin.conf" "$@"; }
query() { admin query "$1"; }
show() { systemctl show -P "$2" "$1"; }

echo "ENV systemd $(systemctl --version | head -1 | cut -d' ' -f2) debian $(cat /etc/debian_version) kernel $(uname -r)"
echo "ENV cgroup $(stat -fc %T /sys/fs/cgroup) controllers: $(cat /sys/fs/cgroup/cgroup.controllers 2>/dev/null)"

# A1 sysusers: dry run and a temporary root, applied twice.
x=$W/pkg; rm -rf "$x"; dpkg-deb -x "$DEB" "$x"
conf=$x/usr/lib/sysusers.d/fabrico11y.conf
r=$W/sysroot; rm -rf "$r"; mkdir -p "$r/etc"
dry=$(systemd-sysusers --dry-run --root="$r" "$conf" 2>&1); dry_rc=$?
dry_clean=0; [ -e "$r/etc/passwd" ] && dry_clean=1
systemd-sysusers --root="$r" "$conf" >/dev/null 2>&1 && first=$(grep '^fabricolly:' "$r/etc/passwd")
systemd-sysusers --root="$r" "$conf" >/dev/null 2>&1 && second=$(grep '^fabricolly:' "$r/etc/passwd")
if [ $dry_rc -eq 0 ] && echo "$dry" | grep -q fabricolly && [ $dry_clean -eq 0 ] \
   && [ -n "${first:-}" ] && [ "$first" = "${second:-}" ] \
   && echo "$first" | grep -Eq '^fabricolly:x:[0-9]+:[0-9]+:Fabric O11y service:/:/usr/sbin/nologin$'; then
  pass A1 "temporary root: $first"
else
  fail A1 "dry_rc=$dry_rc first='${first:-}' second='${second:-}'"
fi

# A2 collision refusal: a human account, then a non-system group, named fabricolly.
useradd -m -u 1500 -s /bin/bash fabricolly
out=$(dpkg -i "$DEB" 2>&1); rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "refusing to install" && [ ! -e /usr/bin/fabric-server ]; then
  pass A2a "human account uid 1500 refused, nothing unpacked"
else
  fail A2a "rc=$rc"
fi
dpkg --purge fabrico11y >/dev/null 2>&1; userdel -r fabricolly 2>/dev/null
groupadd -g 1600 fabricolly
out=$(dpkg -i "$DEB" 2>&1); rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "refusing to install" && [ ! -e /usr/bin/fabric-server ]; then
  pass A2b "non-system group gid 1600 refused, nothing unpacked"
else
  fail A2b "rc=$rc"
fi
dpkg --purge fabrico11y >/dev/null 2>&1; groupdel fabricolly 2>/dev/null

# A3 install, then install again: same identity, exit 0 both times.
dpkg -i "$DEB" >"$W/install1.log" 2>&1; rc1=$?
id1=$(getent passwd fabricolly)
dpkg -i "$DEB" >"$W/install2.log" 2>&1; rc2=$?
id2=$(getent passwd fabricolly)
uid=$(id -u fabricolly 2>/dev/null); gid=$(id -g fabricolly 2>/dev/null); pg=$(id -gn fabricolly 2>/dev/null)
if [ $rc1 -eq 0 ] && [ $rc2 -eq 0 ] && [ "$id1" = "$id2" ] && [ "$uid" -lt 1000 ] && [ "$uid" -gt 0 ] \
   && [ "$pg" = fabricolly ] && [ "$(getent passwd fabricolly | cut -d: -f7)" = /usr/sbin/nologin ]; then
  pass A3 "installed twice; $id1"
else
  fail A3 "rc1=$rc1 rc2=$rc2 id1='$id1' id2='$id2'"
fi
etc_mode=$(stat -c '%a %U:%G' /etc/fabrico11y)
[ "$etc_mode" = "750 root:fabricolly" ] && pass A3b "/etc/fabrico11y $etc_mode" || fail A3b "/etc/fabrico11y $etc_mode"

# A4 systemd-analyze verify on the installed units and slice.
out=$(systemd-analyze verify /usr/lib/systemd/system/fabrico11y-node.service \
      /usr/lib/systemd/system/fabrico11y-server.service /usr/lib/systemd/system/system-fabrico11y.slice 2>&1); rc=$?
[ $rc -eq 0 ] && pass A4 "exit 0" || fail A4 "rc=$rc $out"

# Configuration: a private CA, the server certificate for 127.0.0.1, the admin
# token, and one authorized and one denied log.
cd "$W"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.pem -days 2 \
  -subj "/CN=fabric accept CA" -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout server.key -out server.csr \
  -subj "/CN=127.0.0.1" 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' > san.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 2 -extfile san.ext 2>/dev/null
install -m 0644 -o root -g root ca.pem server.pem /etc/fabrico11y/
install -m 0640 -o root -g fabricolly server.key /etc/fabrico11y/
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > admin-token
install -m 0640 -o root -g fabricolly admin-token /etc/fabrico11y/admin-token
cat > /etc/fabrico11y/server.conf <<EOF
listen=127.0.0.1:7443
tls_cert=/etc/fabrico11y/server.pem
tls_key=/etc/fabrico11y/server.key
admin_token_file=/etc/fabrico11y/admin-token
state_dir=/var/lib/fabrico11y/server
journal_bytes=21474836480
retention_s=86400
retention_bytes=21474836480
EOF
if [ -n "$SELF_SPINDLE_CA" ]; then
  printf 'self_spindle_ca=%s\n' "$SELF_SPINDLE_CA" >> /etc/fabrico11y/server.conf
fi
chmod 0644 /etc/fabrico11y/server.conf
printf 'server_url=https://127.0.0.1:7443\nserver_ca=/etc/fabrico11y/ca.pem\nadmin_token_file=%s/admin-token\n' "$W" > admin.conf
mkdir -p $LOGDIR && chmod 0755 $LOGDIR
install -m 0600 /dev/null $LOGDIR/allowed.log
install -m 0600 /dev/null $LOGDIR/denied.log
setfacl -m u:fabricolly:r $LOGDIR/allowed.log

systemctl enable --now fabrico11y-server.service >/dev/null 2>&1
wait_for 30 curl -fsS --cacert /etc/fabrico11y/ca.pem -H "authorization: Bearer $(cat admin-token)" \
  https://127.0.0.1:7443/v1/admin/nodes || fail SETUP "server did not answer"
enroll=$(admin node add spindle-1 --log $LOGDIR/allowed.log --log $LOGDIR/denied.log --interval 15)
token=$(echo "$enroll" | grep -o '"token":"[^"]*"' | cut -d'"' -f4)
printf '%s\n' "$token" > node-token
install -m 0640 -o root -g fabricolly node-token /etc/fabrico11y/node-token
cat > /etc/fabrico11y/node.conf <<EOF
spool_dir=/var/lib/fabrico11y/node/spool
metric_interval_s=15
spool_bytes=268435456
log=$LOGDIR/allowed.log
log=$LOGDIR/denied.log
server_url=https://127.0.0.1:7443
server_ca=/etc/fabrico11y/ca.pem
token_file=/etc/fabrico11y/node-token
EOF
chmod 0644 /etc/fabrico11y/node.conf
echo "accept-line-before-start" >> $LOGDIR/allowed.log
echo "secret-line-never-read" >> $LOGDIR/denied.log
systemctl enable --now fabrico11y-node.service >/dev/null 2>&1

# A5 TLS: the admin API refuses an unverified client and answers a verified one;
# the node delivers over TLS (rows below).
curl -sS -o /dev/null https://127.0.0.1:7443/v1/admin/nodes 2>/dev/null; untrusted=$?
code=$(curl -sS -o /dev/null -w '%{http_code}' --cacert /etc/fabrico11y/ca.pem \
  -H "authorization: Bearer $(cat admin-token)" https://127.0.0.1:7443/v1/admin/nodes)
[ $untrusted -eq 60 ] && [ "$code" = 200 ] && pass A5 "untrusted client exit 60; CA-verified 200" \
  || fail A5 "untrusted=$untrusted verified=$code"

logs_query() { query "{\"kind\":\"logs\",\"node\":\"spindle-1\",\"from_ns\":0,\"to_ns\":9223372036854775807,\"limit\":1000}"; }
has_line() { logs_query | grep -q "$1"; }

# A6 identity of the running services: non-root UID and primary GID, no capabilities.
wait_for 60 has_line accept-line-before-start
ok=1; detail=""
for unit in fabrico11y-node fabrico11y-server; do
  pid=$(show $unit.service MainPID)
  u=$(awk '/^Uid:/{print $2}' /proc/$pid/status); g=$(awk '/^Gid:/{print $2}' /proc/$pid/status)
  cap=$(awk '/^CapEff:/{print $2}' /proc/$pid/status); nnp=$(awk '/^NoNewPrivs:/{print $2}' /proc/$pid/status)
  detail="$detail $unit:uid=$u,gid=$g,CapEff=$cap,NoNewPrivs=$nnp"
  [ "$u" = "$uid" ] && [ "$g" = "$gid" ] && [ "$u" != 0 ] && [ "$cap" = 0000000000000000 ] && [ "$nnp" = 1 ] || ok=0
done
[ $ok = 1 ] && pass A6 "$detail" || fail A6 "$detail"

# A7 managed directories: owner fabricolly:fabricolly, mode 0700.
ok=1; detail=""
for d in /var/lib/fabrico11y/node /var/lib/fabrico11y/server /run/fabrico11y/node /run/fabrico11y/server; do
  m=$(stat -c '%a %U:%G' "$d" 2>/dev/null || echo missing)
  detail="$detail $d=$m"
  [ "$m" = "700 fabricolly:fabricolly" ] || ok=0
done
[ $ok = 1 ] && pass A7 "$detail" || fail A7 "$detail"

# A8 cgroup placement, effective limits and accounting. On the unified
# hierarchy the files are memory.max/memory.high/pids.max; on the legacy
# hierarchy memory.limit_in_bytes and pids.max, and MemoryHigh has no kernel
# counterpart, so its enforcement (A8h) is not run there.
ok=1; detail=""
cgroup_of() { # pid -> the service's cgroup path in the systemd tree
  if [ "$UNIFIED" = 1 ]; then sed -n 's/^0:://p' /proc/$1/cgroup; else sed -n 's/^[0-9]*:name=systemd://p' /proc/$1/cgroup; fi
}
mem_max_file() { [ "$UNIFIED" = 1 ] && echo "/sys/fs/cgroup$1/memory.max" || echo "/sys/fs/cgroup/memory$1/memory.limit_in_bytes"; }
pids_max_file() { [ "$UNIFIED" = 1 ] && echo "/sys/fs/cgroup$1/pids.max" || echo "/sys/fs/cgroup/pids$1/pids.max"; }
expect() { # unit high max tasks
  local unit=$1 pid cg
  pid=$(show "$unit" MainPID); cg=$(cgroup_of "$pid")
  detail="$detail $unit:cgroup=$cg"
  [ "$cg" = "/system.slice/system-fabrico11y.slice/$unit" ] || ok=0
  for kv in "EffectiveMemoryHigh=$2" "EffectiveMemoryMax=$3" "EffectiveTasksMax=$4"; do
    local got; got=$(show "$unit" "${kv%%=*}")
    detail="$detail ${kv%%=*}=$got"
    [ "$got" = "${kv#*=}" ] || ok=0
  done
  for acct in MemoryCurrent TasksCurrent; do
    local got; got=$(show "$unit" $acct)
    detail="$detail $acct=$got"
    case "$got" in ''|'[not set]'|18446744073709551615) ok=0 ;; esac
  done
  local got_max got_pids
  got_max=$(cat "$(mem_max_file "$cg")" 2>/dev/null); got_pids=$(cat "$(pids_max_file "$cg")" 2>/dev/null)
  detail="$detail kernel-memory-max=$got_max kernel-pids-max=$got_pids"
  [ "$got_max" = "$3" ] && [ "$got_pids" = "$4" ] || ok=0
}
expect fabrico11y-node.service $((128 << 20)) $((256 << 20)) 128
expect fabrico11y-server.service $((2560 << 20)) $((3072 << 20)) 512
slice=/system.slice/system-fabrico11y.slice
s_max=$(cat "$(mem_max_file $slice)" 2>/dev/null); s_pids=$(cat "$(pids_max_file $slice)" 2>/dev/null)
detail="$detail slice kernel-memory-max=$s_max kernel-pids-max=$s_pids"
[ "$s_max" = $((3328 << 20)) ] && [ "$s_pids" = 640 ] || ok=0
[ $ok = 1 ] && pass A8 "$detail" || fail A8 "$detail"
if [ "$UNIFIED" = 1 ]; then
  n_high=$(cat "/sys/fs/cgroup$(cgroup_of "$(show fabrico11y-node.service MainPID)")/memory.high")
  s_high=$(cat /sys/fs/cgroup$slice/memory.high)
  [ "$n_high" = $((128 << 20)) ] && [ "$s_high" = $((2816 << 20)) ] \
    && pass A8h "node memory.high=$n_high slice memory.high=$s_high" || fail A8h "node=$n_high slice=$s_high"
  echo "REPORT controllers slice=[$(cat /sys/fs/cgroup$slice/cgroup.controllers)]"
else
  notrun A8h "legacy cgroup hierarchy: MemoryHigh has no kernel counterpart (systemd reports the value only)"
fi
echo "REPORT IOReadBytes node=$(show fabrico11y-node.service IOReadBytes) (I/O accounting and weights are reported, never counted as enforcement)"

# A9 host metrics and the authorized log, read inside the sandbox.
metric_query='{"kind":"metrics","node":"spindle-1","name":"system.memory.available","from_ns":0,"to_ns":9223372036854775807,"limit":10}'
wait_for 60 sh -c "fabricctl admin $W/admin.conf query '$metric_query' | grep -q '\"time_ns\"'"
m=$(query "$metric_query")
if echo "$m" | grep -q '"time_ns"' && has_line accept-line-before-start; then
  pass A9 "system.memory.available points and the authorized log line delivered"
else
  fail A9 "metrics: $(echo "$m" | head -c 300)"
fi

# A10 the denied log is a visible gap and its content is never delivered.
wait_for 60 sh -c "fabricctl admin $W/admin.conf query '{\"kind\":\"logs\",\"node\":\"spindle-1\",\"from_ns\":0,\"to_ns\":9223372036854775807,\"limit\":1000}' | grep -q 'denied.log'"
answer=$(logs_query)
gap=$(echo "$answer" | grep -o 'log source unavailable [^"]*denied.log[^"]*' | head -1)
if [ -n "$gap" ] && ! echo "$answer" | grep -q secret-line-never-read; then
  pass A10 "gap: $gap"
else
  fail A10 "no denied-log gap or secret delivered"
fi

# A11 graceful shutdown and restart with a retained Spool.
systemctl stop fabrico11y-server.service
server_stop=$(show fabrico11y-server.service Result)
echo "accept-line-during-outage" >> $LOGDIR/allowed.log
sleep 20
systemctl restart fabrico11y-node.service
node_stop=$(show fabrico11y-node.service Result)
spool_files=$(find /var/lib/fabrico11y/node/spool -type f | wc -l)
systemctl start fabrico11y-server.service
wait_for 90 has_line accept-line-during-outage; delivered=$?
has_line accept-line-before-start; kept=$?
if [ "$server_stop" = success ] && [ "$node_stop" = success ] && [ "$spool_files" -gt 0 ] \
   && [ $delivered -eq 0 ] && [ $kept -eq 0 ]; then
  pass A11 "server stop Result=$server_stop; node restart Result=$node_stop; spool files=$spool_files; outage line delivered after restart; earlier line retained"
else
  fail A11 "server=$server_stop node=$node_stop spool=$spool_files delivered=$delivered kept=$kept"
fi

# A12 memory and task pressure are contained in the slice; the services survive.
n_before=$(show fabrico11y-node.service NRestarts); s_before=$(show fabrico11y-server.service NRestarts)
# hog <unit> <limit bytes> <systemd-run properties...>: runs `tail /dev/zero`
# (unbounded memory) and samples MemoryCurrent until it ends. Contained means
# the peak stayed within the limit and the kernel killed it: Result=oom-kill
# on the unified hierarchy; on the legacy one systemd cannot see OOM events,
# so a SIGKILL (Result=signal, status 9) before RuntimeMaxSec is required.
hog() {
  local unit=$1 limit=$2; shift 2
  systemd-run --quiet --unit="$unit" --slice=system-fabrico11y.slice -p RuntimeMaxSec=300 "$@" tail /dev/zero
  local peak=0 cur
  while [ "$(systemctl is-active "$unit" 2>/dev/null)" = active ]; do
    cur=$(show "$unit" MemoryCurrent); case "$cur" in ''|'[not set]') ;; *) [ "$cur" -gt "$peak" ] && peak=$cur ;; esac
    sleep 0.2
  done
  local result status
  result=$(show "$unit" Result); status=$(show "$unit" ExecMainStatus)
  systemctl reset-failed "$unit" 2>/dev/null
  local contained=0
  if [ "$peak" -le "$limit" ] && { [ "$result" = oom-kill ] || { [ "$UNIFIED" = 0 ] && [ "$result" = signal ] && [ "$status" = 9 ]; }; }; then
    contained=1
  fi
  echo "$contained $result/$status peak=$peak limit=$limit"
}
mem=$(hog fabric-accept-mem $((256 << 20)) -p MemoryHigh=128M -p MemoryMax=256M -p TasksMax=128)
slice_hog=$(hog fabric-accept-slice $((3328 << 20)))
# A process tree that tries to exceed the Spindle's task limit.
systemd-run --quiet --unit=fabric-accept-tasks --slice=system-fabrico11y.slice -p TasksMax=128 \
  bash -c 'for i in $(seq 300); do sleep 120 & done; wait' >/dev/null 2>&1
tasks_now=0; end=$((SECONDS + 20))
while [ $SECONDS -lt $end ]; do
  cur=$(show fabric-accept-tasks.service TasksCurrent 2>/dev/null)
  case "$cur" in ''|*[!0-9]*) ;; *) [ "$cur" -gt "$tasks_now" ] && tasks_now=$cur ;; esac
  sleep 0.2
done
fork_errors=$(journalctl -q -u fabric-accept-tasks.service --no-pager 2>/dev/null | grep -ci "fork")
systemctl stop fabric-accept-tasks.service 2>/dev/null; systemctl reset-failed fabric-accept-tasks.service 2>/dev/null
n_after=$(show fabrico11y-node.service NRestarts); s_after=$(show fabrico11y-server.service NRestarts)
alive=$(systemctl is-active fabrico11y-node.service fabrico11y-server.service | tr '\n' ' ')
echo "accept-line-after-pressure" >> $LOGDIR/allowed.log
wait_for 60 has_line accept-line-after-pressure; after=$?
if [ "${mem%% *}" = 1 ] && [ "${slice_hog%% *}" = 1 ] && [ "$tasks_now" -le 128 ] && [ "$tasks_now" -ge 100 ] \
   && [ "$fork_errors" -gt 0 ] && [ "$n_before" = "$n_after" ] && [ "$s_before" = "$s_after" ] \
   && [ "$alive" = "active active " ] && [ $after -eq 0 ]; then
  pass A12 "service-limit hog: ${mem#* }; slice-limit hog: ${slice_hog#* }; task tree peaked at TasksCurrent=$tasks_now with $fork_errors fork-failure lines; services active, NRestarts unchanged, delivery continued"
else
  fail A12 "mem=[$mem] slice=[$slice_hog] tasks=$tasks_now forks=$fork_errors restarts node $n_before->$n_after server $s_before->$s_after alive='$alive' after=$after"
fi

# A13 remove keeps data, configuration and the account; purge removes data and
# configuration.
dpkg -r fabrico11y >"$W/remove.log" 2>&1; rc=$?
stopped=$(systemctl is-active fabrico11y-node.service fabrico11y-server.service | tr '\n' ' ')
if [ $rc -eq 0 ] && [ ! -e /usr/bin/fabric-server ] && [ -d /var/lib/fabrico11y/server ] \
   && [ -f /etc/fabrico11y/server.conf ] && getent passwd fabricolly >/dev/null && [ "$stopped" != "active active " ]; then
  pass A13a "remove: binaries gone, services '$stopped', state, configuration and account kept"
else
  fail A13a "rc=$rc stopped='$stopped'"
fi
dpkg -P fabrico11y >"$W/purge.log" 2>&1; rc=$?
if [ $rc -eq 0 ] && [ ! -e /var/lib/fabrico11y ] && [ ! -e /etc/fabrico11y ]; then
  pass A13b "purge: /var/lib/fabrico11y and /etc/fabrico11y removed"
else
  fail A13b "rc=$rc"
fi

echo "RESULT fails=$FAILS not_run=$NOTRUN"
[ $FAILS -gt 0 ] && exit 1
[ $NOTRUN -gt 0 ] && exit 3
exit 0
