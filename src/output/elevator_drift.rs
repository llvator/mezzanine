//! Drift detection between an Elevator (`.elv`) spec and the code it
//! claims via `cr:` references.
//!
//! Two anchor checks, both derived from what authors already wrote —
//! no new authoring burden:
//!
//! 1. **`cr:` resolution** — every referenced path must exist under
//!    the code root (which may differ from the spec directory when
//!    specs live in a docs repo — `--code-root`). A missing path means
//!    the code moved or died: fix the `cr` or retire the entity.
//!    Error-level, exits 1.
//! 2. **Identifier grounding** — identifier-shaped tokens mentioned in
//!    `d:` descriptions (`CatalogMediaUtil.getImageUrl`, config keys,
//!    `UPPER_SNAKE` job codes) are searched in the text of the files
//!    under the spec's resolved `cr:` paths. A token found nowhere is
//!    a rename/delete suspect — the most common way descriptions rot.
//!    Hint-level, exits 0.
//!
//! Grounding deliberately scans file *text*, not the parsed graph:
//! parser coverage (XML, properties, impex fragments) would otherwise
//! manufacture false "not found"s for tokens the code plainly
//! contains. Deliberately NOT attempted: comparing the spec's shape to
//! the code's shape — the abstraction is not a projection of the code
//! tree, and non-isomorphism (a `fu` pointing into another extension)
//! is the value the spec adds, not drift.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, EntityKind};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Per-file read cap. Claimed files are source/config text; anything
/// larger is a generated artifact whose content can't anchor a
/// description anyway.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub struct DriftReport {
    pub text: String,
    /// True when at least one `cr:` path failed to resolve.
    pub has_errors: bool,
}

pub fn run(result: &AnalysisResult, code_root: &Path) -> DriftReport {
    // entity qualified ref → its cr paths (as written in the spec).
    let mut refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entity in result.entities.iter().filter(|e| e.tags.contains("elevator")) {
        for attr in &entity.attributes {
            if let Some((_, path)) = super::elevator_code_map::parse_cr_attr(attr) {
                refs.entry(short_ref(entity)).or_default().push(path);
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Elevator drift report");
    let _ = writeln!(out, "> code root: {}", code_root.display());

    if refs.is_empty() {
        let _ = writeln!(
            out,
            "\n(no `cr:` references in this spec — drift checks need `cr: \"path\"` anchors to verify)"
        );
        return DriftReport { text: out, has_errors: false };
    }

    // --- Check 1: cr resolution -----------------------------------
    let mut missing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut resolved: BTreeSet<PathBuf> = BTreeSet::new();
    let mut total_refs = 0usize;
    for (entity, paths) in &refs {
        for path in paths {
            total_refs += 1;
            let full = code_root.join(path);
            if full.exists() {
                resolved.insert(full);
            } else {
                missing.entry(entity.as_str()).or_default().push(path.as_str());
            }
        }
    }

    // --- Check 2: identifier grounding ----------------------------
    // Corpus = text of every file under every resolved cr path. Union
    // across the whole spec, not per-entity: descriptions legitimately
    // mention identifiers owned by sibling entities (a consumer `fu`
    // naming the producer's type).
    let corpus = read_corpus(&resolved);

    let mut unanchored: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut grounded_cache: BTreeMap<String, bool> = BTreeMap::new();
    let mut tokens_checked = 0usize;
    for entity in result.entities.iter().filter(|e| e.tags.contains("elevator")) {
        let key = short_ref(entity);
        // Only entities that carry cr refs get grounded — a sketch
        // entity's prose has nothing on disk to anchor against.
        if !refs.contains_key(&key) {
            continue;
        }
        let Some(doc) = entity.documentation.as_deref() else { continue };
        for token in extract_identifier_tokens(doc) {
            tokens_checked += 1;
            let grounded = *grounded_cache
                .entry(token.clone())
                .or_insert_with(|| is_grounded(&token, &corpus));
            if !grounded {
                let entry = unanchored
                    .entry(refs.keys().find(|k| **k == key).unwrap().as_str())
                    .or_default();
                if !entry.contains(&token) {
                    entry.push(token);
                }
            }
        }
    }

    // --- Render ----------------------------------------------------
    let _ = writeln!(
        out,
        "> {} cr reference(s): {} resolved, {} missing — {} identifier(s) checked in {} file(s), {} unanchored",
        total_refs,
        resolved.len(),
        missing.values().map(|v| v.len()).sum::<usize>(),
        tokens_checked,
        corpus.len(),
        unanchored.values().map(|v| v.len()).sum::<usize>(),
    );

    if !missing.is_empty() {
        let _ = writeln!(out, "\n## Missing cr paths (error)");
        let _ = writeln!(
            out,
            "> The code moved or was removed — fix the `cr:` or retire the entity."
        );
        for (entity, paths) in &missing {
            let _ = writeln!(out, "{}", entity);
            for p in paths {
                let _ = writeln!(out, "  cr: {} — not found under code root", p);
            }
        }
    }

    if !unanchored.is_empty() {
        let _ = writeln!(out, "\n## Unanchored identifiers (hint)");
        let _ = writeln!(
            out,
            "> Named in `d:` but found in no file under the spec's resolved cr paths — possibly renamed or removed. Verify against the cr: target and update the description."
        );
        for (entity, tokens) in &unanchored {
            let _ = writeln!(out, "{}", entity);
            for t in tokens {
                let _ = writeln!(out, "  `{}`", t);
            }
        }
    }

    if missing.is_empty() && unanchored.is_empty() {
        let _ = writeln!(out, "\n✓ No drift detected: every cr path resolves and every identifier in `d:` is anchored in the claimed code.");
    }

    DriftReport { text: out, has_errors: !missing.is_empty() }
}

/// Read every file under the resolved cr paths into memory, skipping
/// VCS/build directories and oversized files. Returned as
/// (path, content) pairs; the path is only used for the file count.
fn read_corpus(resolved: &BTreeSet<PathBuf>) -> Vec<(PathBuf, String)> {
    let mut corpus = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for root in resolved {
        collect_files(root, &mut corpus, &mut seen);
    }
    corpus
}

fn collect_files(path: &Path, corpus: &mut Vec<(PathBuf, String)>, seen: &mut BTreeSet<PathBuf>) {
    if path.is_file() {
        if !seen.insert(path.to_path_buf()) {
            return;
        }
        let small_enough = std::fs::metadata(path)
            .map(|m| m.len() <= MAX_FILE_BYTES)
            .unwrap_or(false);
        if small_enough {
            if let Ok(bytes) = std::fs::read(path) {
                corpus.push((path.to_path_buf(), String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
        return;
    }
    let skip = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.') || matches!(n, "target" | "node_modules" | "dist" | "build"))
        .unwrap_or(false);
    if skip {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for child in children {
        collect_files(&child, corpus, seen);
    }
}

/// A token is grounded if it appears verbatim in any claimed file, or
/// — for dotted tokens like `CatalogMediaUtil.getImageUrl`, whose
/// exact dotted form may not occur in source (instance calls) — if
/// every identifier-shaped segment appears somewhere.
fn is_grounded(token: &str, corpus: &[(PathBuf, String)]) -> bool {
    let found = |needle: &str| corpus.iter().any(|(_, text)| text.contains(needle));
    if found(token) {
        return true;
    }
    if token.contains('.') {
        return token.split('.').all(|seg| seg.len() < 4 || found(seg));
    }
    false
}

/// Extract identifier-shaped tokens from a description. Three shapes
/// qualify — chosen to keep prose words out:
/// - CamelCase with ≥2 uppercase and ≥1 lowercase letters, length ≥6
///   (`RemoteMediaRef`, `getImageUrl` — but not `Media` or `Quartz`)
/// - dotted forms where any segment is CamelCase, or all-lowercase
///   config-key forms with ≥3 segments (`catalog.remote.media.format`)
/// - `UPPER_SNAKE` with ≥1 underscore, length ≥4 (`ASSET_UPDATE_JOB`)
fn extract_identifier_tokens(doc: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in doc.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            current.push(c);
        } else {
            flush_token(&mut current, &mut tokens);
        }
    }
    flush_token(&mut current, &mut tokens);
    tokens
}

fn flush_token(current: &mut String, tokens: &mut Vec<String>) {
    let raw = std::mem::take(current);
    let token = raw.trim_matches('.');
    if !token.is_empty() && is_identifier_shaped(token) && !tokens.iter().any(|t| t == token) {
        tokens.push(token.to_string());
    }
}

fn is_identifier_shaped(token: &str) -> bool {
    if token.contains('.') {
        let segments: Vec<&str> = token.split('.').collect();
        if segments.iter().any(|s| s.is_empty() || s.chars().all(|c| c.is_ascii_digit())) {
            return false; // version numbers, ellipsis leftovers
        }
        if segments.iter().any(|s| is_camel(s)) {
            return true;
        }
        // Config-key shape: all lowercase, at least 3 segments.
        return segments.len() >= 3
            && segments.iter().all(|s| s.chars().all(|c| c.is_ascii_lowercase()));
    }
    is_camel(token) || is_upper_snake(token)
}

fn is_camel(s: &str) -> bool {
    s.len() >= 6
        && s.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2
        && s.chars().any(|c| c.is_ascii_lowercase())
        && !s.contains('_')
}

fn is_upper_snake(s: &str) -> bool {
    s.len() >= 4
        && s.contains('_')
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn short_ref(e: &CodeEntity) -> String {
    let prefix = match e.kind {
        EntityKind::Extension => "e",
        EntityKind::Category => "c",
        EntityKind::Feature => "f",
        EntityKind::Functionality => "fu",
        EntityKind::Concept => "@",
        EntityKind::UiPage => "ui",
        _ => "?",
    };
    format!("{} {}", prefix, e.qualified_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Span;
    use std::fs;

    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nao-drift-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn elv_entity(name: &str, doc: &str, cr: &str) -> CodeEntity {
        let mut e = CodeEntity::new(name, EntityKind::Feature, "spec.elv", Span::default());
        e.qualified_name = name.to_string();
        e.tags.insert("elevator".to_string());
        if !doc.is_empty() {
            e.documentation = Some(doc.to_string());
        }
        if !cr.is_empty() {
            e.attributes.push(format!("cr:{}", cr));
        }
        e
    }

    fn result_with(entities: Vec<CodeEntity>) -> AnalysisResult {
        AnalysisResult {
            entities,
            relationships: Vec::new(),
            files: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn missing_cr_path_is_an_error() {
        let dir = TmpDir::new("missing");
        let result = result_with(vec![elv_entity("gone", "Old feature.", "src/removed/")]);
        let report = run(&result, &dir.0);
        assert!(report.has_errors);
        assert!(report.text.contains("Missing cr paths"), "{}", report.text);
        assert!(report.text.contains("src/removed/"), "{}", report.text);
    }

    #[test]
    fn grounded_identifiers_pass_unanchored_flagged() {
        let dir = TmpDir::new("ground");
        dir.write(
            "src/service/AssetService.java",
            "class AssetService { void fetchAssetUrls() {} }",
        );
        let result = result_with(vec![elv_entity(
            "assets",
            "Uses AssetService.fetchAssetUrls, prefers MediaVanishedHelper when set.",
            "src/service/",
        )]);
        let report = run(&result, &dir.0);
        assert!(!report.has_errors);
        assert!(
            report.text.contains("`MediaVanishedHelper`"),
            "renamed identifier should be flagged:\n{}",
            report.text
        );
        assert!(
            !report.text.contains("`AssetService.fetchAssetUrls`"),
            "live identifier must not be flagged:\n{}",
            report.text
        );
    }

    #[test]
    fn clean_spec_reports_no_drift() {
        let dir = TmpDir::new("clean");
        dir.write("src/lib.rs", "pub struct ProtocolBuilder;");
        let result = result_with(vec![elv_entity(
            "protocol",
            "Structured prompts via ProtocolBuilder.",
            "src/lib.rs",
        )]);
        let report = run(&result, &dir.0);
        assert!(!report.has_errors);
        assert!(report.text.contains("✓ No drift detected"), "{}", report.text);
    }

    #[test]
    fn prose_words_are_not_treated_as_identifiers() {
        let tokens = extract_identifier_tokens(
            "Replaces the deprecated Azure CDN. Quartz cron fires at 8. Media rows land via Entrance Area.",
        );
        assert!(tokens.is_empty(), "prose leaked through: {:?}", tokens);
    }

    #[test]
    fn identifier_shapes_are_recognised() {
        let tokens = extract_identifier_tokens(
            "CatalogMediaUtil.getImageUrl prefers RemoteMediaRef[pristine] when catalog.remote.media.format is set; job ASSET_UPDATE_JOB fires.",
        );
        assert!(tokens.contains(&"CatalogMediaUtil.getImageUrl".to_string()), "{:?}", tokens);
        assert!(tokens.contains(&"RemoteMediaRef".to_string()), "{:?}", tokens);
        assert!(tokens.contains(&"catalog.remote.media.format".to_string()), "{:?}", tokens);
        assert!(tokens.contains(&"ASSET_UPDATE_JOB".to_string()), "{:?}", tokens);
    }
}
