# Target-host qualification runbook

Everything that still needs your hardware, in the order to run it, with exact commands, pass rules and how to record the results. The work so far ran on a four-CPU cloud VM with a legacy cgroup hierarchy. On that host:

- the history, outage and stress protocols passed;
- the soak failed one gate;
- the installation acceptance was inconclusive.

> [!NOTE]
> Runs 2 to 6 ran on the 12-CPU target host from binaries frozen at `e68d6ce` on 2026-09-28 and 29: history, stress, outage and fleet are **Qualified**, and the soak **Failed** `no_rss_growth` as expected ([soak run 02](experiments/benchmarks/soak-run-02.md)). Run 1 has not run: that host has no Docker and no passwordless root. What remains is run 1, and the soak after the sealer fix. See the [capability ledger](QUALIFICATION.md#capability-ledger).

## What is left

| # | Run | Protocol | Why it is needed | Host requirement | Time |
| --- | --- | --- | --- | --- | --- |
| 1 | Installation acceptance | [installation acceptance](experiments/formal/installation-acceptance-protocol.md) | Decides A8h (`MemoryHigh`); every other check already passed | unified cgroup hierarchy, root, Docker | ~5 min |
| 2 | History, revision 1 | [history protocol](experiments/benchmarks/alpha-phase4-history-protocol.md) | Target-profile qualification of query latency, freshness and journal versus Segment | ≥ 6 logical CPUs; 12 is the target | ~30 min |
| 3 | Stress, revision 1 | [stress protocol](experiments/benchmarks/alpha-phase5-stress-protocol.md) | Target profile, now with burst latency reported | ≥ 6 CPUs | ~12 min |
| 4 | Outage | [outage protocol](experiments/benchmarks/alpha-phase5-outage-protocol.md) | Target-profile run of an already passing protocol | any | ~1 h 45 min |
| 5 | Soak, target placement | [soak protocol](experiments/benchmarks/soak-protocol.md) | Target profile. **Expect `no_rss_growth` to fail** until the sealer's working set is bounded ([soak run 01](experiments/benchmarks/soak-run-01.md)) | ≥ 6 CPUs | ~1 h 35 min |
| 6 | Fleet tiers (optional refresh) | [fleet protocol](experiments/benchmarks/alpha-phase3-fleet-protocol.md) | The last fleet run is on `4921e5e`, before the architecture work | ≥ 6 CPUs | ~25 min |

The target profile, from the [qualification](QUALIFICATION.md#registered-workload-and-decision-rule) page, is:

- the server has four logical CPUs (0 to 3), and the simulator and harness have the rest;
- the reference host has 12 logical CPUs, runs Debian 13, and uses ext4.

A pass on your PC counts as **Qualified** only if the host matches that profile. Otherwise record it as **Measured** and name the host.

## 1. Prepare the host

**Operating system.**

- Debian 13, natively or in WSL2 with systemd enabled (`[boot] systemd=true` in `/etc/wsl.conf`).
- Check the cgroup hierarchy: `stat -fc %T /sys/fs/cgroup` must print `cgroup2fs` for run 1.
- Report an I/O controller that is missing or ineffective, which is common on WSL. Never count it as enforcement.

**Packages.** `git build-essential pkg-config python3 openssl util-linux dpkg-dev docker.io` (use Docker Engine or Docker Desktop's WSL integration).

**Toolchains.**

```sh
rustup toolchain install stable 1.98.0 --profile minimal   # 1.98.0 is pinned by packaging/build-deb.sh
curl -fsSL https://bun.sh/install | bash -s bun-v1.4.0     # documentation checks only
```

For the extended checks (optional), install a JRE and fetch TLC v1.7.1 into `target/tools/tla2tools.jar`, checking its SHA-256 as the [delivery model README](../formal/delivery/README.md) describes.

**Clone and check.**

```sh
git clone https://github.com/kmosoti/FabricO11y && cd FabricO11y
git switch -c milestone/target-qualification
bun install --cwd tools/docs --frozen-lockfile
cargo xtask checks --profile fast        # must print "checks: PASSED"
```

**Host state.** Record the host in every run record: CPU model, logical CPUs, RAM, kernel (`uname -r`), distribution, filesystem, WSL or native, and cgroup type. Close other heavy programs. Runs are sequential, so never run two at once.

## 2. Freeze the artifacts

```sh
tools/qualification/freeze.sh alpha-q1-frozen
F=$PWD/target/alpha-q1-frozen
```

The script refuses uncommitted source or harness changes. It builds release binaries, then copies them, the examples the harnesses launch and the harness into `target/alpha-q1-frozen`, with `hashes.txt` and `source-commit.txt`. Before recording results, verify the frozen files with `(cd target/alpha-q1-frozen && sha256sum -c hashes.txt)`.

Every run below uses `$F`, and every output directory must be named `target/alpha-*`. The runner starts each child in its output directory, so always pass absolute paths (`$F`, `$PWD/...`).

## 3. Runs

Each run writes its summary to `<out>/*-summary.json` and its runner verdict to `<out>/result.json`. Copy the small JSON files, as listed under *Recording*, before deleting an output directory.

### Run 1: installation acceptance (needs root and cgroup v2)

```sh
RUSTUP_TOOLCHAIN=1.98.0 packaging/build-deb.sh target/alpha-install-deb
D=target/alpha-install-deb/fabrico11y_0.1.0~alpha.1_amd64.deb
sudo tools/qualification/install/run.sh $D target/alpha-install-q1/clean; echo exit $?
for m in root-user no-collision-check no-memory-max; do
  sudo tools/qualification/install/run.sh $D target/alpha-install-q1/$m --mutate $m; echo "$m exit $?"
done
```

**Pass rule.**

- The clean run exits `0` with `RESULT fails=0 not_run=0`, and A8h passes (`memory.high` of 128M on the node and 2816M on the slice).
- Each mutation exits `1` and fails at least its named check: A6, A2a/A2b and A8 respectively.

**Other outcomes.**

- Exit `3` on a cgroup v2 host is a harness defect; stop and report it.
- `run.sh` builds its container image from Debian's mirror. Behind an HTTPS proxy, set `HTTPS_PROXY` and `PROXY_CA_FILE`.

### Run 2: history, revision 1

Run revision 1 as registered: omit `--server-cpus` and `--sim-cpus`, whose defaults are `0-3` and `4-<last>`.

```sh
for t in "segment 1" "segment 2" "segment 3" "journal 1"; do set -- $t
  python3 -B tools/qualification/runner.py --out target/alpha-q1-history-$1-seed$2 --duration-s 1800 \
    --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
    python3 -B $F/tools/history_tier.py --seed 0xA11FA00$2 --mode $1 --bin-dir $F; echo "$t exit $?"
done
```

**Pass rule.** All four exit `0`. In each summary, every entry under `gates` is true: every oracle grade passes, each query kind has p99 ≤ 2 s, and freshness p99 ≤ 5 s in the Segment trials.

### Run 3: stress, revision 1

```sh
for n in 1 2 3; do
  python3 -B tools/qualification/runner.py --out target/alpha-q1-stress-seed$n --duration-s 1500 \
    --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
    python3 -B $F/tools/stress_tier.py --seed 0xA11FA00$n --bin-dir $F; echo "seed $n exit $?"
done
```

**Pass rule.** Every trial exits `0` with every gate true. Record `ack_ms.burst`; it is reported, not gated.

### Run 4: outage (registered protocol, no placement)

```sh
for n in 1 2 3; do
  python3 -B tools/qualification/runner.py --out target/alpha-q1-outage-seed$n --duration-s 3000 \
    --disk-bytes 1073741824 --max-output-bytes 1048576 -- \
    python3 -B $F/tools/outage_drain.py --seed 0xA11FA00$n --bin-dir $F; echo "seed $n exit $?"
done
```

**Pass rule.**

- Every trial exits `0`.
- Drain takes at most 600 s.
- Every line is committed, and the oracle passes.
- Node VmHWM stays at most 64 MiB.

### Run 5: soak, target placement

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-soak-seed1 --duration-s 7200 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/soak_tier.py --seed 0xA11FA001 --bin-dir $F --server-cpus 0-3 --sim-cpus 4-11
```

On a host with a different CPU count, use `--sim-cpus 4-<last>`.

**Pass rule.** All ten gates in `soak-summary.json` are true.

Until the sealer fix lands, `no_rss_growth` is expected to fail as it did in run 01. Record the result either way; it is still useful target-profile data. If you want a pass, land the sealer fix first, freeze again, and rerun unchanged.

### Run 6 (optional): fleet tiers

```sh
for tier in 10 100 1000; do for n in 1 2 3; do
  python3 -B tools/qualification/runner.py --out target/alpha-q1-fleet-$tier-seed$n --duration-s 900 \
    --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
    python3 -B $F/tools/fleet_tier.py --tier $tier --seed 0xA11FA00$n --bin-dir $F; echo "$tier/$n exit $?"
done; done
```

**Pass rule.** Every trial exits `0`, per the [fleet protocol](experiments/benchmarks/alpha-phase3-fleet-protocol.md).

## 4. Recording

Follow the evidence rule in [AGENTS.md](../AGENTS.md#evidence-rule): report the real command and exit, and never call a failed, skipped or non-target run "passed" or "qualified".

**Evidence.** For each run, copy the files below into `docs/experiments/benchmarks/data/target-qualification/<run>/`:

- `result.json`;
- the `*-summary.json`;
- `sim/sim-summary.json`, where present;
- for installation, `acceptance.txt`, `host.txt` and `deb.sha256`.

Also copy `hashes.txt` and `source-commit.txt`, and a `progress.txt` with start and end times.

**Run records.** Write one per protocol. Follow [history run 01](experiments/benchmarks/history-run-01.md) as the model: method, host, commands, results table, gate table, interpretation and limits. Name them `history-run-02.md`, `stress-run-02.md`, `outage-run-02.md`, `soak-run-02.md` and `installation-acceptance-run-02.md`.

**State.**

- **Qualified** only if every gate of the registered revision 1 passed on the target profile.
- **Measured** if everything passed on another host.
- **Failed** or **Inconclusive** as the protocol says.

**Update the records together.** The [capability ledger](QUALIFICATION.md#capability-ledger), [verification matrix](formal/verification-matrix.md) (QUAL rows), [current state](CURRENT.md) and [roadmap](ROADMAP.md). Then check the documentation with `bun tools/docs/check.mjs`.

**Commits and review.**

- Put evidence and run records in one commit, and document updates in another.
- A protocol or harness change is a trust-boundary change: register it before running, in its own commit.
- Push `milestone/target-qualification` and open a PR. CI runs the fast, documentation and extended checks.

**Failures.** Keep a failed gate's summary and write down the counterexample and the next action. Never loosen a gate, and never rerun until the result is acceptable.
