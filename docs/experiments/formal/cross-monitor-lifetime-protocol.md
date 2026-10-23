# Cross-family resource monitor lifetime diagnostic

Prospective correctness diagnostic, 2026-10-22. The stopped
`split-debian-fedora-smoke-04` reached offered work and captured its phase/query
evidence, then failed before resource extraction with guest command status 5.
Both QEMU lifetimes and the original remote worker have confirmed shutdown.
The run remains failed and receives no acceptance credit.

Hypothesis: the bounded 330-sample monitor finished while post-drain queries
were still being collected. Its successful transient systemd unit was unloaded,
so the later combined monitor/node stop returned status 5. Do not infer the
monitor's exit status from transport or from the product service's exit.

Use one diagnostic clone overlay over the owned stopped Fedora edge image.
Pin and recheck that backing image's SHA-256; keep it unchanged. Reuse Guest
provisioning/SSH and G15 KVM with two CPUs, 1,536 MiB guest RAM and the actual
2 GiB outer job profile. Start no other guest. Stop cloned product services
before inspecting its old resource file and available journal. This clone is
diagnostic input, not a fresh official-image acceptance cell or a replay of
the stopped trial.

In that disposable guest, run short independently observable systemd controls:
an ordinary successful transient command, the same command with
`RemainAfterExit=yes`, and a failing retained command. Capture original manager
status and stop exits. Require the retained positive to expose successful exit
status and stop cleanly; require the failing command to remain distinguishable
and be refused by the proposed collector health check. Keep named control units
bounded and clean them and the clone afterward, retaining evidence first.

No sampler horizon, cadence, workload, oracle, threshold or negative expectation
changes. A lifecycle repair must retain the monitor's original status until
collection and reject failed/missing state. Rerun affected source controls and
a fresh full split smoke on official images before placement admission. A
diagnostic result cannot promote smoke04 to success.

The [literal states](fixtures/cross-monitor-states.json) are the regression
inputs for this mechanism; the diagnostic's full artifacts remain separate.
