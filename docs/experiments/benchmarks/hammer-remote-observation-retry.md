# Remote observation admission and network diagnostics

The first real-edge attempt verified its 128 MiB memory, 50% CPU and ten-minute
deadline, then stopped in resource sampling with a missing-file exception.
Read-only inspection found the droplet's system slice exposes CPU/memory/pids
controllers but no IO controller without IO accounting. The runner had sampled
`io.stat` without requesting IO accounting. All children and remote scratch
were cleaned after the complete failed remote tree was authenticated locally.

The retry requests `IOAccounting=yes` on its owned transient services and checks
every required resource counter before starting project binaries. Sampler errors
now preserve the failing path and traceback. The original exception lacks its
file path, so attributing that attempt to `io.stat` is an inference supported
by the inspected environment, not an observed traceback. Workloads, memory/CPU
limits, expected results and independent oracles remain unchanged.

The owner additionally requested measured Tailscale latency and bandwidth.
Before continuing edge trials, inspect `tailscale ping` direct/relay status,
then use one existing-private-path SSH connection to digitalocean-02 for thirty
one-byte echo round trips and three 16 MiB transfers in each direction.
Compression is disabled. A remote 128 MiB, 50%-CPU, zero-swap, 128-task,
180-second transient service owns the transfer endpoint, verifying limits first;
the local command remains in the laboratory cgroup. Record establishment time
separately, verify transferred length and bytes, and report payload throughput
including SSH/Tailscale encryption and endpoint scheduling rather than claiming
raw-link capacity. Transfers use memory buffers and no fixture storage. There
is no bandwidth load on the webserver droplet, firewall change or new software.
