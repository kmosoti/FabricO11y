use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use toml::Value;

const INVENTORY: &str = "xtask/documentation-inventory.toml";
const RECORDS: &str = "knowledge/records.toml";
const MAX_MARKDOWN_BYTES: u64 = 1024 * 1024;
const MAX_MARKDOWN_FILES: usize = 256;
const MAX_MARKDOWN_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 512 * 1024;

#[derive(Deserialize)]
struct Inventory {
    schema_version: u32,
    files: Vec<InventoryFile>,
}

#[derive(Deserialize)]
struct InventoryFile {
    path: String,
    classification: String,
    presentation_metadata: String,
    machine_required_metadata: bool,
    fenced_examples: bool,
    license_notice: bool,
    semantic_directive: bool,
    historical_evidence: bool,
}

#[derive(Deserialize)]
struct Records {
    schema_version: u32,
    migration: Migration,
    records: Vec<Value>,
}

#[derive(Deserialize)]
struct Migration {
    source_commit: String,
    source_format: String,
    record_count: usize,
}

pub fn check(root: &Path) -> Result<(), String> {
    let markdown_files = check_inventory(root)?;
    let (records, baseline_verified) = check_records(root)?;
    check_markdown(root)
        .map(|()| {
            println!(
                "documentation check passed: {markdown_files} Markdown files, {records} knowledge records, metadata history {}",
                if baseline_verified { "verified" } else { "not present in this checkout" }
            );
        })
}

fn toml_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let size = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > MAX_METADATA_BYTES {
        return Err(format!("{} exceeds 512 KiB limit", path.display()));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn tracked_markdown(root: &Path) -> Result<BTreeSet<String>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*.md",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            std::str::from_utf8(p)
                .map(str::to_owned)
                .map_err(|e| format!("tracked Markdown path is not UTF-8: {e}"))
        })
        .collect()
}

fn check_inventory(root: &Path) -> Result<usize, String> {
    let inventory: Inventory = toml_file(&root.join(INVENTORY))?;
    if inventory.schema_version != 1 {
        return Err("documentation inventory schema_version must be 1".into());
    }
    let actual = tracked_markdown(root)?;
    if actual.len() > MAX_MARKDOWN_FILES {
        return Err(format!(
            "documentation inventory exceeds {MAX_MARKDOWN_FILES} Markdown files"
        ));
    }
    let mut declared = BTreeSet::new();
    for file in inventory.files {
        let path = Path::new(&file.path);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "invalid documentation inventory path: {}",
                file.path
            ));
        }
        if !matches!(
            file.classification.as_str(),
            "operational-directives"
                | "knowledge-record"
                | "knowledge-index-and-guidance"
                | "published-release-artifact"
                | "proof-or-verification-evidence"
                | "maintained-prose"
        ) {
            return Err(format!("unknown documentation class for {}", file.path));
        }
        let is_knowledge_record = file.classification == "knowledge-record";
        if file.machine_required_metadata != is_knowledge_record
            || file.presentation_metadata
                != if is_knowledge_record {
                    "relocated-to-knowledge/records.toml"
                } else {
                    "none-found"
                }
        {
            return Err(format!(
                "inconsistent metadata classification for {}",
                file.path
            ));
        }
        if !declared.insert(file.path.clone()) {
            return Err(format!("duplicate inventory path: {}", file.path));
        }
    }
    if declared != actual {
        let missing: Vec<_> = actual.difference(&declared).cloned().collect();
        let stale: Vec<_> = declared.difference(&actual).cloned().collect();
        return Err(format!(
            "documentation inventory mismatch; unlisted={missing:?}, no-longer-tracked={stale:?}"
        ));
    }
    Ok(actual.len())
}

fn check_records(root: &Path) -> Result<(usize, bool), String> {
    let records: Records = toml_file(&root.join(RECORDS))?;
    if records.schema_version != 1
        || records.migration.source_format != "YAML front matter"
        || records.migration.source_commit.len() != 40
        || !records
            .migration
            .source_commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || records.migration.record_count != records.records.len()
        || records.records.len() > 128
    {
        return Err("invalid knowledge record migration declaration".into());
    }

    let mut by_id = BTreeMap::<String, &Value>::new();
    let mut source_paths = BTreeSet::new();
    for record in &records.records {
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .ok_or("knowledge record missing string id")?;
        let source = record
            .get("source_path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{id} missing source_path"))?;
        if !source.starts_with("knowledge/") || !source.ends_with(".md") {
            return Err(format!("invalid knowledge source path for {id}: {source}"));
        }
        if !source_paths.insert(source.to_owned()) {
            return Err(format!("duplicate knowledge source path: {source}"));
        }
        if !root.join(source).is_file() {
            return Err(format!("missing Markdown body for {id}: {source}"));
        }
        if by_id.insert(id.to_owned(), record).is_some() {
            return Err(format!("duplicate knowledge ID: {id}"));
        }
        if record.get("added_by").and_then(Value::as_str).is_none()
            || record
                .get("human_ratified")
                .and_then(Value::as_bool)
                .is_none()
        {
            return Err(format!("{id} missing provenance or ratification metadata"));
        }
        if let Some(ratified) = record.get("human_ratified").and_then(Value::as_bool) {
            let fields = record.as_table().unwrap();
            if ratified != fields.contains_key("ratified_by")
                || ratified != fields.contains_key("ratified_on")
            {
                return Err(format!("{id} has inconsistent ratification details"));
            }
        }
    }

    for (id, record) in &by_id {
        for relation in ["related", "supersedes", "superseded_by"] {
            let Some(values) = record.get(relation) else {
                continue;
            };
            let targets: Vec<&str> = match values {
                Value::String(target) => vec![target],
                Value::Array(items) => items
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| format!("{id}.{relation} contains a non-string"))
                    })
                    .collect::<Result<_, _>>()?,
                _ => return Err(format!("{id}.{relation} must be a string or string array")),
            };
            for target in targets {
                if !by_id.contains_key(target) {
                    return Err(format!("{id}.{relation} refers to missing {target}"));
                }
                if id == target {
                    return Err(format!("{id}.{relation} refers to itself"));
                }
            }
        }
    }

    // Compare the migrated metadata against its immutable Git history baseline
    // when it is available. Shallow CI checkouts may not contain this older
    // object, so they still enforce the versioned schema and graph invariants.
    let baseline = format!("{}^{{commit}}", records.migration.source_commit);
    let baseline_available = Command::new("git")
        .args(["cat-file", "-e", &baseline])
        .current_dir(root)
        .status()
        .map_err(|e| format!("git cat-file: {e}"))?
        .success();
    if !baseline_available {
        eprintln!(
            "documentation: {} absent from this checkout; metadata history comparison skipped",
            records.migration.source_commit
        );
    }
    for (id, record) in by_id.iter().filter(|_| baseline_available) {
        let source = record["source_path"].as_str().unwrap();
        let spec = format!("{}:{source}", records.migration.source_commit);
        let output = Command::new("git")
            .args(["show", &spec])
            .current_dir(root)
            .output()
            .map_err(|e| format!("git show {spec}: {e}"))?;
        if !output.status.success() {
            return Err(format!("cannot read knowledge baseline {spec}"));
        }
        if output.stdout.len() as u64 > MAX_MARKDOWN_BYTES {
            return Err(format!("historical Markdown exceeds 1 MiB: {source}"));
        }
        let old = String::from_utf8(output.stdout).map_err(|e| format!("{spec}: {e}"))?;
        let header = old
            .strip_prefix("---\n")
            .and_then(|tail| tail.find("\n---\n").map(|end| &tail[..end]))
            .ok_or_else(|| format!("baseline has no YAML header: {source}"))?;
        let old_metadata = parse_legacy_header(header, source)?;
        if let Some(old_id) = old_metadata.get("id").and_then(Value::as_str) {
            if old_id != id {
                return Err(format!("stable ID changed for {source}: {old_id} -> {id}"));
            }
        } else if !id_matches_source(id, source) {
            return Err(format!("stable identity does not match {source}"));
        }
        let current_metadata: BTreeMap<_, _> = record
            .as_table()
            .unwrap()
            .iter()
            .filter(|(key, _)| key != &"id" && key != &"source_path")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let old_metadata: BTreeMap<_, _> = old_metadata
            .into_iter()
            .filter(|(key, _)| key != "id")
            .collect();
        if old_metadata != current_metadata {
            return Err(format!("migrated metadata differs from history for {id}"));
        }
    }
    Ok((by_id.len(), baseline_available))
}

fn id_matches_source(id: &str, source: &str) -> bool {
    Path::new(source)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|stem| stem.starts_with(id))
}

fn parse_legacy_header(header: &str, source: &str) -> Result<BTreeMap<String, Value>, String> {
    let mut fields = BTreeMap::new();
    let mut pending_array: Option<String> = None;
    for line in header.lines() {
        if let Some(item) = line.strip_prefix("  - ") {
            let key = pending_array
                .as_ref()
                .ok_or_else(|| format!("unattached YAML array item in {source}"))?;
            fields
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .ok_or_else(|| format!("invalid YAML array in {source}"))?
                .push(Value::String(item.to_owned()));
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| format!("unsupported YAML metadata in {source}: {line}"))?;
        let value = value.trim_start();
        pending_array = None;
        let toml_value = if value.is_empty() {
            pending_array = Some(key.to_owned());
            Value::Array(Vec::new())
        } else {
            let serialized = if matches!(value, "true" | "false")
                || value.len() == 10 && value.as_bytes().get(4) == Some(&b'-')
            {
                value.to_owned()
            } else {
                toml::Value::String(value.to_owned()).to_string()
            };
            toml::from_str::<toml::Table>(&format!("value = {serialized}"))
                .map_err(|e| format!("unsupported YAML scalar in {source}: {e}"))?["value"]
                .clone()
        };
        if fields.insert(key.to_owned(), toml_value).is_some() {
            return Err(format!("duplicate YAML key {key} in {source}"));
        }
    }
    Ok(fields)
}

fn check_markdown(root: &Path) -> Result<(), String> {
    let paths = tracked_markdown(root)?;
    if paths.len() > MAX_MARKDOWN_FILES {
        return Err(format!(
            "documentation scan exceeds {MAX_MARKDOWN_FILES} Markdown files"
        ));
    }
    let mut headings = BTreeMap::<String, BTreeSet<String>>::new();
    let mut all_text = BTreeMap::<String, String>::new();
    let mut total_bytes = 0_u64;
    for path in &paths {
        let full = root.join(path);
        let metadata = fs::metadata(&full).map_err(|e| format!("{path}: {e}"))?;
        if metadata.len() > MAX_MARKDOWN_BYTES {
            return Err(format!("Markdown exceeds 1 MiB limit: {path}"));
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_MARKDOWN_TOTAL_BYTES {
            return Err("Markdown documentation exceeds 16 MiB total scan limit".into());
        }
        let text = fs::read_to_string(&full).map_err(|e| format!("{path}: {e}"))?;
        let (plain, slug_set) = markdown_without_fences(path, &text)?;
        if plain.starts_with("---") {
            return Err(format!("presentation front matter remains in {path}"));
        }
        headings.insert(path.clone(), slug_set);
        all_text.insert(path.clone(), plain);
    }

    let inventory: Inventory = toml_file(&root.join(INVENTORY))?;
    for file in inventory.files {
        let source =
            fs::read_to_string(root.join(&file.path)).map_err(|e| format!("{}: {e}", file.path))?;
        let fenced = source.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("```") || trimmed.starts_with("~~~")
        });
        if file.fenced_examples != fenced {
            return Err(format!(
                "fenced-example classification mismatch for {}",
                file.path
            ));
        }
        let lower = source.to_ascii_lowercase();
        let license_notice = lower.contains("copyright")
            || source.contains("SPDX-License-Identifier")
            || lower.contains("mit license")
            || lower.contains("apache license");
        if file.license_notice != license_notice {
            return Err(format!("license classification mismatch for {}", file.path));
        }
        let expected_semantic_directive = file.classification == "operational-directives"
            || matches!(
                file.path.as_str(),
                ".github/pull_request_template.md"
                    | "CONTRIBUTING.md"
                    | "SECURITY.md"
                    | "docs/dependency-review.md"
                    | "docs/release-promotion.md"
            )
            || file.path.starts_with("packaging/");
        if file.semantic_directive != expected_semantic_directive {
            return Err(format!(
                "semantic-directive classification mismatch for {}",
                file.path
            ));
        }
        let expected_historical = matches!(
            file.classification.as_str(),
            "published-release-artifact" | "proof-or-verification-evidence" | "knowledge-record"
        );
        if file.historical_evidence != expected_historical {
            return Err(format!(
                "historical evidence classification mismatch for {}",
                file.path
            ));
        }
    }

    for (path, text) in &all_text {
        let reference_targets = check_reference_definitions(path, text)?;
        for target in markdown_targets(text)?.into_iter().chain(reference_targets) {
            check_target(root, path, &target, &headings)?;
        }
    }

    let records: Records = toml_file(&root.join(RECORDS))?;
    let ids: BTreeSet<_> = records
        .records
        .iter()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .collect();
    for (path, text) in &all_text {
        for reference in record_references(text) {
            if !ids.contains(reference.as_str()) {
                return Err(format!(
                    "{path} refers to unknown knowledge record {reference}"
                ));
            }
        }
        if contains_unapproved_comment(text) {
            return Err(format!(
                "unapproved HTML comment outside a code fence: {path}"
            ));
        }
    }
    Ok(())
}

fn markdown_without_fences(path: &str, text: &str) -> Result<(String, BTreeSet<String>), String> {
    let mut plain = String::with_capacity(text.len());
    let mut in_fence: Option<(char, usize)> = None;
    let mut headings = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        if let Some(ch) = marker {
            let count = trimmed.chars().take_while(|c| *c == ch).count();
            if count >= 3 && line.len() - trimmed.len() <= 3 {
                match in_fence {
                    None => in_fence = Some((ch, count)),
                    Some((open, length))
                        if open == ch && count >= length && trimmed[count..].trim().is_empty() =>
                    {
                        in_fence = None
                    }
                    _ => {}
                }
                plain.push('\n');
                continue;
            }
        }
        if in_fence.is_none() {
            if trimmed.starts_with('#') {
                let hashes = trimmed.chars().take_while(|c| *c == '#').count();
                if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
                    headings.insert(slug(trimmed[hashes..].trim().trim_end_matches('#').trim()));
                }
            }
            plain.push_str(line);
        }
        plain.push('\n');
        if plain.len() > MAX_MARKDOWN_BYTES as usize {
            return Err(format!("Markdown scan exceeded limit in {path}"));
        }
    }
    if in_fence.is_some() {
        return Err(format!("unclosed fenced code block in {path}"));
    }
    Ok((plain, headings))
}

fn slug(heading: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in heading.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            slug.push(ch);
            dash = false;
        } else if ch.is_whitespace() && !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn check_reference_definitions(path: &str, text: &str) -> Result<Vec<String>, String> {
    let mut definitions = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix('[') else {
            continue;
        };
        let Some((label, target)) = rest.split_once("]:") else {
            continue;
        };
        let target = target.trim();
        if label.is_empty() || target.is_empty() {
            return Err(format!("malformed Markdown reference definition in {path}"));
        }
        if definitions
            .insert(
                label.to_ascii_lowercase(),
                reference_destination(target).to_owned(),
            )
            .is_some()
        {
            return Err(format!(
                "duplicate Markdown reference definition in {path}: {label}"
            ));
        }
    }
    let mut targets = Vec::new();
    for reference in markdown_reference_uses(text) {
        let target = definitions
            .get(&reference.to_ascii_lowercase())
            .ok_or_else(|| format!("undefined Markdown reference in {path}: {reference}"))?;
        targets.push(target.clone());
    }
    Ok(targets)
}

fn markdown_reference_uses(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = text[i + 1..].find(']') {
                let after = i + 1 + end + 1;
                if text.as_bytes().get(after) == Some(&b'[') {
                    if let Some(close) = text[after + 1..].find(']') {
                        let label = &text[after + 1..after + 1 + close];
                        if !label.is_empty() {
                            out.push(label.to_owned());
                        }
                        i = after + 2 + close;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn markdown_targets(text: &str) -> Result<Vec<String>, String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        if let Some(end) = rest.find(')') {
            targets.push(reference_destination(rest[..end].trim()).to_owned());
            rest = &rest[end + 1..];
        } else {
            return Err("unterminated Markdown link destination".into());
        }
    }
    Ok(targets)
}

fn reference_destination(definition: &str) -> &str {
    let definition = definition.trim();
    if let Some(angle) = definition.strip_prefix('<') {
        return angle.split_once('>').map_or(angle, |(url, _)| url);
    }
    definition
        .split_once(char::is_whitespace)
        .map_or(definition, |(url, _)| url)
}

fn check_target(
    root: &Path,
    source: &str,
    target: &str,
    headings: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), String> {
    if target.is_empty()
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("data:")
    {
        return Ok(());
    }
    let (path_part, fragment) = target.split_once('#').unwrap_or((target, ""));
    let path_part = path_part.split('?').next().unwrap_or(path_part);
    let destination = if path_part.is_empty() {
        source.to_owned()
    } else {
        let joined = PathBuf::from(source)
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path_part);
        normalize_relative(&joined)
            .ok_or_else(|| format!("invalid link target in {source}: {target}"))?
    };
    if !root.join(&destination).is_file() {
        return Err(format!("broken Markdown link in {source}: {target}"));
    }
    if !fragment.is_empty() && destination.ends_with(".md") {
        let fragment = fragment.to_ascii_lowercase();
        if headings
            .get(&destination)
            .is_some_and(|known| !known.contains(&fragment))
        {
            return Err(format!("unknown Markdown heading in {source}: {target}"));
        }
    }
    Ok(())
}

fn normalize_relative(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => parts.push(value.to_str()?.to_owned()),
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn record_references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        if matches!(bytes[i], b'D' | b'O' | b'R')
            && bytes[i + 1] == b'-'
            && bytes[i + 2..i + 6].iter().all(u8::is_ascii_digit)
        {
            let boundary_before = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let boundary_after = i + 6 == bytes.len() || !bytes[i + 6].is_ascii_digit();
            if boundary_before && boundary_after {
                references.push(text[i..i + 6].to_owned());
            }
            i += 6;
        } else {
            i += 1;
        }
    }
    references
}

fn contains_unapproved_comment(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let mut remaining = lower.as_str();
    while let Some(start) = remaining.find("<!--") {
        let after = &remaining[start + 4..];
        let Some(end) = after.find("-->") else {
            return true;
        };
        let body = after[..end].trim();
        if ![
            "markdownlint-",
            "cspell:",
            "spell-checker:",
            "prettier-ignore",
        ]
        .iter()
        .any(|prefix| body.starts_with(prefix))
        {
            return true;
        }
        remaining = &after[end + 3..];
    }
    false
}
