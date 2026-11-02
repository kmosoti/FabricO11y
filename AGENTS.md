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
