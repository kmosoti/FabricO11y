# Working on Fabric O11y

Begin with [CURRENT.md](CURRENT.md), the affected [system view](architecture/system.md), and the source. [AGENTS.md](../AGENTS.md) is the standing project instruction file, including the full architecture documentation policy and learning preferences. Update the implementation and its affected documentation together; identify proposals and unverified assumptions explicitly.

## Skills and custom instructions

Project custom instructions live in `AGENTS.md`. The reusable workflows are repository-local skills:

| Invocation | When to use it |
| --- | --- |
| `$fabric-architecture` | Update or audit the relationship between implementation and documentation. |
| `$fabric-lesson` | Explain or build one requested learning increment. |
| `$fabric-experiment` | Plan or run a scoped performance comparison or formal investigation. |

The skill files are [fabric-architecture](../.agents/skills/fabric-architecture/SKILL.md), [fabric-lesson](../.agents/skills/fabric-lesson/SKILL.md), and [fabric-experiment](../.agents/skills/fabric-experiment/SKILL.md). They can be read as ordinary Markdown. Codex discovers `.agents/skills` within the repository; if a new skill does not appear, restart the session. See the [official skill documentation](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills).

## Documentation checks

The application needs Rust. Documentation maintenance additionally uses Bun 1.4.0 and Python 3; VS Code extensions are optional. From the repository root:

```sh
bun install --cwd tools/docs --frozen-lockfile
bun tools/docs/check.mjs
```

The [checker](../tools/docs/check.mjs) reads Markdown and Mermaid sources. It verifies relative link targets and Markdown heading fragments, Mermaid syntax, marked diagram copies, JSON syntax in the editor and hook configuration, optional architecture graph JSON, and duplicate ADR numbers. It includes hidden skill Markdown while excluding `.git`, `target`, and `node_modules`. It follows no directory symlinks and rejects local links escaping the repository.

The checker uses a Markdown parser and Mermaid's parser, with a local DOM for syntax parsing. It performs no rendering, network access, or file writes. It rejects `file:` URIs and absolute local paths. It does not verify external URLs, raw HTML links, Foam wikilinks, GitHub's renderer version, code line anchors, or whether architectural claims are true. Source inspection remains necessary. Its development dependencies live under [tools/docs](../tools/docs/package.json) and do not enter the Rust dependency graph.

When changing the checker or hooks, also run the corresponding probes:

```sh
bun tools/docs/check.test.mjs
python3 -B tools/docs/test_hooks.py
```

The checker probes inject broken links, malformed Mermaid, diagram drift, invalid JSON, and duplicate ADRs into temporary fixtures; valid controls must still pass. Hook probes exercise context loading, failure feedback, missing tools, and the continuation guard. [Documentation CI](../.github/workflows/docs.yml) runs the same commands.

Canonical diagrams live in `docs/diagrams/*.mmd`. A Markdown page can repeat a diagram for GitHub rendering; place a `<!-- diagram: relative/path.mmd -->` comment directly before its Mermaid fence. The checker requires that marked copy to match its canonical source after newline normalization. See [docs/README.md](README.md) for the working example. Policy examples in `AGENTS.md` are examples, not application components.

## Codex hooks

[.codex/hooks.json](../.codex/hooks.json) configures documentation handlers implemented by [.codex/hooks/run.py](../.codex/hooks/run.py):

- `SessionStart` supplies a bounded copy of `docs/CURRENT.md` as context.
- `Stop` runs the documentation checker. On a failure it requests one corrective continuation. If already continued, it reports remaining failures without requesting another continuation. Missing tools or a timeout are reported as skipped or incomplete checks, never as success.

Those documentation scripts only read and validate. They do not edit, stage, commit, install packages, invoke a model, or change global settings. Hook commands resolve the Git root so starting Codex in a repository subdirectory works.

Codex requires the project configuration and the exact hook definitions to be trusted before automatic execution. After inspecting these files, open `/hooks` in Codex CLI to review and trust them; changes to definitions may require another review. The setup's direct script probes do not grant runtime trust. This requirement comes from [Codex hook trust](https://learn.chatgpt.com/docs/hooks#review-and-trust-hooks). Without hook activation, run the documentation commands manually.

## Passive agent telemetry

The same [hook configuration](../.codex/hooks.json) contains separate telemetry handlers for session, turn, and tool observations. The [observer](../tools/telemetry/cli.py) writes validated events under gitignored `.local/codex-telemetry/` and returns no model-visible content. It does not change the existing context loader or documentation checks. See the [telemetry commands and contract](../tools/telemetry/README.md), including runtime trust and failure limits.

Validate this development tool with:

```sh
python3 -B tools/telemetry/test_contract.py
python3 -B tools/telemetry/test_cli.py
```

The probes exercise invalid records, concurrent appenders, reordered events, silent handled errors, and explicit replay failures. They do not establish live hook activation or unchanged model quality.

## Optional editor tools

[.vscode/extensions.json](../.vscode/extensions.json) recommends identifiers verified against their publisher listings:

- [Foam](https://marketplace.visualstudio.com/items?itemName=foam.foam-vscode) for local backlinks and discovery.
- [Mermaid Preview](https://marketplace.visualstudio.com/items?itemName=vstirbu.vscode-mermaid-preview) for diagrams.
- [Mermaid Visual Diff](https://marketplace.visualstudio.com/items?itemName=orhymed.mermaid-visual-diff) for diagram changes.
- [CodeTour](https://marketplace.visualstudio.com/items?itemName=vsls-contrib.codetour) for future execution-path tours.

No tour is justified by the current short demonstration. All primary navigation uses ordinary Markdown links, so the repository is readable directly on GitHub or in any text editor. Extensions, Foam wikilinks, and a documentation server are not required.
