# FabricO11y development

Read README.md and docs/direction.md. Product work is queued in local backlog/.
Use Rust for repository utilities and tests in xtask. Keep CI bootstrap scripts
under .github and agent configuration under .codex. Prefer maintained native tools
over new frameworks.

Use the existing user-installed jj from PATH with its Git backend for snapshots,
commits and bookmarks. Do not require a system package or change the global shell
PATH. Keep archive/* bookmarks local and backlog/ untracked.

Inspect status and conflicts before changes. Snapshot before capturing a concrete
commit ID; change IDs do not identify verified source. A new Jujutsu workspace
creates a working commit: compare its contents with the captured parent.
Rewrites invalidate receipts. Use cargo xtask candidate for capture and verification.
Publish only an explicitly authorized bookmark. Successful verification does not
grant publication permission. Preserve secrets and user-owned data.

Use native Cargo and checker commands. Choose and record resource settings for
expensive work; no mandatory launcher or fixed memory, swap or time ceiling applies.
Skills guide procedures; LogicGate coordinates supported evidence; native tools
establish results. Report commands, exits, assumptions and unavailable capabilities.
Keep specification changes separate from implementation.

Name files, workflows and components by their responsibility, not a temporary
phase. Keep commentary brief and focused on outcomes, decisions and material blockers.
Write project documentation for its readers: product behavior, interfaces, setup
and contribution procedures. Keep owner-specific preferences, conversation history,
cleanup receipts and agent process narration out of project docs. Put agent operating
rules in AGENTS.md and operational records in local artifacts.

For substantive architecture or optimization, write a requirements-only brief
before selecting a mechanism: observable behavior, invariants, workloads,
acceptance criteria and uncertainty. Treat current code as a measured baseline.
Before extensive refinement, implement structurally different candidates and
record differences in algorithm, representation, ownership and data flow.
Record a parent, captured concrete source, governing contract digest, expected
advantage and a falsifying experiment. Retain failed candidates and earlier
alternatives in explicit local artifact directories; do not commit experiment
records or backlog. Use `cargo xtask search` for the bounded algorithm suite and
`cargo xtask candidate` for repository capture and isolated verification.

Evaluate compilation/interface conformance, independent correctness/proofs,
development workloads, then held-out workloads after selection. Incorrect or
incomplete candidates cannot enter performance ranking. A changed contract
creates a new evaluation revision. Candidate changes cannot modify the frozen
evaluator or weaken acceptance. Compare per-workload latency, throughput, memory
and setup costs without an arbitrary composite score. Report token/evaluation
costs and unavailable usage metrics. Choose a search strategy from measured
quality/cost evidence; do not assume retained alternatives improve this project.

Use a continuous session by default, with explicit structural alternatives for
consequential decisions. Escalate to fresh contexts when early assumptions need
an independent challenge or measured search warrants the additional cost. Retain
candidate evidence, but do not add branch-revisiting search as a default step.
