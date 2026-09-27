#!/usr/bin/env bun
// Independent black-box acceptance probes for tools/docs/check.mjs.
// Run: bun tools/docs/check.test.mjs [optional-path-to-candidate-checker]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const ownDir = path.dirname(fileURLToPath(import.meta.url));
const candidate = path.resolve(process.argv[2] ?? path.join(ownDir, 'check.mjs'));
if (!fs.existsSync(candidate) || !fs.statSync(candidate).isFile()) {
  console.error(`Candidate absent: ${candidate}`);
  process.exit(2);
}

function put(root, rel, content) {
  const dest = path.join(root, rel);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, content);
}

function fixture(root) {
  put(root, 'README.md', '# Project\n\n[Architecture](docs/README.md)\n');
  put(root, 'docs/README.md', '# Architecture\n\n[System](architecture/system.md)\n');
  put(root, 'docs/architecture/system.md', '# System\n\n[Diagram](../diagrams/system.mmd)\n');
  put(root, 'docs/diagrams/system.mmd', 'flowchart LR\n    A --> B\n');
  put(root, 'docs/decisions/ADR-0001-first.md', '# First decision\n');
  put(root, '.vscode/extensions.json', '{"recommendations":[]}\n');
  put(root, '.codex/hooks.json', '{}\n');
  put(root, 'docs/architecture/graph.json', '{"nodes":[],"edges":[]}\n');
}

function snapshot(base) {
  const entries = [];
  function visit(dir) {
    for (const item of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const absolute = path.join(dir, item.name);
      const relative = path.relative(base, absolute);
      if (item.isSymbolicLink()) entries.push([relative, 'link', fs.readlinkSync(absolute)]);
      else if (item.isDirectory()) { entries.push([relative, 'dir']); visit(absolute); }
      else if (item.isFile()) entries.push([relative, 'file', fs.readFileSync(absolute).toString('base64')]);
      else entries.push([relative, 'other']);
    }
  }
  visit(base);
  return JSON.stringify(entries);
}

const cases = [
  {
    name: 'absolute file URI is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](file:///etc/passwd)\n'); },
  },
  {
    name: 'relative file URI is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](file:../../outside.md)\n'); },
  },
  {
    name: 'Windows drive link is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](C:/outside.md)\n'); },
  },
  {
    name: 'Windows drive link with encoded slash is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](C:%2Foutside.md)\n'); },
  },
  {
    name: 'Windows drive link with encoded backslash is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](C:%5Coutside.md)\n'); },
  },
  {
    name: 'Windows drive link with encoded colon is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Outside](C%3A%2Foutside.md)\n'); },
  },
  {
    name: 'baseline repository documents', valid: true,
    change() {},
  },
  {
    name: 'external, mailto, and protocol-relative URLs require no network', valid: true,
    change({ root }) {
      put(root, 'docs/README.md', '# Architecture\n\n[Web](https://example.invalid/page) [Mail](mailto:a@example.invalid) [CDN](//example.invalid/a.png)\n');
    },
  },
  {
    name: 'broken relative Markdown link', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Missing](missing.md)\n'); },
  },
  {
    name: 'missing heading fragment', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Missing](architecture/system.md#absent)\n'); },
  },
  {
    name: 'inline image destination is checked', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n![Missing image](images/missing.png)\n'); },
  },
  {
    name: 'reference-style link destination is checked', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Missing guide][guide]\n\n[guide]: missing.md\n'); },
  },
  {
    name: 'reference-style image destination is checked', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n![Missing image][image]\n\n[image]: missing.png\n'); },
  },
  {
    name: 'valid inline link in a GFM table cell', valid: true,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n| View | Document |\n| --- | --- |\n| System | [Architecture](architecture/system.md) |\n'); },
  },
  {
    name: 'missing inline link in a GFM table cell', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n| View | Document |\n| --- | --- |\n| System | [Architecture](architecture/missing.md) |\n'); },
  },
  {
    name: 'valid image and reference-style destinations', valid: true,
    change({ root }) {
      put(root, 'docs/README.md', '# Architecture\n\n![Image](images/icon.svg) [Guide][guide] ![Image][image]\n\n[guide]: architecture/system.md\n[image]: images/icon.svg\n');
      put(root, 'docs/images/icon.svg', '<svg xmlns="http://www.w3.org/2000/svg"/>\n');
    },
  },
  {
    name: 'percent-decoded local path, query, and heading fragment', valid: true,
    change({ root }) {
      put(root, 'docs/README.md', '# Architecture\n\n[Guide](guide%20one.md?raw=1#some-heading)\n');
      put(root, 'docs/guide one.md', '# Some Heading\n');
    },
  },
  {
    name: 'second duplicate GitHub heading slug', valid: true,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n## Repeat\n\n## Repeat\n\n[Second](#repeat-1)\n'); },
  },
  {
    name: 'nonexistent third duplicate heading slug', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n## Repeat\n\n## Repeat\n\n[Third](#repeat-2)\n'); },
  },
  {
    name: 'root-absolute local link is rejected', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n[Root](/docs/architecture/system.md)\n'); },
  },
  {
    name: 'inline code and ordinary fenced code do not contain links', valid: true,
    change({ root }) {
      put(root, 'docs/README.md', '# Architecture\n\n`[Fake](absent.md)`\n\n```text\n[Also fake](absent.md)\n```\n');
    },
  },
  {
    name: 'quadruple example fence hides nested Mermaid and links', valid: true,
    change({ root }) {
      put(root, 'docs/README.md', '# Architecture\n\n````markdown\n```mermaid\nflowchart LR\n  A -->\n```\n[Example](absent.md)\n````\n');
    },
  },
  {
    name: 'valid unmarked Mermaid flowchart fence', valid: true,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n```mermaid\nflowchart LR\n    A --> B\n```\n'); },
  },
  {
    name: 'valid standalone Mermaid state diagram', valid: true,
    change({ root }) { put(root, 'docs/diagrams/state-machines.mmd', 'stateDiagram-v2\n    [*] --> Pending\n    Pending --> Done\n'); },
  },
  {
    name: 'malformed standalone Mermaid flowchart', valid: false,
    change({ root }) { put(root, 'docs/diagrams/system.mmd', 'flowchart LR\n    A -->\n'); },
  },
  {
    name: 'malformed real Mermaid fence', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n```mermaid\nflowchart LR\n    A -->\n```\n'); },
  },
  {
    name: 'malformed Mermaid fence in a blockquote', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n> ```mermaid\n> flowchart LR\n>     A -->\n> ```\n'); },
  },
  {
    name: 'canonical Mermaid marker with blank line and normalized endings', valid: true,
    change({ root }) {
      put(root, 'docs/diagrams/system.mmd', 'flowchart LR\r\n    A --> B\r\n');
      put(root, 'docs/README.md', '# Architecture\n\n<!-- diagram: diagrams/system.mmd -->\n\n```mermaid\nflowchart LR\n    A --> B\n```\n');
    },
  },
  {
    name: 'root README canonical marker and inline-code marker literal', valid: true,
    change({ root }) {
      put(root, 'README.md', '# Project\n\n<!-- diagram: docs/diagrams/system.mmd -->\n\n```mermaid\nflowchart LR\n    A --> B\n```\n\n`<!-- diagram: docs/diagrams/missing.mmd -->`\n\n```mermaid\nflowchart LR\n    B --> C\n```\n');
    },
  },
  {
    name: 'canonical Mermaid copy drift', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n<!-- diagram: diagrams/system.mmd -->\n```mermaid\nflowchart LR\n    A --> C\n```\n'); },
  },
  {
    name: 'canonical Mermaid marker points to missing file', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n<!-- diagram: diagrams/missing.mmd -->\n```mermaid\nflowchart LR\n    A --> B\n```\n'); },
  },
  {
    name: 'orphan canonical Mermaid marker', valid: false,
    change({ root }) { put(root, 'docs/README.md', '# Architecture\n\n<!-- diagram: diagrams/system.mmd -->\nIntervening prose.\n\n```mermaid\nflowchart LR\n    A --> B\n```\n'); },
  },
  {
    name: 'malformed VS Code extensions JSON', valid: false,
    change({ root }) { put(root, '.vscode/extensions.json', '{"recommendations": [}\n'); },
  },
  {
    name: 'malformed Codex hooks JSON', valid: false,
    change({ root }) { put(root, '.codex/hooks.json', '{broken\n'); },
  },
  {
    name: 'malformed optional architecture graph JSON', valid: false,
    change({ root }) { put(root, 'docs/architecture/graph.json', '{"nodes":,}\n'); },
  },
  {
    name: 'duplicate four-digit ADR number', valid: false,
    change({ root }) { put(root, 'docs/decisions/ADR-0001-second.md', '# Second decision\n'); },
  },
  {
    name: 'hidden agent skill Markdown is scanned', valid: false,
    change({ root }) { put(root, '.agents/skills/sample/SKILL.md', '# Skill\n\n[Missing](missing.md)\n'); },
  },
  {
    name: '.git, target, and node_modules trees are skipped', valid: true,
    change({ root }) {
      for (const dir of ['.git', 'target', 'node_modules']) put(root, `${dir}/bad.md`, '# Skipped\n\n[Missing](missing.md)\n');
    },
  },
  {
    name: 'symlinked directory outside root is not traversed', valid: true,
    change({ root, outside }) {
      put(outside, 'bad.md', '# Outside\n\n[Missing](missing.md)\n');
      fs.symlinkSync(outside, path.join(root, 'external-docs'), 'dir');
    },
  },
  {
    name: 'link through symlinked file outside root is rejected', valid: false,
    change({ root, outside }) {
      put(outside, 'guide.md', '# Outside guide\n');
      fs.symlinkSync(path.join(outside, 'guide.md'), path.join(root, 'docs', 'external-guide.md'), 'file');
      put(root, 'docs/README.md', '# Architecture\n\n[Outside](external-guide.md)\n');
    },
  },
];

let failures = 0;
for (const test of cases) {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), 'fabric-docs-oracle-'));
  const root = path.join(sandbox, 'root');
  const outside = path.join(sandbox, 'outside');
  fs.mkdirSync(root);
  fs.mkdirSync(outside);
  try {
    fixture(root);
    test.change({ root, outside });
    const before = snapshot(sandbox);
    const result = spawnSync('bun', [candidate, root], {
      cwd: root,
      encoding: 'utf8',
      timeout: 20000,
      maxBuffer: 4 * 1024 * 1024,
    });
    const unchanged = snapshot(sandbox) === before;
    const diagnostic = `${result.stdout ?? ''}${result.stderr ?? ''}`.trim();
    const actual = result.error ? `spawn error: ${result.error.message}` : result.signal ? `signal ${result.signal}` : `exit ${result.status}`;
    const exitMatches = !result.error && !result.signal && (test.valid ? result.status === 0 : result.status !== 0 && result.status !== null);
    const diagnosticMatches = test.valid || diagnostic.length > 0;
    const passed = exitMatches && diagnosticMatches && unchanged;
    if (!passed) failures++;
    console.log(`${passed ? 'PASS' : 'FAIL'} ${test.name}: expected ${test.valid ? 'exit 0' : 'nonzero exit with diagnostic'}, got ${actual}${!diagnosticMatches ? ', no diagnostic' : ''}${!unchanged ? ', fixture changed' : ''}`);
    if (!passed && diagnostic) console.log(`  output: ${diagnostic.slice(0, 500).replace(/\n/g, '\n  ')}`);
  } catch (error) {
    failures++;
    console.log(`FAIL ${test.name}: probe setup error: ${error?.message ?? error}`);
  } finally {
    fs.rmSync(sandbox, { recursive: true, force: true });
  }
}
console.log(`${cases.length - failures}/${cases.length} acceptance probes passed`);
process.exit(failures ? 1 : 0);
