---
name: fabric-delegation
description: Brief for an implementation subagent that receives one task card (issue, owned paths, line budget, verification route). Covers the project's shape, the rules that cannot bend, line-count and consolidation discipline, and the report the orchestrator needs back.
---

# Implement one task card

Your task card names the issue, the paths you own, a net line budget and a verification route. Read [AGENTS.md](../../../AGENTS.md) first; where it is stricter than this brief, it wins. Then read the issue, the files you own and their tests.

## Project shape

| Layer | Crates | May depend on | Holds |
| --- | --- | --- | --- |
| core | `fabric-core` | core | Pure `no_std` decisions with no dependencies: delivery, control, query, retention and collection kernels. |
| ports | `fabric-ports` | core | Three effect traits: `DurableJournal`, `Clock`, `SegmentStore`. |
| app | `fabric-app` | core, ports | The `commit_group` and `apply_retention` use cases. |
| adapter support | `fabric-frame`, `fabric-observation` | core | The FAB1 frame log and batch envelope; the FOB1 codec. |
| adapter | `fabric-adapter-linux` | core, ports, adapter support | `/proc` metrics, log reads, `openat2`. |
| composition root | `fabric_o11y` (`src/`), `fabric-server`, `fabric-ui` | everything above, never another root | Spindle runtime and Spool; HTTP, access, console, query, journal, Segments and sealer, which still hold adapter code; the Leptos PWA. |
| verification | `fabric-properties`, `fabric-sim`, `fabric-fuzz-targets` | anything | Property, simulation and fuzz checks. |
| tooling | `xtask` | nothing | Layer, purity, mutation and check-registry gates. |

Where things live:
- **Server tests:** `*_tests.rs` files beside their modules, and `crates/*/tests/`.
- **Policy files:** `docs/architecture/layers.json` and `core-purity.json`, `xtask/checks.json` and `mutants.json`. The independent oracles are `tools/qualification/*oracle*.py`.
- **Claims and current state:** claims belong in `docs/formal/verification-matrix.md`; present state belongs in `docs/CURRENT.md`.
- **Historical evidence:** `docs/experiments/` holds experiment evidence. Never edit a historical record.

## Rules that cannot bend

1. **Stay inside your owned paths.** If you need another file, stop and say so in your report.
2. **Never weaken a check, oracle, negative control, invariant or policy file to make code pass.** A change to a policy file goes in its own commit whose body starts with `Trust-boundary update:` and gives the reason.
3. **Put domain decisions in `fabric-core`, and keep effects at the edge.** Add no crate, trait, service locator, plugin system, event bus or dependency-injection layer unless your card asks for one.
4. **Every defect fix gets a deterministic regression test that fails before the fix.** Say how you saw it fail.
5. **Never claim a check passed unless you ran it.** Report the exact command and its exit status. "Not run" is an acceptable answer.
6. **Commit only on your own branch.** Do not push, open pull requests, rewrite history or force-push. The orchestrator integrates your work.
7. **Do not name anything after release maturity, pull requests, models or sessions** ([ADR-0019](../../../docs/decisions/ADR-0019-keep-release-maturity-in-tags.md)).
8. **Write commit subjects as one imperative sentence,** with a body explaining why, matching `git log`.

## Line count and consolidation

- **Keep within the card's budget.** The budget is net production lines (added minus removed, excluding tests and docs). Under budget is good; a net reduction is better.
- **Search before writing anything new** (`rg`). Reuse an existing helper before adding one. A new helper must replace at least two existing copies.
- **Consolidate duplicates you touch.** Move the logic into the innermost layer that can own it, and delete the copies. Use `fabric-core` constants instead of repeating literals.
- **Add nothing speculative:** no options, config keys, wrappers or comments that restate the code. A configuration key without a gate consumer is a defect. Match the surrounding comment density.
- **Keep tests small.** Write the smallest deterministic case, and extend an existing table or fixture before adding a file.
- **Measure your diff before reporting.** Inline `#[cfg(test)]` modules count as production in this summary, so state their size separately.

  ```sh
  git diff --numstat "$BASE"...HEAD | awk '
    { f = $3
      if (f ~ /^(vendor|packaging\/licenses)\// || f ~ /\.lock$/) c = "vendored"
      else if (f ~ /^docs\/experiments\//) c = "evidence"
      else if (f ~ /(_tests?\.rs$|\/tests\/|\/test_[^\/]*$)/) c = "tests"
      else if (f ~ /\.md$/ || f ~ /^docs\//) c = "docs"
      else c = "production"
      a[c] += $1; d[c] += $2 }
    END { for (c in a) printf "%-10s +%d -%d net %+d\n", c, a[c], d[c], a[c] - d[c] }'
  ```

## Verification routes

The card names your route:
- **CI only.** Do not run cargo, bun or project Python. The orchestrator triggers the repository workflows on the integration branch, and their receipts are the evidence.
- **Container diagnostics.** Use this only where the owner has allowed it for ephemeral cloud containers. Run `CARGO_BUILD_JOBS=2 timeout 1200 nice cargo <check|test|clippy> -p <crate> --locked` with the shared `CARGO_TARGET_DIR` the card gives. Results are diagnostics for iteration, not release evidence. Say so in your report.

On the owner's workstation, every command goes through `python3 -B tools/resource_group.py -- COMMAND`, as AGENTS.md requires.

## Report

End with at most about 300 words:
- the card ID, your branch and your commits (short SHA and subject);
- the line summary from the command above, whether it is within budget, and why if not;
- what you consolidated or deleted;
- each check, with its exact command and exit status, or "not run" with the reason;
- open questions, risks, and any files outside your ownership you needed.
