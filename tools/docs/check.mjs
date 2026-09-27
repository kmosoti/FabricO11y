#!/usr/bin/env bun

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { marked } from "marked";
import GithubSlugger from "github-slugger";
import { JSDOM } from "jsdom";

// Mermaid's parser sanitizes diagram configuration through DOMPurify. Supplying
// an isolated in-memory DOM lets that real parser work in a headless CLI; no
// browser, network, or rendered page is involved.
const dom = new JSDOM("");
globalThis.window = dom.window;
globalThis.document = dom.window.document;
const { default: mermaid } = await import("mermaid");

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const rootArgument = args[0] ?? path.resolve(scriptDir, "../..");
const errors = [];

function displayPath(file) {
  const rel = path.relative(root, file);
  return rel === "" ? "." : rel.split(path.sep).join("/");
}

function report(file, reason) {
  errors.push(`${displayPath(file)}: ${reason}`);
}

function insideRoot(file) {
  const rel = path.relative(root, file);
  return rel === "" || (!rel.startsWith(`..${path.sep}`) && rel !== ".." && !path.isAbsolute(rel));
}

let root;
try {
  root = fs.realpathSync(path.resolve(rootArgument));
  if (!fs.statSync(root).isDirectory()) throw new Error("not a directory");
} catch (error) {
  console.error(`${rootArgument}: cannot read repository root (${error.message})`);
  process.exit(1);
}

function collectFiles(dir, files = []) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch (error) {
    report(dir, `cannot read directory (${error.message})`);
    return files;
  }

  for (const entry of entries) {
    if ([".git", "target", "node_modules"].includes(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      collectFiles(full, files);
      continue;
    }
    if (entry.isSymbolicLink()) {
      let stat;
      let real;
      try {
        stat = fs.statSync(full);
        real = fs.realpathSync(full);
      } catch {
        continue;
      }
      if (stat.isDirectory()) continue;
      if (!insideRoot(real) || !stat.isFile()) continue;
    } else if (!entry.isFile()) {
      continue;
    }

    const ext = path.extname(entry.name).toLowerCase();
    if (ext === ".md" || ext === ".mmd") files.push(full);
  }
  return files;
}

function decodeBasicEntities(value) {
  return value.replace(/&(#(?:x[\da-f]+|\d+)|amp|lt|gt|quot|apos|nbsp);/gi, (match, entity) => {
    const lower = entity.toLowerCase();
    if (lower === "amp") return "&";
    if (lower === "lt") return "<";
    if (lower === "gt") return ">";
    if (lower === "quot") return '"';
    if (lower === "apos") return "'";
    if (lower === "nbsp") return "\u00a0";
    const numeric = lower.startsWith("#x")
      ? Number.parseInt(lower.slice(2), 16)
      : Number.parseInt(lower.slice(1), 10);
    try {
      return Number.isFinite(numeric) && numeric > 0 && numeric <= 0x10ffff
        ? String.fromCodePoint(numeric)
        : match;
    } catch {
      return match;
    }
  });
}

function inlineText(tokens) {
  if (!Array.isArray(tokens)) return "";
  let out = "";
  for (const token of tokens) {
    if (!token || typeof token !== "object") continue;
    switch (token.type) {
      case "html":
        // HTML tags do not contribute to GitHub's heading text; text between
        // tags is represented by separate Marked text tokens.
        break;
      case "br":
        out += " ";
        break;
      case "image":
      case "link":
      case "strong":
      case "em":
      case "del":
        out += inlineText(token.tokens);
        break;
      case "codespan":
      case "text":
      case "escape":
        out += token.text ?? "";
        break;
      default:
        if (token.tokens) out += inlineText(token.tokens);
        else if (token.text) out += token.text;
    }
  }
  return decodeBasicEntities(out);
}

function descendants(tokens, callback, seen = new Set()) {
  if (!tokens) return;
  if (Array.isArray(tokens)) {
    for (const item of tokens) descendants(item, callback, seen);
    return;
  }
  if (typeof tokens !== "object" || seen.has(tokens)) return;
  seen.add(tokens);
  if (typeof tokens.type === "string") callback(tokens);
  for (const value of Object.values(tokens)) {
    if (value && typeof value === "object") descendants(value, callback, seen);
  }
}

function normalizeDiagram(source) {
  return source.replace(/\r\n?/g, "\n").trim();
}

function stripBlockquotePrefix(line) {
  let rest = line;
  while (true) {
    const match = rest.match(/^ {0,3}> ?/);
    if (!match) return rest;
    rest = rest.slice(match[0].length);
  }
}

// Marked supplies the parsed fences. This small source pass records their line
// positions and marker comments, while correctly treating a longer outer
// fence as containing any shorter example fences inside it.
function scanFencePositions(source) {
  const lines = source.split("\n");
  const fences = [];
  const markers = [];
  let active = null;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index].replace(/\r$/, "");
    const content = stripBlockquotePrefix(line);
    if (active) {
      const close = content.match(/^ {0,3}(`+|~+)[ \t]*$/);
      if (close && close[1][0] === active.char && close[1].length >= active.length) active = null;
      continue;
    }

    const marker = content.match(/^\s*<!--\s*diagram:\s*(.*?)\s*-->\s*$/);
    if (marker) markers.push({ line: index, target: marker[1] });

    const open = content.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (!open) continue;
    const fenceText = open[1];
    const info = open[2].trim();
    const language = info.split(/[ \t]+/, 1)[0] ?? "";
    fences.push({ line: index, language });
    active = { char: fenceText[0], length: fenceText.length };
  }
  return { fences, markers, lines };
}

function localPathForLink(markdownFile, href) {
  const raw = href.trim();
  if (/^(?:https?:|mailto:)/i.test(raw) || raw.startsWith("//")) return null;

  const hashAt = raw.indexOf("#");
  const beforeFragment = hashAt < 0 ? raw : raw.slice(0, hashAt);
  const encodedFragment = hashAt < 0 ? "" : raw.slice(hashAt + 1);
  const queryAt = beforeFragment.indexOf("?");
  const encodedPath = queryAt < 0 ? beforeFragment : beforeFragment.slice(0, queryAt);

  let decodedPath;
  let fragment;
  try {
    decodedPath = decodeURIComponent(encodedPath);
    fragment = decodeURIComponent(encodedFragment);
  } catch {
    report(markdownFile, `invalid percent encoding in link ${JSON.stringify(href)}`);
    return null;
  }
  // Classify decoded paths so encoded separators cannot hide a drive path.
  if (/^file:/i.test(decodedPath) || decodedPath.startsWith("/") || path.win32.isAbsolute(decodedPath)) {
    report(markdownFile, `local link must use a repository-relative path: ${JSON.stringify(href)}`);
    return null;
  }
  // Other URI schemes are external/non-file destinations.
  if (/^[a-z][a-z\d+.-]*:/i.test(raw)) return null;

  const target = path.resolve(path.dirname(markdownFile), decodedPath || path.basename(markdownFile));
  return { target, fragment, hasFragment: hashAt >= 0 };
}

async function checkMarkdown(file, source, filesByRealPath) {
  let tokens;
  try {
    tokens = marked.lexer(source);
  } catch (error) {
    report(file, `Markdown parse failed (${error.message})`);
    return;
  }

  const headings = [];
  const links = [];
  const mermaidTokens = [];
  descendants(tokens, (token) => {
    if (token.type === "heading") headings.push(token);
    if ((token.type === "link" || token.type === "image") && typeof token.href === "string") links.push(token);
    if (token.type === "code" && /^mermaid(?:\s|$)/i.test((token.lang ?? "").trim())) mermaidTokens.push(token);
  });

  const slugger = new GithubSlugger();
  const anchors = new Set();
  for (const heading of headings) anchors.add(slugger.slug(inlineText(heading.tokens)));

  for (const link of links) {
    const local = localPathForLink(file, link.href);
    if (!local) continue;
    const { target, fragment, hasFragment } = local;
    let realTarget;
    try {
      realTarget = fs.realpathSync(target);
    } catch {
      report(file, `local link target does not exist: ${JSON.stringify(link.href)}`);
      continue;
    }
    if (!insideRoot(realTarget)) {
      report(file, `local link resolves outside the repository: ${JSON.stringify(link.href)}`);
      continue;
    }

    let stat;
    try {
      stat = fs.statSync(realTarget);
    } catch {
      report(file, `local link target cannot be read: ${JSON.stringify(link.href)}`);
      continue;
    }
    if (!stat.isFile() && !stat.isDirectory()) {
      report(file, `local link target is not a file or directory: ${JSON.stringify(link.href)}`);
      continue;
    }
    if (!hasFragment || !fragment || !stat.isFile() || path.extname(realTarget).toLowerCase() !== ".md") continue;

    // GitHub's source-code line anchors are generated by the hosting UI and do
    // not correspond to Markdown headings.
    if (/^L\d+(?:-L\d+)?$/i.test(fragment) && !filesByRealPath.has(realTarget)) continue;

    const targetSource = readText(realTarget, file, `linked Markdown file ${JSON.stringify(link.href)}`);
    if (targetSource === null) continue;
    let targetTokens;
    try {
      targetTokens = marked.lexer(targetSource);
    } catch (error) {
      report(realTarget, `Markdown parse failed (${error.message})`);
      continue;
    }
    const targetSlugger = new GithubSlugger();
    const targetAnchors = new Set();
    descendants(targetTokens, (token) => {
      if (token.type === "heading") targetAnchors.add(targetSlugger.slug(inlineText(token.tokens)));
    });
    if (!targetAnchors.has(fragment)) {
      report(file, `heading fragment #${fragment} does not exist in ${displayPath(realTarget)}`);
    }
  }

  const scanned = scanFencePositions(source);
  const diagramFences = scanned.fences.filter((fence) => /^mermaid$/i.test(fence.language));
  if (diagramFences.length !== mermaidTokens.length) {
    report(file, `could not align parsed Mermaid fences with source (${mermaidTokens.length} parsed, ${diagramFences.length} found)`);
  }

  const associatedMarkers = new Set();
  const associatedFences = new Map();
  for (let i = 0; i < diagramFences.length; i += 1) {
    const fence = diagramFences[i];
    let previous = fence.line - 1;
    while (previous >= 0 && scanned.lines[previous].trim() === "") previous -= 1;
    const markerIndex = scanned.markers.findIndex((marker) => marker.line === previous);
    if (markerIndex >= 0) {
      associatedMarkers.add(markerIndex);
      associatedFences.set(i, markerIndex);
    }

    const token = mermaidTokens[i];
    if (token) {
      try {
        await mermaid.parse(token.text);
      } catch (error) {
        report(file, `invalid Mermaid fence (${error.message ?? error})`);
      }
    }
  }

  for (let i = 0; i < scanned.markers.length; i += 1) {
    const marker = scanned.markers[i];
    if (!associatedMarkers.has(i)) {
      report(file, `orphan diagram marker for ${JSON.stringify(marker.target)}`);
      continue;
    }
    const fenceIndex = [...associatedFences.entries()].find(([, markerIndex]) => markerIndex === i)?.[0];
    const token = fenceIndex === undefined ? null : mermaidTokens[fenceIndex];
    if (!token) continue;

    if (!marker.target || path.isAbsolute(marker.target)) {
      report(file, `diagram marker must name a relative .mmd file: ${JSON.stringify(marker.target)}`);
      continue;
    }
    const candidate = path.resolve(path.dirname(file), marker.target);
    let canonical;
    try {
      canonical = fs.realpathSync(candidate);
    } catch {
      report(file, `canonical Mermaid file does not exist: ${JSON.stringify(marker.target)}`);
      continue;
    }
    if (!insideRoot(canonical)) {
      report(file, `canonical Mermaid file resolves outside the repository: ${JSON.stringify(marker.target)}`);
      continue;
    }
    if (path.extname(canonical).toLowerCase() !== ".mmd") {
      report(file, `diagram marker must name a .mmd file: ${JSON.stringify(marker.target)}`);
      continue;
    }
    let canonicalText;
    try {
      canonicalText = fs.readFileSync(canonical, "utf8");
    } catch (error) {
      report(file, `cannot read canonical Mermaid file ${JSON.stringify(marker.target)} (${error.message})`);
      continue;
    }
    if (normalizeDiagram(canonicalText) !== normalizeDiagram(token.text)) {
      report(file, `Mermaid fence drifts from canonical file ${displayPath(canonical)}`);
    }
  }
}

function readText(file, diagnosticFile = file, description = "file") {
  try {
    return fs.readFileSync(file, "utf8");
  } catch (error) {
    report(diagnosticFile, `cannot read ${description} (${error.message})`);
    return null;
  }
}

async function main() {
  const files = collectFiles(root).sort((a, b) => a.localeCompare(b));
  const filesByRealPath = new Set();
  for (const file of files) {
    try {
      filesByRealPath.add(fs.realpathSync(file));
    } catch {
      // A disappearing file is diagnosed when it is read below.
    }
  }

  const markdownFiles = files.filter((file) => path.extname(file).toLowerCase() === ".md");
  for (const file of markdownFiles) {
    const source = readText(file);
    if (source !== null) await checkMarkdown(file, source, filesByRealPath);
  }

  for (const file of files.filter((item) => path.extname(item).toLowerCase() === ".mmd")) {
    const source = readText(file);
    if (source === null) continue;
    try {
      await mermaid.parse(source);
    } catch (error) {
      report(file, `invalid Mermaid diagram (${error.message ?? error})`);
    }
  }

  const jsonFiles = [
    ".vscode/extensions.json",
    ".codex/hooks.json",
    "docs/architecture/graph.json",
  ];
  for (const relative of jsonFiles) {
    const file = path.join(root, relative);
    if (!fs.existsSync(file)) continue;
    const source = readText(file);
    if (source === null) continue;
    try {
      JSON.parse(source);
    } catch (error) {
      report(file, `invalid JSON (${error.message})`);
    }
  }

  const adrDir = path.join(root, "docs", "decisions");
  if (fs.existsSync(adrDir) && fs.statSync(adrDir).isDirectory()) {
    const numbers = new Map();
    for (const file of collectFiles(adrDir).filter((item) => /^ADR-\d{4}-.+\.md$/.test(path.basename(item)))) {
      const number = path.basename(file).match(/^ADR-(\d{4})-/)[1];
      const prior = numbers.get(number);
      if (prior) report(file, `duplicate ADR number ${number} (also used by ${displayPath(prior)})`);
      else numbers.set(number, file);
    }
  }

  if (errors.length) {
    for (const error of errors) console.error(error);
    console.error(`Documentation checks found ${errors.length} issue${errors.length === 1 ? "" : "s"}.`);
    process.exitCode = 1;
  } else {
    console.log("Documentation checks passed.");
    process.exitCode = 0;
  }
}

await main();
dom.window.close();
