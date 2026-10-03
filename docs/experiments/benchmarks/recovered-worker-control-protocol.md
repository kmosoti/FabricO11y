# Recovered scheduling hypothesis: two-worker medium control

Registered before measurements. This recovers the concurrency/headroom hypothesis from [adaptive sealing](adaptive-sealing-hypotheses.md) and routes R5/R6 of the [pipeline experiment catalog](sealing-pipeline-experiment-design.md). All original branch experiments already exist as ancestors of the consolidated branch; no cherry-pick or dropped failure is needed.

After the registered [medium then enterprise simulations](medium-enterprise-local-protocol.md), repeat medium on fresh state with exactly two sealing workers. Retain all other workload, seed, affinity, clock, correctness and resource settings from that protocol. This is a static control, not implementation of the manifold/controller.

Exploratory H1: two workers reduce record-weighted collection-to-ingestion p99 by >10% while retaining correctness, server RSS <=2 GiB, and creation-lag p99 <=1 s. H0: those conditions are not all met. Report RSS, CPU, drain/backlog and scheduled latency costs regardless of outcome; single trials cannot establish significance or general causality. Generator/co-tenancy conditions can make the comparison inconclusive. Runtime/disk/evidence bounds and cleanup remain those of the main protocol. Protocol is separate from simulator/runner implementation.
