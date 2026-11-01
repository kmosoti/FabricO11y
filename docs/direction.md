# Product direction — proposed

Collect structured observations cheaply from Rust applications and agent harnesses.
Provide time-series/SQL, lexical, semantic and relationship views, with explicit
coverage, freshness and approximation. Distinguish facts, derived statistics,
hypotheses and forecasts. Similarity and temporal correlation do not prove causality.

Future schema/SDK work separates bounded memory acceptance from durable submission.
Spindle revalidates untrusted data and exposes custody and delivery receipts.
Finite buffers require explicit refusal, loss or backpressure during long outages.

Evaluate Weaver and OpenTelemetry semantics before creating schema tooling.
Compare engine-led custody (GreptimeDB candidate) with log-led replay only when
needed. Evaluate DataFusion/Parquet, redb, Tantivy, Qdrant/LanceDB and bounded graph
algorithms per capability, not as a compulsory stack. HTAP means transactional
control with explicit analytical progress. Forecasts need temporal holdouts.

Prove small semantic kernels with relevant Lean/Aeneas/Verus paths and connect
proofs to exact Rust code. Measure performance against equivalent baselines.
No existing proof sketch establishes product correctness. Archived Fabric code
can be reconsidered only through a new evidence-supported decision.
