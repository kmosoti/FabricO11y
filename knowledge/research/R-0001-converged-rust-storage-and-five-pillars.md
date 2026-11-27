# R-0001 · Converged Pure-Rust Storage & Five Observability Pillars

## Storage Access Patterns & Implementation Tradeoffs

1. **OLTP & Adjacency Catalog (`redb = 3.1.3` + `live_state.json`):**
   - *Design:* `redb` provides ACID MVCC tables (`ENTITIES`, `ADJ_OUT`, `ADJ_IN`, `UNITS`, `WATERMARKS`, `ANOMALIES`, `META`) in pure Rust with zero C dependencies.
   - *Concurrency constraint observed:* `redb::Database::create` takes an exclusive advisory `flock` on the `.redb` file. While the daemon runs continuously, concurrent CLI queries (`fabric snapshot`, `fabric blame`, `fabric lineage`) query either `$XDG_RUNTIME_DIR/fabrico11y/daemon.sock` or the atomic `<data_dir>/state/live_state.json` snapshot written after every tick, while short-lived `CatalogStore` commits open `redb` with bounded backoff retry on `DatabaseAlreadyOpen`.
2. **Hot Time-Series Ring (`HotMetricRing <= 32 MiB`):**
   - *Design:* Fixed RAM budget (`32 MiB`) storing Gorilla-style delta-of-delta timestamp encoding and `f64` XOR bit packing per series key, evicting oldest series blocks before crossing the cap and flushing unflushed points every 60s (and on startup/shutdown) to ZSTD Parquet.
3. **OLAP & Retention (`arrow 59.2.0` + `parquet 59.2.0` + `datafusion 55.0.0`):**
   - *Design:* Date-partitioned (`dt=YYYY-MM-DD/part-*.parquet`) ZSTD-compressed columnar files under `<data_dir>/olap/{metrics,logs,traces,fs_events,edges}`. DataFusion queries execute under a `64 MiB` `GreedyMemoryPool` with disk spill routed to `<data_dir>/state/spill`.
   - *Janitor:* `RetentionJanitor` enforces `max_disk_bytes` (`4 GiB` default), compacts fragmented same-day part files, downsamples metrics partitions older than 7 days to 5-minute rollups, and evicts oldest `dt=*` partitions when over quota.

## Statistical Sketches & Five Pillars (`fabric-core`)

- **`DDSketch` (`alpha = 0.01`, `max_bins = 2048`):** Logarithmic bin indexing (`ceil(log_{gamma}(v))` with `gamma = (1+alpha)/(1-alpha)`) yields bounded relative quantile error `<= alpha` across arbitrary positive latency/byte distributions while enforcing a deterministic `2048`-bin memory cap via lowest-bin merging.
- **`EwmaMadDetector`:** Exponentially weighted baseline + MAD deviation provides outlier robustness against multi-modal spikes compared to sample standard deviation.
- **Five Data Observability Pillars (`Freshness`, `Volume`, `Schema`, `Distribution`, `Lineage`):** Evaluated purely in `fabric-core` from collected snapshots, `DDSketch` distributions, and unit/path attribution flags so findings are deterministic, unit-tested, and property-tested without kernel mocking overhead.

## Open Questions / Next Validation

- Benchmark multi-day ZSTD compaction ratios and DataFusion query latency under `MemoryMax=512M` inside `fabrico11y.slice` across 24+ hour workstation traces.

## Relations

- *supports*: [D-0002](../decisions/D-0002-cli-pivot-and-converged-telemetry-engine.md)
- *grounded-in*: [O-0001](../observations/O-0001-workstation-resource-postmortem.md)
