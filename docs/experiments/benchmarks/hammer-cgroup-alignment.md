# Decimal memory-cap alignment

The first pressure attempt failed closed before starting any product process:
Linux rounded the requested decimal 4 GB/3 GB limits down to page boundaries,
and the existing exact enforcement check rejected the difference. Its failed
receipt remains recorded. The harness now computes the page-aligned floor
before requesting the limits, still verifying exact equality. On this host
the actual maximum/high values are 3,999,997,952 / 2,999,996,416 bytes, both below
the owner's caps. The workload and all semantic checks are unchanged.

The retry has a distinct identifier. Remote local-server setup uses the same
page-aligned floor; remote MiB limits already align. This corrects launcher
configuration without changing the cgroup verifier or product behavior.
