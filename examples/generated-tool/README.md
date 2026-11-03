# Generated tool integration

Generated Rust tools call `Client::try_record(Data::ToolExecution { ... }, now_ns())`
using the harness-owned client. See `fabric/examples/agent_harness.rs`. The harness
flushes once at its durable boundary; each tool checks admission refusal.

Generated Python tools call the harness `record(process, data)` helper through the
persistent adapter. See `examples/agent_harness.py`. Keep one adapter per harness,
check each admission response and require a successful flush receipt before
reporting the run as durably collected. Instrument tool name, run identity,
elapsed nanoseconds and outcome; never put credentials or tool arguments in an
observation by default.
