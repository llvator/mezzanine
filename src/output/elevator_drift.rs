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

use super::elevator_fix::{self, CrRef, FixPlan};
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
    /// True when at least one `cr:` path failed to resolve — after any
    /// repairs `--fix` applied, so a run that fixed everything it found
    /// exits clean.
    pub has_errors: bool,
}

/// What a drift run was asked to do.
pub struct DriftOptions<'a> {
    /// Root the `cr:` paths resolve against.
    pub code_root: &'a Path,
    /// Root the spec was walked from. Entity file paths are relative to
    /// it, and it is the only tree `--fix` writes in — the code root is
    /// read for evidence and never touched.
    pub spec_root: &'a Path,
    /// Apply the repairs git can prove, instead of only naming them.
    pub fix: bool,
}

pub fn run(result: &AnalysisResult, opts: &DriftOptions) -> DriftReport {
    let code_root = opts.code_root;
    let sites = collect_sites(result, opts.spec_root);

    let mut out = String::new();
    let _ = writeln!(out, "# Elevator drift report");
    let _ = writeln!(out, "> code root: {}", code_root.display());

    if sites.is_empty() {
        let _ = writeln!(
            out,
            "\n(no `cr:` references in this spec — drift checks need `cr: \"path\"` anchors to verify)"
        );
        return DriftReport {
            text: out,
            has_errors: false,
        };
    }

    // Entities carrying at least one cr ref — the only ones whose `d:`
    // prose has something on disk to be grounded against.
    let anchored: BTreeSet<&str> = sites.iter().map(|s| s.entity.as_str()).collect();

    // --- Check 1: cr resolution -----------------------------------
    let mut missing_sites: Vec<CrRef> = Vec::new();
    let mut resolved: BTreeSet<PathBuf> = BTreeSet::new();
    let total_refs = sites.len();
    for site in &sites {
        let full = code_root.join(&site.path);
        if full.exists() {
            resolved.insert(full);
        } else {
            missing_sites.push(site.clone());
        }
    }

    // Rename evidence for whatever died, computed before anything is
    // written so the report reads the same with and without `--fix`.
    let plan = elevator_fix::plan(&missing_sites, code_root);

    let mut missing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for site in &missing_sites {
        missing
            .entry(site.entity.as_str())
            .or_default()
            .push(site.path.as_str());
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
    for entity in result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
    {
        let key = short_ref(entity);
        // Only entities that carry cr refs get grounded — a sketch
        // entity's prose has nothing on disk to anchor against.
        let Some(key) = anchored.get(key.as_str()).copied() else {
            continue;
        };
        let Some(doc) = entity.documentation.as_deref() else {
            continue;
        };
        for token in extract_identifier_tokens(doc) {
            tokens_checked += 1;
            let grounded = *grounded_cache
                .entry(token.clone())
                .or_insert_with(|| is_grounded(&token, &corpus));
            if !grounded {
                let entry = unanchored.entry(key).or_default();
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
        missing_sites.len(),
        tokens_checked,
        corpus.len(),
        unanchored.values().map(|v| v.len()).sum::<usize>(),
    );

    if !missing.is_empty() {
        render_missing(&mut out, &missing, &plan);
    }

    // Applying happens after the report has described what it would do,
    // so a run that fails to write still tells the author what was
    // meant to change.
    let mut repaired: BTreeSet<String> = BTreeSet::new();
    if opts.fix && !plan.repairs.is_empty() {
        let outcome = elevator_fix::apply(&missing_sites, &plan.repairs);
        for a in &outcome.applied {
            repaired.extend(a.paths.iter().cloned());
        }
        render_applied(&mut out, &outcome, &plan);
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

    // What is left after any repairs — a run that fixed everything it
    // found has nothing to fail on.
    let unfixed = missing_sites
        .iter()
        .filter(|s| !repaired.contains(&s.path))
        .count();

    if unfixed == 0 && unanchored.is_empty() {
        let _ = writeln!(out, "\n✓ No drift detected: every cr path resolves and every identifier in `d:` is anchored in the claimed code.");
    }

    DriftReport {
        text: out,
        has_errors: unfixed > 0,
    }
}

/// Every `cr:` reference in the spec, located well enough to be
/// rewritten where it was written.
fn collect_sites(result: &AnalysisResult, spec_root: &Path) -> Vec<CrRef> {
    let mut sites = Vec::new();
    for entity in result
        .entities
        .iter()
        .filter(|e| e.tags.contains("elevator"))
    {
        let file = spec_file(spec_root, entity);
        let span = entity.span.start.offset..entity.span.end.offset;
        for attr in &entity.attributes {
            if let Some((_, path)) = super::elevator_code_map::parse_cr_attr(attr) {
                sites.push(CrRef {
                    entity: short_ref(entity),
                    file: file.clone(),
                    span: span.clone(),
                    path,
                });
            }
        }
    }
    sites
}

/// Where the entity's `.elv` actually lives. Entity file paths are
/// recorded relative to the analyzed root, so they only resolve once
/// rejoined to the spec path the caller walked.
fn spec_file(spec_root: &Path, entity: &CodeEntity) -> PathBuf {
    if entity.file_path.is_absolute() {
        return entity.file_path.clone();
    }
    let joined = spec_root.join(&entity.file_path);
    let joined = if joined.exists() {
        joined
    } else {
        entity.file_path.clone()
    };
    // `.` roots on both sides otherwise stack up as `././spec/x.elv`,
    // which resolves fine but reads like a bug in the report.
    joined.components().collect()
}

/// The missing-path section, with whatever evidence was found for each
/// path attached to it. A path with no evidence renders exactly as it
/// did before this feature existed.
fn render_missing(out: &mut String, missing: &BTreeMap<&str, Vec<&str>>, plan: &FixPlan) {
    let _ = writeln!(out, "\n## Missing cr paths (error)");
    let _ = writeln!(
        out,
        "> The code moved or was removed — fix the `cr:` or retire the entity."
    );
    for (entity, paths) in missing {
        let _ = writeln!(out, "{}", entity);
        for p in paths {
            let _ = writeln!(out, "  cr: {} — not found under code root", p);
            if let Some(repair) = plan.repairs.get(*p) {
                let _ = writeln!(
                    out,
                    "      → renamed to {} in commit {} — `--fix` applies this",
                    repair.new,
                    &repair.commit[..repair.commit.len().min(8)],
                );
            } else if let Some(candidate) = plan.candidates.get(*p) {
                let _ = writeln!(
                    out,
                    "      ? only same-named file left is {} — unverified, never applied; confirm it yourself",
                    candidate,
                );
            }
        }
    }
    if let Some(note) = &plan.note {
        let _ = writeln!(out, "> {}", note);
    }
}

fn render_applied(out: &mut String, outcome: &elevator_fix::ApplyOutcome, plan: &FixPlan) {
    let fixed: usize = outcome.applied.iter().map(|a| a.paths.len()).sum();
    let _ = writeln!(out, "\n## Applied fixes");
    let _ = writeln!(
        out,
        "> Rewrote {} of {} repairable cr path(s) across {} spec file(s), from git rename evidence.",
        fixed,
        plan.repairs.len(),
        outcome.applied.len(),
    );
    for a in &outcome.applied {
        let _ = writeln!(out, "{}", a.file.display());
        for p in &a.paths {
            if let Some(repair) = plan.repairs.get(p) {
                let _ = writeln!(out, "  {} → {}", p, repair.new);
            }
        }
    }
    if let Some(e) = &outcome.error {
        let _ = writeln!(
            out,
            "> Stopped after an error: {}\n> Files listed above were written; the rest were not.",
            e
        );
    }
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
                corpus.push((
                    path.to_path_buf(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
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
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
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
        if segments
            .iter()
            .any(|s| s.is_empty() || s.chars().all(|c| c.is_ascii_digit()))
        {
            return false; // version numbers, ellipsis leftovers
        }
        if segments.iter().any(|s| is_camel(s)) {
            return true;
        }
        // Config-key shape: all lowercase, at least 3 segments.
        return segments.len() >= 3
            && segments
                .iter()
                .all(|s| s.chars().all(|c| c.is_ascii_lowercase()));
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
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
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
    use crate::models::{Position, Span};
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

    /// Co-located specs with no fixing — what every check-only test
    /// wants, and the default the CLI applies when `--code-root` is
    /// omitted.
    fn opts(root: &Path) -> DriftOptions<'_> {
        DriftOptions {
            code_root: root,
            spec_root: root,
            fix: false,
        }
    }

    fn result_with(entities: Vec<CodeEntity>) -> AnalysisResult {
        AnalysisResult {
            entities,
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn missing_cr_path_is_an_error() {
        let dir = TmpDir::new("missing");
        let result = result_with(vec![elv_entity("gone", "Old feature.", "src/removed/")]);
        let report = run(&result, &opts(&dir.0));
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
        let report = run(&result, &opts(&dir.0));
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
        let report = run(&result, &opts(&dir.0));
        assert!(!report.has_errors);
        assert!(
            report.text.contains("✓ No drift detected"),
            "{}",
            report.text
        );
    }

    // --- `--fix` -------------------------------------------------
    //
    // These drive a real git repository rather than a stubbed log: the
    // whole claim of the feature is "git already knows", and a fake
    // rename log would test the parser while assuming the thing worth
    // checking — that git reports the move at all.

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {:?}: {}", args, e));
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Drift Test"]);
    }

    /// Write a one-Feature `.elv` and hand back an entity spanning it,
    /// with byte offsets taken from the text the parser would see.
    fn spec_entity(dir: &TmpDir, rel: &str, name: &str, crs: &[&str]) -> CodeEntity {
        let quoted: Vec<String> = crs.iter().map(|c| format!("\"{}\"", c)).collect();
        let text = format!(
            "f {} {{\n    d: \"A feature.\"\n    cr: {}\n}}\n",
            name,
            quoted.join(", ")
        );
        dir.write(rel, &text);
        let end = text.rfind('}').unwrap() + 1;
        let mut e = CodeEntity::new(
            name,
            EntityKind::Feature,
            rel,
            Span::new(Position::new(0, 0, 0), Position::new(3, 1, end)),
        );
        e.qualified_name = name.to_string();
        e.tags.insert("elevator".to_string());
        e.documentation = Some("A feature.".to_string());
        for c in crs {
            e.attributes.push(format!("cr:{}", c));
        }
        e
    }

    #[test]
    fn a_moved_file_is_rewritten_from_the_rename_git_recorded() {
        let dir = TmpDir::new("fix-file");
        init_repo(&dir.0);
        dir.write("src/old.rs", "pub struct Thing;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "thing", &["src/old.rs"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/old.rs", "src/new.rs"]);
        commit_all(&dir.0, "move it");

        let result = result_with(vec![entity]);
        let report = run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/new.rs\""),
            "spec not rewritten:\n{}",
            spec
        );
        assert!(
            !report.has_errors,
            "a fully repaired spec must exit clean:\n{}",
            report.text
        );
    }

    #[test]
    fn a_moved_directory_is_rewritten_when_its_files_agree() {
        let dir = TmpDir::new("fix-dir");
        init_repo(&dir.0);
        dir.write("src/output/a.rs", "pub struct A;\n");
        dir.write("src/output/b.rs", "pub struct B;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "render", &["src/output/"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/output", "src/report"]);
        commit_all(&dir.0, "move the folder");

        let result = result_with(vec![entity]);
        let report = run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/report/\""),
            "spec not rewritten:\n{}",
            spec
        );
        assert!(!report.has_errors, "{}", report.text);
    }

    #[test]
    fn a_deleted_path_is_reported_and_left_alone() {
        let dir = TmpDir::new("fix-deleted");
        init_repo(&dir.0);
        dir.write("src/doomed.rs", "pub struct Doomed;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "doomed", &["src/doomed.rs"]);
        commit_all(&dir.0, "initial");
        fs::remove_file(dir.0.join("src/doomed.rs")).unwrap();
        commit_all(&dir.0, "delete it");

        let result = result_with(vec![entity]);
        let report = run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/doomed.rs\""),
            "a deletion must not be guessed at:\n{}",
            spec
        );
        assert!(
            report.has_errors,
            "unrepaired drift must still fail:\n{}",
            report.text
        );
    }

    #[test]
    fn a_sibling_path_on_the_same_line_is_untouched() {
        let dir = TmpDir::new("fix-sibling");
        init_repo(&dir.0);
        dir.write("src/old.rs", "pub struct Thing;\n");
        dir.write("src/keep.rs", "pub struct Keep;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "pair", &["src/old.rs", "src/keep.rs"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/old.rs", "src/new.rs"]);
        commit_all(&dir.0, "move one");

        let result = result_with(vec![entity]);
        run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("cr: \"src/new.rs\", \"src/keep.rs\""),
            "the live sibling and the line's shape must survive:\n{}",
            spec
        );
    }

    #[test]
    fn a_rename_chain_lands_on_the_path_that_exists() {
        let dir = TmpDir::new("fix-chain");
        init_repo(&dir.0);
        dir.write("src/one.rs", "pub struct Chained;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "chained", &["src/one.rs"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/one.rs", "src/two.rs"]);
        commit_all(&dir.0, "first move");
        git(&dir.0, &["mv", "src/two.rs", "src/three.rs"]);
        commit_all(&dir.0, "second move");

        let result = result_with(vec![entity]);
        run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/three.rs\""),
            "chain not followed to the end:\n{}",
            spec
        );
    }

    /// The rename is real and git reports it, but the file it moved to
    /// was deleted afterwards. Following the evidence alone would write
    /// a path that is already dead — which is why existence is a second
    /// gate rather than an assumption.
    #[test]
    fn a_rename_into_a_path_that_was_then_deleted_is_not_applied() {
        let dir = TmpDir::new("fix-moved-then-deleted");
        init_repo(&dir.0);
        dir.write("src/one.rs", "pub struct Gone;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "gone", &["src/one.rs"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/one.rs", "src/two.rs"]);
        commit_all(&dir.0, "move it");
        fs::remove_file(dir.0.join("src/two.rs")).unwrap();
        commit_all(&dir.0, "then delete it");

        let result = result_with(vec![entity]);
        let report = run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/one.rs\""),
            "a rename to a path that no longer exists must not be applied:\n{}",
            spec
        );
        assert!(report.has_errors, "{}", report.text);
    }

    #[test]
    fn without_fix_the_rename_is_named_but_nothing_is_written() {
        let dir = TmpDir::new("fix-dry");
        init_repo(&dir.0);
        dir.write("src/old.rs", "pub struct Thing;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "thing", &["src/old.rs"]);
        commit_all(&dir.0, "initial");
        git(&dir.0, &["mv", "src/old.rs", "src/new.rs"]);
        commit_all(&dir.0, "move it");

        let result = result_with(vec![entity]);
        let report = run(&result, &opts(&dir.0));

        assert!(
            report.text.contains("renamed to src/new.rs"),
            "{}",
            report.text
        );
        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/old.rs\""),
            "reporting must not write:\n{}",
            spec
        );
        assert!(report.has_errors);
    }

    #[test]
    fn a_lone_same_named_file_is_offered_but_never_applied() {
        let dir = TmpDir::new("fix-candidate");
        init_repo(&dir.0);
        // Created fresh rather than moved, so git has no rename to
        // report and only the basename connects the two.
        dir.write("src/other/helper.rs", "pub struct Helper;\n");
        let entity = spec_entity(&dir, "spec/app.elv", "helper", &["src/helper.rs"]);
        commit_all(&dir.0, "initial");

        let result = result_with(vec![entity]);
        let report = run(
            &result,
            &DriftOptions {
                code_root: &dir.0,
                spec_root: &dir.0,
                fix: true,
            },
        );

        assert!(
            report.text.contains("src/other/helper.rs"),
            "the candidate should be named:\n{}",
            report.text
        );
        let spec = fs::read_to_string(dir.0.join("spec/app.elv")).unwrap();
        assert!(
            spec.contains("\"src/helper.rs\""),
            "a candidate must never be applied:\n{}",
            spec
        );
        assert!(
            report.has_errors,
            "an unconfirmed candidate is not a repair:\n{}",
            report.text
        );
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
        assert!(
            tokens.contains(&"CatalogMediaUtil.getImageUrl".to_string()),
            "{:?}",
            tokens
        );
        assert!(
            tokens.contains(&"RemoteMediaRef".to_string()),
            "{:?}",
            tokens
        );
        assert!(
            tokens.contains(&"catalog.remote.media.format".to_string()),
            "{:?}",
            tokens
        );
        assert!(
            tokens.contains(&"ASSET_UPDATE_JOB".to_string()),
            "{:?}",
            tokens
        );
    }
}
