# Evaluation

Evaluation proves semantic correctness before throughput. The initial evaluation matrix will cover:

- replay determinism;
- correction propagation against a full-rebuild oracle;
- frontier accuracy under gaps, late arrivals, sampling, and retention;
- segment corruption and recovery behavior;
- compression ratio and cost by payload family and dictionary version;
- indexed query latency without archive-scan fallback;
- Rust and Python API parity.

Results must record fixture provenance, tool versions, configuration, and whether caches were warm
or cold. Performance claims are invalid when canonical outputs differ from the correctness oracle.
