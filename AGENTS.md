# FabricO11y development

The repository contains development tooling. The telemetry platform is planned.
Read README.md and docs/direction.md. Product work is queued in local backlog/.
Use the existing user-installed jj from PATH with its Git backend for snapshots, commits and bookmarks. Never publish
archive/* bookmarks. Bind verification to concrete commit IDs, not change IDs.
Use native Cargo and checker commands; no mandatory resource launcher or fixed
memory/swap/time ceiling applies. Choose and record settings for expensive work.
Name files, workflows and components by their responsibility, not a temporary
development phase. Prefer existing maintained tools over new frameworks. Skills guide procedures;
LogicGate coordinates supported evidence; actual tools establish results.
Report real commands, exits, assumptions and unavailable capabilities. Keep
specification changes separate from implementation. Never infer publication
permission from a successful check. Preserve secrets and user-owned data.

Use the existing user installation (currently ~/.cargo/bin/jj); do not require
a system package or change the global shell PATH for this repository.
