---
name: jj-workflow
description: Capture and verify FabricO11y changes using Jujutsu with a Git backend; use for candidate preparation and recovery.
---
Inspect jj status and conflicts. Snapshot explicitly before reading the full
commit ID. Keep archive/* bookmarks local. Capture before verification; use the
captured parent when materializing, since workspace add creates a new working
commit. Rewrites invalidate receipts. Use native jj interfaces and the repository
candidate wrapper; do not invent a second revision framework. Publish only the
explicitly authorized bookmark, never all bookmarks by default.
