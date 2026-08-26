//! Code analysis and dependency resolution.

mod dependency_resolver;
mod file_walker;
pub mod folder_shape;
pub mod grouping;
mod lsp_tracer;
mod markdown_links;
mod parse_store;
mod receiver_index;
pub mod relayout;
pub mod sql_fold;

pub use dependency_resolver::DependencyResolver;
pub use file_walker::{is_test_path, FileWalker};
pub use parse_store::ParseStore;
pub(crate) use parse_store::cache_root;

use crate::config::Config;
use crate::models::file_info::Language;
use crate::models::{
    CodeEntity, EntityKind, FileInfo, Position, Precision, Relationship, RelationshipKind, Span,
};
use crate::parser::{self};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Sentinel error returned by `Analyzer::analyze_with_cancel` when the
/// caller flips the cancel flag mid-run. Callers catch this and skip
/// any work that would persist the half-built result (writing JSON,
/// swapping the in-memory graph, emitting reload events).
#[derive(Debug, thiserror::Error)]
#[error("analysis cancelled")]
pub struct Cancelled;

/// Convert a set cancel flag into `Err(Cancelled)`, otherwise `Ok(())`.
/// Inlined at every stage boundary so a flipped flag aborts before the
/// next expensive pass.
fn check_cancel(cancel: &Arc<AtomicBool>) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Cancelled.into())
    } else {
        Ok(())
    }
}

/// Longest type fragment a synthesized `name: type` node label carries.
/// Past this the label has stopped being a name and started being source.
const MAX_LABEL_TYPE_CHARS: usize = 60;

/// Build the display name of a synthesized parameter or field node.
///
/// These names are read at a glance on the graph canvas, so the type has
/// to survive as one short line. A language parser can hand back a type
/// spanning many lines — a Rust enum variant body, a TypeScript inline
/// object type — and without this the whole block became the label.
fn member_label(name: &str, type_name: Option<&str>) -> String {
    let Some(type_name) = type_name else {
        return name.to_string();
    };
    let flat = type_name.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return name.to_string();
    }
    if flat.chars().count() <= MAX_LABEL_TYPE_CHARS {
        return format!("{name}: {flat}");
    }
    let kept: String = flat.chars().take(MAX_LABEL_TYPE_CHARS).collect();
    format!("{name}: {}…", kept.trim_end())
}

/// Main analyzer that coordinates parsing and dependency analysis.
pub struct Analyzer {
    config: Config,
    /// Parsed entities indexed by file
    file_entities: HashMap<PathBuf, Vec<CodeEntity>>,
    /// All entities indexed by ID
    entities: HashMap<String, CodeEntity>,
    /// All relationships
    relationships: Vec<Relationship>,
    /// File information
    files: HashMap<PathBuf, FileInfo>,
    /// Import information by file
    imports: HashMap<PathBuf, Vec<crate::parser::language_parser::ImportInfo>>,
    /// Where each cross-file import was written (AN-024). Filled by
    /// `resolve_dependencies` from the same map above, once the whole
    /// file set is known — a specifier cannot be resolved to a file
    /// before the analysis knows which files there are.
    import_sites: Vec<crate::models::ImportSite>,
    /// Cross-file warnings surfaced during resolution (e.g. Elevator
    /// out-of-scope references). Populated by the post-merge passes
    /// that have access to the merged graph; flushed into
    /// `AnalysisResult::warnings`.
    warnings: Vec<String>,
}

impl Analyzer {
    /// Create a new analyzer with the given configuration
    pub fn new(config: Config) -> Self {
        Self {
            config,
            file_entities: HashMap::new(),
            entities: HashMap::new(),
            relationships: Vec::new(),
            files: HashMap::new(),
            imports: HashMap::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Run the analysis on the configured path. Equivalent to
    /// `analyze_with_cancel` with a never-flipped flag — preserved so
    /// callers that don't need cancellation (CLI `analyze`, tests) stay
    /// unchanged.
    pub fn analyze(&mut self) -> Result<AnalysisResult> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.analyze_with_cancel(&cancel)
    }

    /// Like `analyze` but checks `cancel` between stages and inside the
    /// rayon parse loop. Returns `Err(Cancelled)` (downcastable via
    /// `anyhow::Error::downcast_ref::<Cancelled>()`) if the flag is set —
    /// callers should skip persisting the partial result.
    pub fn analyze_with_cancel(&mut self, cancel: &Arc<AtomicBool>) -> Result<AnalysisResult> {
        check_cancel(cancel)?;
        let files = self.discover_files(cancel)?;
        check_cancel(cancel)?;
        let store = ParseStore::open();
        let mut parse_results = self.parse_files(&files, &store, cancel);
        check_cancel(cancel)?;
        self.fold_sql_schema(&mut parse_results);
        check_cancel(cancel)?;
        self.merge_parse_results(parse_results);
        check_cancel(cancel)?;
        self.resolve_deferred_receivers();
        check_cancel(cancel)?;
        self.trace_lsp_exact_calls();
        check_cancel(cancel)?;
        self.resolve_and_filter()?;
        Ok(self.build_result())
    }

    /// Stage 1: walk the configured root and return the list of source files
    /// eligible for parsing. Filtering (exclude patterns, test heuristics,
    /// language gating) happens inside `FileWalker`.
    ///
    /// Two roots rather than one when `spec_dir` points outside the analyzed
    /// tree — a docs repo beside the code, or the top of a monorepo whose
    /// services are watched one at a time. The second walk yields `.elv`
    /// files only, and yields nothing at all in the ordinary case, so the
    /// concatenation costs nothing to leave in.
    fn discover_files(&self, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>> {
        FileWalker::new(&self.config).walk_all(&self.config.root_path, cancel)
    }

    /// Stage 2: parse every discovered file in parallel, with a progress bar.
    /// Files whose language isn't in the configured language filter are
    /// skipped; parse errors are reported as warnings and dropped. The
    /// `cancel` flag is polled per file (one `Relaxed` load) and a set
    /// flag short-circuits the rest of the batch.
    fn parse_files(
        &self,
        files: &[PathBuf],
        store: &ParseStore,
        cancel: &Arc<AtomicBool>,
    ) -> Vec<ParsedFile> {
        let progress = ProgressBar::new(files.len() as u64);
        progress.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} Parsing [{bar:40.cyan/dim}] {pos}/{len} {msg}",
            )
            .unwrap()
            .progress_chars("━╸─"),
        );

        let analysis = &self.config.analysis;
        let results: Vec<_> = files
            .par_iter()
            .filter_map(|file_path| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let language = parser::detect_language(file_path);
                // Same predicate the walker used to discover this file. It
                // was a second, independently-written copy of the rule, and
                // the two disagreed the moment `include_docs` existed:
                // the walker admitted a `.md` and this dropped it.
                if !analysis.accepts_language(language) {
                    progress.inc(1);
                    return None;
                }
                let result = Self::parse_file_standalone(file_path, language, store);
                progress.inc(1);
                match result {
                    Ok(parsed) => Some(parsed),
                    Err(e) => {
                        progress.suspend(|| {
                            eprintln!("Warning: Failed to parse {}: {}", file_path.display(), e);
                        });
                        None
                    }
                }
            })
            .collect();
        progress.finish_with_message("done");
        let (hits, misses) = store.stats();
        eprintln!(
            "  Parse store: {} hits, {} misses (re-parsed {} changed/new files)",
            hits, misses, misses
        );
        results
    }

    /// Stage 2b: replay SQL migrations in filename order and attach the
    /// resulting tables to the files that created them.
    ///
    /// `.sql` files arrive here with `schema_ops` and no entities, because a
    /// migration states a *change* to the schema rather than the schema —
    /// what a table finally looks like is not knowable from one file. This is
    /// the only stage in the pipeline where file order is load-bearing; see
    /// ADR-0007 for why it is confined to one pass instead of pushed into the
    /// parser, where it would cost the per-file parse cache.
    fn fold_sql_schema(&mut self, parse_results: &mut [ParsedFile]) {
        let inputs: Vec<sql_fold::FileOps<'_>> = parse_results
            .iter()
            .filter(|f| !f.schema_ops.is_empty())
            .map(|f| sql_fold::FileOps {
                path: f.file_path.as_path(),
                ops: &f.schema_ops,
            })
            .collect();
        if inputs.is_empty() {
            return;
        }

        let folded = sql_fold::fold(inputs);
        if folded.alters_on_unknown_tables > 0 {
            self.warnings.push(format!(
                "SQL fold: {} statement(s) altered a table no migration creates \
                 (declared outside the migration set, or created inside a \
                 procedural block)",
                folded.alters_on_unknown_tables
            ));
        }

        // Attribute each table back to the migration that created it, so the
        // file → entity containment the rest of the pipeline builds is right.
        let mut by_file: HashMap<PathBuf, Vec<CodeEntity>> = HashMap::new();
        for entity in folded.entities {
            by_file
                .entry(entity.file_path.clone())
                .or_default()
                .push(entity);
        }
        for file in parse_results.iter_mut() {
            file.schema_ops.clear();
            if let Some(entities) = by_file.remove(&file.file_path) {
                file.entities.extend(entities);
            }
        }

        // Foreign keys are cross-file by nature, so they belong to no single
        // migration. Park them on the first SQL file; the graph assembler
        // resolves them by id regardless of which file carries them.
        if let Some(first) = parse_results
            .iter_mut()
            .find(|f| f.file_info.language == crate::models::file_info::Language::Sql)
        {
            first.relationships.extend(folded.relationships);
        }
    }

    /// Stage 3: fold parse results into the analyzer's aggregate state —
    /// `files`, `file_entities`, `entities`, `relationships`, `imports`.
    fn merge_parse_results(&mut self, parse_results: Vec<ParsedFile>) {
        for parsed in parse_results {
            self.files
                .insert(parsed.file_info.path.clone(), parsed.file_info);
            for entity in parsed.entities {
                self.file_entities
                    .entry(entity.file_path.clone())
                    .or_default()
                    .push(entity.clone());
                self.entities.insert(entity.id.clone(), entity);
            }
            self.relationships.extend(parsed.relationships);
            if !parsed.imports.is_empty() {
                self.imports.insert(parsed.file_path, parsed.imports);
            }
            self.warnings.extend(parsed.warnings);
        }
    }

    /// Stage 3.4 (AN-012): finish the receiver paths the Rust parser could
    /// only type halfway.
    ///
    /// A field hop needs the struct's declaration, which routinely sits in
    /// another file — `entity.kind.is_callable()` stalls on `.kind` because
    /// `CodeEntity` is declared in `models/entity.rs`. The parser cannot reach
    /// it (one file at a time, and the parse store caches per file), so it
    /// hands over where it stalled and this pass finishes the walk against
    /// every struct in the tree. See [`receiver_index`].
    ///
    /// Runs after merge, so every struct's fields are known, and before the
    /// LSP tracer, so an exact resolution still overrides this one.
    fn resolve_deferred_receivers(&mut self) {
        let index = receiver_index::FieldIndex::build(self.entities.values());
        receiver_index::resolve_deferred(&mut self.relationships, &index);
    }

    /// Stage 3.5 (AN-004): upgrade Rust call edges to LSP-exact resolution.
    ///
    /// Runs after merge (so all callable entities and their spans exist) and
    /// before resolve/synthesis (so branch re-sourcing and parameter/field
    /// synthesis see final targets). For each `Calls` edge the Rust parser
    /// tagged with a call-site position, ask rust-analyzer for the exact
    /// definition and, when it maps to a known entity, rewrite the target and
    /// mark the edge `Exact`. Everything left keeps its heuristic target and
    /// the `Heuristic` label applied later in `graph::from_analysis`.
    ///
    /// Opt-in (`MEZZ_LSP_EXACT=1`): default off so it never re-costs the
    /// self-review hook AN-003 made cheap. Best-effort: the tracer degrades to
    /// an empty upgrade map on any failure (disabled, no server, no manifest,
    /// timeout), so this pass can only ever improve precision, never break
    /// analysis. The transient position metadata is stripped here regardless,
    /// so it never reaches the graph or renderers.
    ///
    /// The resolution itself is gated on `allow_unsafe_passes` — see
    /// [`Self::apply_lsp_upgrades`].
    fn trace_lsp_exact_calls(&mut self) {
        let mut sites: Vec<lsp_tracer::CallSite> = Vec::new();
        for (idx, rel) in self.relationships.iter().enumerate() {
            if rel.kind != RelationshipKind::Calls {
                continue;
            }
            let (Some(line), Some(col)) = (
                rel.metadata.get("lsp_line").and_then(|v| v.parse().ok()),
                rel.metadata.get("lsp_col").and_then(|v| v.parse().ok()),
            ) else {
                continue;
            };
            let Some(source) = self.entities.get(&rel.source_id) else {
                continue;
            };
            sites.push(lsp_tracer::CallSite {
                rel_idx: idx,
                file: source.file_path.clone(),
                line,
                col,
            });
        }

        if !sites.is_empty() {
            self.apply_lsp_upgrades(&sites);
        }

        // Strip the transient call-site positions from every call edge
        // (upgraded or not) so they never leak into the cached graph.
        for rel in &mut self.relationships {
            if rel.kind == RelationshipKind::Calls {
                rel.metadata.remove("lsp_line");
                rel.metadata.remove("lsp_col");
            }
        }
    }

    /// Resolve `sites` through rust-analyzer and rewrite the edges it can
    /// place exactly.
    ///
    /// Gated on `config.analysis.allow_unsafe_passes`, which `mezz serve`
    /// clears: resolution drives `cargo check`, which executes the analyzed
    /// repo's `build.rs` and proc-macros. That's fine for a tree the operator
    /// chose and unacceptable for one a visitor pasted a URL for. The check
    /// lives at the config layer, so it holds regardless of what
    /// `MEZZ_LSP_EXACT` says in the environment.
    fn apply_lsp_upgrades(&mut self, sites: &[lsp_tracer::CallSite]) {
        if !self.config.analysis.allow_unsafe_passes {
            return;
        }
        let callables: Vec<CodeEntity> = self
            .entities
            .values()
            .filter(|e| e.kind.is_callable())
            .cloned()
            .collect();
        let upgrades = lsp_tracer::resolve_exact_calls(&self.config.root_path, &callables, sites);
        for (rel_idx, target_id) in upgrades {
            let rel = &mut self.relationships[rel_idx];
            rel.target_id = target_id;
            rel.precision = Some(Precision::Exact);
        }
    }

    /// Stage 4: enrich with synthetic parameter entities, resolve
    /// cross-entity dependencies, and apply configured filters.
    fn resolve_and_filter(&mut self) -> Result<()> {
        eprintln!("  Extracting parameters...");
        self.create_parameter_entities();

        eprintln!("  Extracting class fields...");
        self.create_field_entities();

        eprintln!("  Grouping calls by conditional branch...");
        self.create_branch_entities();

        eprintln!("  Resolving dependencies...");
        let resolver =
            DependencyResolver::new(&self.config, &self.entities, &self.imports, &self.files);
        let dep_relationships = resolver.resolve()?;
        self.import_sites = resolver.import_sites();
        self.relationships.extend(dep_relationships);

        self.validate_elevator_imports();
        self.resolve_markdown_links();
        self.synthesise_unresolved_stubs();
        self.derive_parent_from_contains();

        self.apply_filters();
        Ok(())
    }

    /// Fill in `parent_id` for entities whose containment was declared
    /// in a different file than their definition. Single-file parsers
    /// can set `parent_id` directly because they own both endpoints,
    /// but languages like Elevator (`.elv`) where one file declares
    /// `c library { f protocol }` and another defines `f protocol`
    /// need a post-merge pass: stable IDs make the `Contains` edge
    /// connect cleanly, but the child entity's `parent_id` field
    /// still has to be patched in once both halves have been merged.
    ///
    /// First-Contains-targeting-this-entity wins, mirroring the
    /// in-parser "first parent wins" rule. Entities that already had
    /// a `parent_id` set in their parser are left alone.
    /// Validate that every Elevator cross-file reference points at an
    /// entity defined in a file that the source's file imports
    /// (transitively). Out-of-scope references are tagged on the
    /// relationship's metadata (`unresolved=true`,
    /// `reason=out_of_scope`) so the renderer can show them
    /// distinctively, and a warning is recorded for the user.
    ///
    /// Strict explicit-only model: ambient resolution is disabled. A
    /// reference resolves only if the target's file is in the
    /// source's import scope (own file ∪ transitive imports).
    fn validate_elevator_imports(&mut self) {
        // 1. Build per-file import set, restricted to Elevator files
        //    and resolved to canonical PathBufs so the comparison is
        //    insensitive to relative-vs-absolute and `./` quirks.
        let mut direct_imports: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for (file_path, imports) in &self.imports {
            let lang = self
                .files
                .get(file_path)
                .map(|f| f.language)
                .unwrap_or(Language::Unknown);
            if lang != Language::Elevator {
                continue;
            }
            let dir = file_path.parent().unwrap_or(Path::new("."));
            let mut resolved: HashSet<PathBuf> = HashSet::new();
            for imp in imports {
                let candidate = dir.join(&imp.path);
                let canon = candidate.canonicalize().unwrap_or(candidate);
                resolved.insert(canon);
            }
            let canon_self = file_path
                .canonicalize()
                .unwrap_or_else(|_| file_path.clone());
            direct_imports.insert(canon_self, resolved);
        }

        // 2. Compute transitive closure per file. Cycles allowed; the
        //    visited set guards against infinite loops.
        let mut scopes: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for file_path in direct_imports.keys() {
            let mut scope: HashSet<PathBuf> = HashSet::new();
            let mut stack: Vec<PathBuf> = vec![file_path.clone()];
            while let Some(current) = stack.pop() {
                if !scope.insert(current.clone()) {
                    continue;
                }
                if let Some(direct) = direct_imports.get(&current) {
                    for d in direct {
                        if !scope.contains(d) {
                            stack.push(d.clone());
                        }
                    }
                }
            }
            scopes.insert(file_path.clone(), scope);
        }

        // 3. Validate each relationship whose source is an Elevator
        //    entity. We collect violations first then mutate so we
        //    don't fight the borrow checker.
        let mut violations: Vec<usize> = Vec::new();
        for (i, rel) in self.relationships.iter().enumerate() {
            let Some(source) = self.entities.get(&rel.source_id) else {
                continue;
            };
            if !is_elevator_entity(source) {
                continue;
            }
            let Some(target) = self.entities.get(&rel.target_id) else {
                continue; // Truly missing — handled by the stub pass.
            };
            let source_file = source
                .file_path
                .canonicalize()
                .unwrap_or_else(|_| source.file_path.clone());
            let target_file = target
                .file_path
                .canonicalize()
                .unwrap_or_else(|_| target.file_path.clone());
            if source_file == target_file {
                continue; // Same-file references always resolve.
            }
            let in_scope = scopes
                .get(&source_file)
                .map(|s| s.contains(&target_file))
                .unwrap_or(false);
            if !in_scope {
                violations.push(i);
            }
        }
        for i in &violations {
            let rel = &mut self.relationships[*i];
            rel.metadata
                .insert("unresolved".to_string(), "true".to_string());
            rel.metadata
                .insert("reason".to_string(), "out_of_scope".to_string());
        }

        // 4. Emit one warning per (source_file, target_file) pair so
        //    the user gets concrete "add `import \"...\"` to <file>"
        //    guidance without N copies for N references.
        let mut already_warned: HashSet<(PathBuf, PathBuf)> = HashSet::new();
        for i in &violations {
            let rel = &self.relationships[*i];
            let source = self.entities.get(&rel.source_id);
            let target = self.entities.get(&rel.target_id);
            if let (Some(s), Some(t)) = (source, target) {
                let key = (s.file_path.clone(), t.file_path.clone());
                if already_warned.insert(key) {
                    self.warnings.push(format!(
                        "elevator: `{}` references `{}` defined in {} but {} doesn't import it",
                        s.qualified_name,
                        t.qualified_name,
                        t.file_path.display(),
                        s.file_path.display(),
                    ));
                }
            }
        }
    }

    /// Synthesise stub entities for relationship targets that don't
    /// exist anywhere in the parsed project — i.e. references with no
    /// definition in any file. Tags them `unresolved` so the renderer
    /// can flag the broken-link state.
    ///
    /// This runs *after* the merge so it has the full picture: a stub
    /// only appears if no real definition was emitted by any file. No
    /// race with parser-side stubs (we removed those for non-UI
    /// kinds).
    /// Resolve the markdown link forms that need the whole corpus:
    /// wikilinks (which name a note without locating it) and code refs
    /// (which the parser could only record as absolute paths). See
    /// [`markdown_links`].
    fn resolve_markdown_links(&mut self) {
        let unresolved = markdown_links::resolve_wikilinks(&self.entities, &mut self.relationships);
        if !unresolved.is_empty() {
            self.warnings.push(format!(
                "markdown: {} wikilink target(s) match no note: {}",
                unresolved.len(),
                unresolved.join(", ")
            ));
        }
        markdown_links::relativize_code_refs(&mut self.entities, &self.config.root_path);
    }

    fn synthesise_unresolved_stubs(&mut self) {
        // Capture (kind, file_path_to_inherit) per missing target.
        // The stub inherits the file_path of whichever entity first
        // referenced it. That keeps the stub colocated with the
        // referrer in the file tree / Visual Scopes panel — putting
        // it under a synthetic `<unresolved>` path instead would
        // bury it in a sibling tree branch the reader rarely opens.
        let mut to_create: HashMap<String, (EntityKind, PathBuf)> = HashMap::new();
        for rel in &self.relationships {
            if self.entities.contains_key(&rel.target_id) {
                continue;
            }
            let Some(kind) = stub_kind_from_id(&rel.target_id) else {
                continue;
            };
            let inherited_path = self
                .entities
                .get(&rel.source_id)
                .map(|e| e.file_path.clone())
                .unwrap_or_else(|| PathBuf::from("<unresolved>"));
            to_create
                .entry(rel.target_id.clone())
                .or_insert((kind, inherited_path));
        }
        for (id, (kind, file_path)) in to_create {
            let qualname = stub_qualname_from_id(&id).unwrap_or_else(|| id.clone());
            let leaf = stub_leaf(&id, &qualname);
            let span = Span::new(Position::new(0, 0, 0), Position::new(0, 0, 0));
            let mut entity = CodeEntity::new(leaf, kind, file_path, span);
            entity.id = id.clone();
            entity.qualified_name = qualname;
            entity.tags.insert(stub_layer_tag(&id).to_string());
            entity.tags.insert("unresolved".to_string());
            self.entities.insert(id, entity);
        }
    }

    fn derive_parent_from_contains(&mut self) {
        let mut assignments: HashMap<String, String> = HashMap::new();
        for rel in &self.relationships {
            if rel.kind != RelationshipKind::Contains {
                continue;
            }
            // Out-of-scope edges shouldn't establish a parent: the
            // import-validation pass marked them as unresolved
            // because the source's file didn't import the target's
            // file. Treating them as parents would silently re-enable
            // ambient resolution.
            if rel.metadata.get("unresolved").map(String::as_str) == Some("true") {
                continue;
            }
            let already_set = self
                .entities
                .get(&rel.target_id)
                .map(|e| e.parent_id.is_some())
                .unwrap_or(true); // missing entity → skip
            if already_set {
                continue;
            }
            assignments
                .entry(rel.target_id.clone())
                .or_insert_with(|| rel.source_id.clone());
        }
        for (child_id, parent_id) in assignments {
            if let Some(child) = self.entities.get_mut(&child_id) {
                child.parent_id = Some(parent_id);
            }
        }
    }

    /// Stage 5: snapshot the aggregate state into an `AnalysisResult`.
    ///
    /// Order is pinned here (AN-002): the aggregate maps are HashMaps whose
    /// iteration order varies per process, and downstream consumers resolve
    /// ambiguous names first-match — unsorted output made fan-in and smells
    /// flap between identical runs.
    fn build_result(&self) -> AnalysisResult {
        let mut entities: Vec<CodeEntity> = self.entities.values().cloned().collect();
        entities.sort_by(|a, b| {
            (&a.file_path, a.span.start.offset, &a.id).cmp(&(
                &b.file_path,
                b.span.start.offset,
                &b.id,
            ))
        });
        let mut relationships = self.relationships.clone();
        relationships.sort_by(|a, b| {
            (&a.source_id, &a.target_id, a.kind.display_label(), &a.label).cmp(&(
                &b.source_id,
                &b.target_id,
                b.kind.display_label(),
                &b.label,
            ))
        });
        let mut files: Vec<FileInfo> = self.files.values().cloned().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        AnalysisResult {
            entities,
            relationships,
            files,
            import_sites: self.import_sites.clone(),
            warnings: self.warnings.clone(),
        }
    }

    /// Parse a single file without requiring &mut self (suitable for
    /// parallel execution). Consults the AN-003 parse store first: on a
    /// usable hit ([`usable_hit`]) it returns the stored `ParsedFile`
    /// verbatim, skipping tree-sitter entirely; on a miss it parses and
    /// persists.
    fn parse_file_standalone(
        path: &Path,
        language: Language,
        store: &ParseStore,
    ) -> Result<ParsedFile> {
        // Read once; the hash keys the store and (below) populates
        // FileInfo.content_hash, so there's no second read on a miss.
        let content = std::fs::read_to_string(path)?;
        let content_hash = ParseStore::content_hash(&content);
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if let Some(parsed) = usable_hit(store, &abs_path, path, &content_hash) {
            return Ok(parsed);
        }

        let result = parser::parse_content(path, &content, language)?;

        let file_info = FileInfo {
            path: path.to_path_buf(),
            language,
            size: content.len() as u64,
            line_count: content.lines().count(),
            content_hash: Some(content_hash.clone()),
            documentation: result.file_documentation,
        };

        let parsed = ParsedFile {
            file_path: path.to_path_buf(),
            file_info,
            entities: result.entities,
            relationships: result.relationships,
            imports: result.imports,
            schema_ops: result.schema_ops,
            warnings: result
                .warnings
                .into_iter()
                .map(|w| format!("{}: {}", path.display(), w))
                .collect(),
        };
        store.put(&abs_path, &content_hash, &parsed);
        Ok(parsed)
    }

    /// Create Parameter entities for each function/method parameter
    /// and TakesParam relationships pointing into the function.
    fn create_parameter_entities(&mut self) {
        let callables: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.kind.is_callable() && !e.parameters.is_empty())
            .map(|e| {
                (
                    e.id.clone(),
                    e.file_path.clone(),
                    e.span,
                    e.parameters.clone(),
                )
            })
            .collect();

        for (func_id, file_path, span, params) in callables {
            for param in &params {
                // Skip `self` / `&self` / `&mut self`
                if param.name == "self" || param.name == "&self" || param.name == "&mut self" {
                    continue;
                }

                let display_name = member_label(&param.name, param.type_name.as_deref());

                let param_id = format!("{}::param::{}", func_id, param.name);
                let mut entity =
                    CodeEntity::new(display_name, EntityKind::Parameter, file_path.clone(), span);
                entity.id = param_id.clone();
                entity.qualified_name = entity.name.clone();
                entity.parent_id = Some(func_id.clone());

                self.entities.insert(param_id.clone(), entity);

                let rel =
                    Relationship::new(param_id, func_id.clone(), RelationshipKind::TakesParam);
                self.relationships.push(rel);
            }
        }
    }

    /// Create synthetic Variable entities for each class field (class-level
    /// assignments and `self.x = …` instance attributes already collected by
    /// language parsers into `entity.fields`). Emits `Contains` relationships
    /// from the parent class so the graph renders them as children of the
    /// class, alongside methods and properties.
    fn create_field_entities(&mut self) {
        let containers: Vec<_> = self
            .entities
            .values()
            .filter(|e| {
                !e.fields.is_empty()
                    && matches!(
                        e.kind,
                        EntityKind::Class
                            | EntityKind::Dataclass
                            | EntityKind::AbstractClass
                            | EntityKind::Struct
                            | EntityKind::Interface
                            | EntityKind::Trait
                            | EntityKind::Enum
                    )
            })
            .map(|e| (e.id.clone(), e.file_path.clone(), e.span, e.fields.clone()))
            .collect();

        for (owner_id, file_path, span, fields) in containers {
            for field in &fields {
                // Skip obviously-synthetic names the language parsers
                // produced before we fixed the subscript / attribute-chain
                // bugs (still defensive, cheap).
                if field.name.is_empty() || field.name.contains('[') || field.name.contains('.') {
                    continue;
                }

                let display_name = member_label(&field.name, field.type_name.as_deref());

                let field_id = format!("{}::field::{}", owner_id, field.name);
                // De-dupe across multiple synthesis paths (e.g. same file
                // analyzed twice on watch reload).
                if self.entities.contains_key(&field_id) {
                    continue;
                }

                let mut entity =
                    CodeEntity::new(display_name, EntityKind::Variable, file_path.clone(), span);
                entity.id = field_id.clone();
                entity.qualified_name = entity.name.clone();
                entity.parent_id = Some(owner_id.clone());
                entity.tags.insert("class_field".to_string());

                self.entities.insert(field_id.clone(), entity);
                // No explicit Contains relationship — the subsequent
                // DependencyResolver pass derives one from `parent_id`,
                // and emitting it here too just duplicates the edge.
            }
        }
    }

    /// Group calls that live inside the same conditional arm under a
    /// synthetic Branch entity. The language parser tags each Calls /
    /// Instantiates relationship with a `branch` metadata key holding
    /// a dotted path of branch ids (e.g. `c1.c3` for a call nested
    /// inside `c3` which is inside `c1`). This pass:
    ///
    ///   1. Walks each tagged path's prefixes so it creates one Branch
    ///      entity per segment (`c1`, then `c1.c3`) and wires
    ///      `parent_id` to the preceding segment — or to the caller at
    ///      the root — so a nested branch renders as a child of its
    ///      enclosing branch, not of the method.
    ///   2. Re-sources the tagged relationship so its source is the
    ///      innermost Branch (the full path), then strips the metadata
    ///      key now that grouping is encoded structurally.
    ///
    /// Branch nodes are tagged `branch_node` so downstream counts (fan-
    /// out, fan-in, entity totals) can filter them out the same way they
    /// filter Parameter entities — they're a rendering concept, not
    /// genuine program structure.
    fn create_branch_entities(&mut self) {
        use std::collections::HashMap;

        // Collect the full set of unique (caller_id, branch_path) pairs
        // that appear in tagged relationships, along with a
        // representative file_path and span copied from the caller.
        struct BranchInfo {
            file_path: std::path::PathBuf,
            span: crate::models::Span,
        }
        let mut branches: HashMap<(String, String), BranchInfo> = HashMap::new();
        for rel in &self.relationships {
            let Some(branch) = rel.metadata.get("branch") else {
                continue;
            };
            let key = (rel.source_id.clone(), branch.clone());
            if branches.contains_key(&key) {
                continue;
            }
            let Some(caller) = self.entities.get(&rel.source_id) else {
                continue;
            };
            branches.insert(
                key,
                BranchInfo {
                    file_path: caller.file_path.clone(),
                    span: caller.span,
                },
            );
        }

        // Create Branch entities for every prefix of every path. For a
        // call tagged `c1.c3`, that means one entity for `c1` (parent =
        // caller) and one for `c1.c3` (parent = the `c1` entity). We key
        // the id map by full path so the re-source pass can find the
        // innermost branch directly.
        let mut branch_ids: HashMap<(String, String), String> = HashMap::new();
        for ((caller_id, branch_path), info) in &branches {
            let mut parent_entity_id = caller_id.clone();
            let mut accumulated = String::new();
            for segment in branch_path.split('.') {
                if !accumulated.is_empty() {
                    accumulated.push('.');
                }
                accumulated.push_str(segment);
                let entity_id = format!("{}::branch::{}", caller_id, accumulated);
                let prefix_key = (caller_id.clone(), accumulated.clone());
                if !self.entities.contains_key(&entity_id) {
                    let mut entity = CodeEntity::new(
                        // Display name is the full accumulated path
                        // (`c1`, `c1.1`, `c1.2`, …) so a reader can
                        // tell at a glance which branch nests under
                        // which — structural parent_id still encodes
                        // the hierarchy, but the label makes it
                        // visible inline on the node.
                        accumulated.clone(),
                        EntityKind::Branch,
                        info.file_path.clone(),
                        info.span,
                    );
                    entity.id = entity_id.clone();
                    entity.qualified_name = format!("{}::{}", caller_id, accumulated);
                    entity.parent_id = Some(parent_entity_id.clone());
                    entity.tags.insert("branch_node".to_string());
                    self.entities.insert(entity_id.clone(), entity);
                }
                branch_ids.insert(prefix_key, entity_id.clone());
                parent_entity_id = entity_id;
            }
        }

        // Re-source every tagged call relationship so its new source is
        // the innermost Branch node (the full-path entity). Drop the
        // metadata key — structural parent_id chain now encodes the
        // full branch grouping.
        for rel in self.relationships.iter_mut() {
            let Some(branch) = rel.metadata.get("branch").cloned() else {
                continue;
            };
            let key = (rel.source_id.clone(), branch);
            if let Some(branch_entity_id) = branch_ids.get(&key) {
                rel.source_id = branch_entity_id.clone();
                rel.metadata.remove("branch");
            }
        }
    }

    /// Apply configured filters to entities and relationships
    fn apply_filters(&mut self) {
        self.drop_local_assignments();

        let filters = &self.config.filters;

        // Filter entities by kind
        if !filters.entity_kinds.is_empty() {
            self.entities
                .retain(|_, e| filters.entity_kinds.contains(&e.kind));
        }

        // Filter relationships by kind
        if !filters.relationship_kinds.is_empty() {
            self.relationships
                .retain(|r| filters.relationship_kinds.contains(&r.kind));
        }

        // Filter by minimum weight
        if filters.min_weight > 0 {
            self.relationships
                .retain(|r| r.weight >= filters.min_weight);
        }

        // Filter by name patterns (if any)
        // TODO: Implement regex matching
    }

    /// Drop the assignments nothing reads, unless `include_locals` asks for
    /// them (CFG-005).
    ///
    /// One entity per assignment is most of a real graph — 72% of tinygrad's
    /// entities and 46% of its edges — while `mcp::tools::is_listed` hides
    /// `Variable` from every agent answer and the canvas draws at most 2,000
    /// nodes. So the default analysis built and shipped a population no
    /// consumer displays.
    ///
    /// Two carve-outs, and they are what keeps this a *cost* cut rather than
    /// a fidelity cut:
    ///
    /// - **Class fields stay.** The canvas injects `class_field`-tagged
    ///   Variables as a class's field column, so they are drawn, not merely
    ///   stored. (Per-class `field_count` is computed from `entity.fields` in
    ///   the parsers and does not depend on these entities either way.)
    /// - **Anything doing structural work stays.** A name whose whole edge
    ///   set is `Contains` (its parent declaring it) and `WritesTo` (someone
    ///   assigning it) is a leaf nobody navigates through. One that *calls*,
    ///   *instantiates*, *returns* or is *used as a type* is a participant in
    ///   the call graph, and dropping it would silently delete a real edge —
    ///   Python's module-level registries and decorator tables are exactly
    ///   this. On tinygrad that exemption keeps 200 entities and with them
    ///   3,562 relationships, so fan-in, fan-out and every coupling metric
    ///   downstream are untouched by the cut.
    ///
    /// Runs before the `entity_kinds` allow-list, and at graph assembly
    /// rather than in the parsers: `ParseStore` is keyed by (path, parser
    /// version) with nothing about the run in it, so a parser that honoured
    /// this flag would hand a cached locals-included parse to a run with the
    /// flag off and assemble a mixed graph with no hash mismatch to catch it.
    /// The cost of filtering here is that parse time is unchanged; the
    /// benefit is one cache generation per repo, and a flag whose flip
    /// re-assembles without re-parsing anything.
    fn drop_local_assignments(&mut self) {
        if self.config.analysis.include_locals {
            return;
        }

        let structural = self.structural_endpoints();

        let dropped: HashSet<String> = self
            .entities
            .values()
            .filter(|e| matches!(e.kind, EntityKind::Variable | EntityKind::Constant))
            .filter(|e| !e.tags.contains("class_field"))
            .filter(|e| {
                !structural.contains(e.id.as_str()) && !structural.contains(e.name.as_str())
            })
            .map(|e| e.id.clone())
            .collect();

        if dropped.is_empty() {
            return;
        }

        self.entities.retain(|id, _| !dropped.contains(id));
        // An edge naming an entity the payload no longer holds is a dangling
        // reference every consumer would have to guard against.
        self.relationships
            .retain(|r| !dropped.contains(&r.source_id) && !dropped.contains(&r.target_id));
        // `file_entities` is the per-file view the same entities are also
        // stored in; leaving it whole would let them back in through
        // `analyze_file` and the diff path.
        for entities in self.file_entities.values_mut() {
            entities.retain(|e| !dropped.contains(&e.id));
        }
    }

    /// Every name that appears at either end of an edge that is not mere
    /// containment or assignment — plus the last qualified segment of each.
    ///
    /// The segments are why this is not a one-liner. At this point in the
    /// pipeline a call's `target_id` is often still the name the source wrote
    /// (`adder`, `mod.adder`) rather than an entity id;
    /// `DependencyGraph::from_analysis` is what matches those against entity
    /// *names*, minting a ghost when nothing matches. An exemption that
    /// checked ids alone would therefore miss the case it exists for —
    /// `adder = functools.partial(make, 1)` was dropped and the call to it
    /// landed on `ghost:adder`. Matching what the graph builder matches keeps
    /// the two passes agreeing about what a name refers to.
    ///
    /// Both separators, because both spell the same thing. A Rust reader of
    /// `use crate::vocab::LIMIT` records `vocab::LIMIT` (AN-028), and reading
    /// only the dotted spelling left the constant looking unreferenced and
    /// dropped it out from under its own edge. Adding `::` rescued no other
    /// entity in this repo, so it costs the cut nothing.
    fn structural_endpoints(&self) -> HashSet<&str> {
        let mut ends = HashSet::new();
        for rel in &self.relationships {
            if matches!(
                rel.kind,
                RelationshipKind::Contains | RelationshipKind::WritesTo
            ) {
                continue;
            }
            for end in [rel.source_id.as_str(), rel.target_id.as_str()] {
                Self::with_segments(end, &mut ends);
            }
        }
        ends
    }

    /// One endpoint and the last segment of it under either qualifier.
    ///
    /// Beside [`Self::structural_endpoints`] rather than inside it because
    /// the loop it sits in is charged for every branch it holds, and the
    /// second separator was one branch more than the ceiling allows.
    fn with_segments<'a>(end: &'a str, ends: &mut HashSet<&'a str>) {
        ends.insert(end);
        for separator in [".", "::"] {
            if let Some(last) = end.rsplit(separator).next() {
                ends.insert(last);
            }
        }
    }

    /// Analyze a specific file
    pub fn analyze_file(&mut self, path: &Path) -> Result<AnalysisResult> {
        let language = parser::detect_language(path);
        let store = ParseStore::open();
        let parsed = Self::parse_file_standalone(path, language, &store)?;
        self.files
            .insert(parsed.file_info.path.clone(), parsed.file_info);
        for entity in parsed.entities {
            self.file_entities
                .entry(entity.file_path.clone())
                .or_default()
                .push(entity.clone());
            self.entities.insert(entity.id.clone(), entity);
        }
        self.relationships.extend(parsed.relationships);
        if !parsed.imports.is_empty() {
            self.imports.insert(parsed.file_path, parsed.imports);
        }
        // Single-file analysis sees only the structs analyzed so far, so most
        // deferred receivers stay unresolved here. The pass still runs: it is
        // what strips the transient hints, and they must not reach the graph.
        self.resolve_deferred_receivers();

        let file_entities: Vec<CodeEntity> =
            self.file_entities.get(path).cloned().unwrap_or_default();

        let entity_ids: std::collections::HashSet<_> =
            file_entities.iter().map(|e| &e.id).collect();

        let relationships: Vec<Relationship> = self
            .relationships
            .iter()
            .filter(|r| entity_ids.contains(&r.source_id) || entity_ids.contains(&r.target_id))
            .cloned()
            .collect();

        Ok(AnalysisResult {
            entities: file_entities,
            relationships,
            files: self.files.get(path).cloned().into_iter().collect(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    /// Get entities at a specific depth from the root
    pub fn get_entities_at_depth(&self, depth: usize) -> Vec<&CodeEntity> {
        if depth == 0 {
            // Return top-level entities (no parent)
            self.entities
                .values()
                .filter(|e| e.parent_id.is_none())
                .collect()
        } else {
            // Return entities at the specified depth
            // This requires traversing the containment hierarchy
            let mut current_level: Vec<&CodeEntity> = self
                .entities
                .values()
                .filter(|e| e.parent_id.is_none())
                .collect();

            for _ in 0..depth {
                let parent_ids: std::collections::HashSet<_> =
                    current_level.iter().map(|e| &e.id).collect();

                current_level = self
                    .entities
                    .values()
                    .filter(|e| {
                        e.parent_id
                            .as_ref()
                            .map(|p| parent_ids.contains(p))
                            .unwrap_or(false)
                    })
                    .collect();
            }

            current_level
        }
    }
}

/// A stored parse that may actually be used for this walk: right content,
/// and parsed at the path being walked *now*.
///
/// The second half is not belt-and-braces. The store keys on the canonical
/// path, while everything the entry records — `file_path`, `FileInfo::path`,
/// every entity's `file_path` — is the path as walked, and one physical file
/// has as many walked paths as there are ways to reach it: a symlinked spec
/// directory, `mezz analyze .` versus an absolute root, a repo checked out
/// twice. Serving the entry regardless puts the *other* spelling on every
/// entity, so click-to-open and `cr:` anchors name a path this run never
/// saw — and since content is what decides a hit, it stays wrong until
/// someone edits the file.
///
/// Keying on the walked path instead looks simpler and is worse: the store
/// is machine-wide, so a bare `./src/main.rs` is not a unique name and two
/// repos with an identical file would trade entries. Canonical for identity,
/// walked path for validity.
fn usable_hit(
    store: &ParseStore,
    abs_path: &Path,
    walked: &Path,
    content_hash: &str,
) -> Option<ParsedFile> {
    store
        .get(abs_path, content_hash)
        .filter(|parsed| parsed.file_path == walked)
}

/// Intermediate result from parsing a single file (used for parallel
/// parsing). Serializable so the AN-003 parse store can persist and reload
/// it verbatim — a store hit reconstructs exactly what a cold parse would
/// have produced.
#[derive(serde::Serialize, serde::Deserialize)]
struct ParsedFile {
    file_path: PathBuf,
    file_info: FileInfo,
    entities: Vec<CodeEntity>,
    relationships: Vec<Relationship>,
    imports: Vec<crate::parser::language_parser::ImportInfo>,
    /// SQL schema operations awaiting the fold. Empty for every other
    /// language. Drained by `fold_sql_schema` before the merge — a `.sql`
    /// file's entities do not exist until every migration has been replayed.
    schema_ops: Vec<crate::parser::sql::ops::PositionedOp>,
    /// Per-file parse warnings (lex errors, malformed syntax, etc.).
    /// Propagated into `Analyzer::warnings` during merge so they reach
    /// `AnalysisResult::warnings` and the user. Without this they'd
    /// be silently dropped — exactly the failure mode that lets a
    /// stray double-quote zero out a whole `.elv` file.
    warnings: Vec<String>,
}

/// True if the entity was emitted by the Elevator parser. Identified
/// by the `elevator` tag the parser attaches to every entity it
/// produces.
fn is_elevator_entity(e: &CodeEntity) -> bool {
    e.tags.contains("elevator")
}

/// Parse the kind segment of an Elevator entity ID
/// (`elevator::<kind>.<qualname>`) back into an `EntityKind`. Returns
/// `None` for IDs that aren't shaped like an Elevator ID.
fn elevator_kind_from_id(id: &str) -> Option<EntityKind> {
    let rest = id.strip_prefix("elevator::")?;
    let segment = rest.split('.').next()?;
    match segment {
        "e" => Some(EntityKind::Extension),
        "c" => Some(EntityKind::Category),
        "f" => Some(EntityKind::Feature),
        "fu" => Some(EntityKind::Functionality),
        "concept" => Some(EntityKind::Concept),
        "ui" => Some(EntityKind::UiPage),
        _ => None,
    }
}

/// Extract the qualified-name portion of an Elevator ID. For
/// `elevator::fu.protocol.creation` returns `protocol.creation`.
fn elevator_qualname_from_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("elevator::")?;
    let mut parts = rest.splitn(2, '.');
    parts.next()?; // skip kind segment
    parts.next().map(|s| s.to_string())
}

/// The kind a dangling relationship target should be given as a stub, or
/// `None` for an ID from a layer that does not want ghosts.
///
/// Two layers do. Elevator, where a reference to an undefined Feature is
/// drift worth seeing. And Markdown, where a link to a note that does not
/// exist is the single most useful thing a document graph can show you —
/// Obsidian draws it as a hollow node and so does mezz, through this path.
fn stub_kind_from_id(id: &str) -> Option<EntityKind> {
    if id.starts_with("md::") {
        return Some(EntityKind::Note);
    }
    elevator_kind_from_id(id)
}

/// The qualified name a stub should carry, per layer.
fn stub_qualname_from_id(id: &str) -> Option<String> {
    if let Some(name) = id.strip_prefix("md::wiki.") {
        return Some(name.to_string());
    }
    if let Some(path) = id.strip_prefix("md::note.") {
        return Some(path.to_string());
    }
    elevator_qualname_from_id(id)
}

/// The display name a stub carries.
///
/// An Elevator qualname is dot-separated and its last segment is the name.
/// The two markdown forms are neither: a wikilink is already a name and may
/// itself contain dots ("v1.2 notes"), and a note path wants its filename
/// rather than the whole path.
fn stub_leaf(id: &str, qualname: &str) -> String {
    if id.starts_with("md::wiki.") {
        return qualname.to_string();
    }
    if id.starts_with("md::note.") {
        return PathBuf::from(qualname)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(qualname)
            .to_string();
    }
    qualname.rsplit('.').next().unwrap_or(qualname).to_string()
}

/// The layer tag a stub inherits, so filters and `isSpecEntity`-style
/// predicates treat it like the real entities beside it.
fn stub_layer_tag(id: &str) -> &'static str {
    if id.starts_with("md::") {
        "markdown"
    } else {
        "elevator"
    }
}

/// Result of code analysis.
///
/// `Serialize`/`Deserialize` exist so `mezz serve` can snapshot a finished
/// analysis to disk and rebuild the `DependencyGraph` from it on restart
/// (SRV-004) — the *rendered* JSON the UI consumes is a lossy projection and
/// can't reconstruct the graph that `/scope` traverses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// All discovered entities
    pub entities: Vec<CodeEntity>,

    /// All discovered relationships
    pub relationships: Vec<Relationship>,

    /// Information about analyzed files
    pub files: Vec<FileInfo>,

    /// Where each cross-file import was written (AN-024). File-granular
    /// rather than entity-granular, and so beside the relationships
    /// rather than among them — see [`crate::models::ImportSite`].
    pub import_sites: Vec<crate::models::ImportSite>,

    /// Any warnings generated during analysis
    pub warnings: Vec<String>,
}

impl AnalysisResult {
    /// Get entities of a specific kind
    pub fn entities_of_kind(&self, kind: crate::models::EntityKind) -> Vec<&CodeEntity> {
        self.entities.iter().filter(|e| e.kind == kind).collect()
    }

    /// Get relationships of a specific kind
    pub fn relationships_of_kind(
        &self,
        kind: crate::models::RelationshipKind,
    ) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    /// Get all dependencies (imports, calls, etc.) for an entity
    pub fn dependencies_of(&self, entity_id: &str) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| r.source_id == entity_id && r.kind.is_dependency())
            .collect()
    }

    /// Get entities that depend on a specific entity
    pub fn dependents_of(&self, entity_id: &str) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| r.target_id == entity_id && r.kind.is_dependency())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    // --------------------------------------------------------------
    //  Type-only imports, end to end (AN-022)
    // --------------------------------------------------------------

    /// The ticket's fixture, written to disk and analysed whole.
    ///
    /// `main.ts` and `a.ts` both name a type from `vocab.ts` and nothing
    /// else, so neither specifier survives compilation; `main.ts` also
    /// imports a *function* from `a.ts`, which does.
    fn type_only_fixture(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mezz-an022-{}-{}-{}",
            name,
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("main.ts"),
            "import type { T } from './vocab';\n\
             import { f } from './a';\n\
             export function main(t: T) { f(t); }\n",
        )
        .unwrap();
        // The other spelling: the keyword on the specifier, not the
        // statement, and no value beside it to keep the module alive.
        std::fs::write(
            src.join("a.ts"),
            "import { type T } from './vocab';\n\
             export function f(_t: T) {}\n",
        )
        .unwrap();
        std::fs::write(src.join("vocab.ts"), "export type T = 'x' | 'y';\n").unwrap();
        root
    }

    fn analyse(root: &Path) -> AnalysisResult {
        let config = Config {
            root_path: root.to_path_buf(),
            ..Config::default()
        };
        Analyzer::new(config).analyze().unwrap()
    }

    /// Both spellings reach the site, and the ordinary import beside them
    /// does not.
    #[test]
    fn both_spellings_of_import_type_reach_the_edge() {
        let root = type_only_fixture("sites");
        let result = analyse(&root);

        let mark = |from: &str, to: &str| {
            result
                .import_sites
                .iter()
                .find(|s| s.from.ends_with(from) && s.to.ends_with(to))
                .unwrap_or_else(|| panic!("no site {from} → {to}: {:?}", result.import_sites))
                .is_type_only
        };
        assert!(mark("main.ts", "vocab.ts"), "statement-level `import type`");
        assert!(mark("a.ts", "vocab.ts"), "specifier-level `{{ type T }}`");
        assert!(!mark("main.ts", "a.ts"), "an ordinary `import {{ f }}`");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// AN-025's answer, end to end: an arrow the build erases is left out
    /// of every shape score (ADR 0026).
    ///
    /// The fixture is AN-022's own miniature of the failure. Two of three
    /// edges step one level down; `main.ts → vocab.ts` skips one, because
    /// `main.ts` also reaches `vocab.ts` through `a.ts` — and both arrows
    /// onto `vocab.ts` are `import type`. Before this ticket `src` was
    /// held at `tangled`, `layering` 0.67, by two edges no bundler ever
    /// resolves, which is exactly the reading five rounds of `reshape`
    /// could not offer.
    ///
    /// Deliberately replaces AN-022's `marking_an_edge_erased_moves_no_score`,
    /// which asserted these same numbers unchanged and said in its own doc
    /// comment that a reviewer changing the answer should be changing this
    /// test on purpose. This is that change.
    #[test]
    fn an_erased_edge_is_left_out_of_the_shape_scores() {
        let root = type_only_fixture("shape");
        let result = analyse(&root);
        let graph = crate::graph::DependencyGraph::from_analysis(&result);

        let src = graph
            .folder_metrics()
            .iter()
            .find(|m| m.path.ends_with("src"))
            .expect("the fixture has one folder");
        let shape = src.metrics.shape.as_ref().expect("the folder is drawn");

        // One arrow left — `main.ts → a.ts`, the one ordinary import in
        // the fixture — and it steps a level cleanly.
        assert_eq!(shape.layering, Some(1.0), "the erased edges still count");
        assert_eq!(shape.acyclicity, 1.0);
        // Every child is still drawn: discounting an arrow does not delete
        // the file it points at. `vocab.ts` is now reached by nothing, so
        // ADR 0022 charges it as a second root and the gate moves from
        // `layering` to `arborescence` rather than disappearing.
        assert_eq!(shape.child_count, 3);
        assert_eq!(shape.arborescence, Some(0.5));
        assert_eq!(shape.pattern, crate::models::ShapePattern::Hierarchical);
        assert!(
            matches!(shape.blocker, Some(crate::models::ShapeBlocker::Merges(_))),
            "{:?}",
            shape.blocker
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// One physical file, two ways to reach it — which is all a symlink is,
    /// and the normal shape of a spec directory kept outside the tree it
    /// describes. The parse store keys on the canonical path, so both
    /// spellings land on one entry; the entry records the path it was walked
    /// at. Serving it to the other spelling puts a path this run never saw
    /// on every entity, and since the content is what decides a hit, it
    /// stays wrong until someone edits the file.
    #[cfg(unix)]
    #[test]
    fn a_second_path_to_one_file_does_not_inherit_the_first_path() {
        let root = std::env::temp_dir().join(format!("mezz-linked-parse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("real")).unwrap();
        let real = root.join("real").join("thing.rs");
        std::fs::write(&real, "fn thing() {}\n").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let linked = root.join("link").join("thing.rs");

        let store = ParseStore::open_at(root.join("cache"));
        let first = Analyzer::parse_file_standalone(&real, Language::Rust, &store).unwrap();
        assert_eq!(first.file_path, real);

        let second = Analyzer::parse_file_standalone(&linked, Language::Rust, &store).unwrap();
        assert_eq!(
            second.file_path, linked,
            "the cached entry's path leaked through"
        );
        assert!(
            second.entities.iter().all(|e| e.file_path == linked),
            "entities kept the other spelling: {:?}",
            second
                .entities
                .iter()
                .map(|e| &e.file_path)
                .collect::<Vec<_>>()
        );

        // And the first spelling still hits, rather than each run evicting
        // the other — the point is a correct memo, not a disabled one.
        let again = Analyzer::parse_file_standalone(&real, Language::Rust, &store).unwrap();
        assert_eq!(again.file_path, real);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A node label is a name, not a source excerpt. Whatever a language
    /// parser hands back as a type, the synthesized parameter/field label
    /// stays one short line.
    #[test]
    fn a_long_or_wrapped_type_cannot_run_away_with_the_label() {
        assert_eq!(member_label("path", Some("PathBuf")), "path: PathBuf");
        assert_eq!(member_label("path", None), "path");
        assert_eq!(
            member_label("cfg", Some("{\n  a: u32,\n  b: u32,\n}")),
            "cfg: { a: u32, b: u32, }"
        );

        let long = member_label("x", Some(&"Deep<".repeat(40)));
        assert!(long.chars().count() <= MAX_LABEL_TYPE_CHARS + 5, "{long}");
        assert!(long.ends_with('…'), "truncation should be visible: {long}");
    }

    /// Two analyses of the same tree must be identical — same entity
    /// order, same relationship targets, same fan-in/out, same smells
    /// (AN-002). Guards the sorted `build_result` snapshot and the
    /// sorted candidate lists in `DependencyResolver`: without them,
    /// per-instance HashMap seeds made ambiguous names resolve to a
    /// different file's entity on each run, flapping threshold smells.
    #[test]
    fn analysis_is_deterministic_across_runs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/analyzer");
        let run = || {
            let config = Config::for_path(&dir);
            let mut analyzer = Analyzer::new(config);
            let result = analyzer.analyze().expect("analysis should succeed");
            let graph = crate::graph::DependencyGraph::from_analysis(&result);

            let ids: Vec<&str> = result
                .entities
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>();
            let rels: Vec<(String, String, &'static str)> = result
                .relationships
                .iter()
                .map(|r| {
                    (
                        r.source_id.clone(),
                        r.target_id.clone(),
                        r.kind.display_label(),
                    )
                })
                .collect();
            let fans: Vec<(String, u32, u32, usize)> = graph
                .entities()
                .map(|e| {
                    (
                        e.id.clone(),
                        e.metrics.fan_in,
                        e.metrics.fan_out,
                        e.metrics.smells.len(),
                    )
                })
                .collect();
            (
                ids.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                rels,
                fans,
            )
        };

        assert_eq!(run(), run(), "two runs on an identical tree diverged");
    }

    /// The CFG-005 fixture: a module constant, a function local, two class
    /// fields, and a module-level name that is *called* elsewhere.
    fn assignments_fixture(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mezz-cfg005-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("app.py"),
            "import functools\n\
             \n\
             MAX_RETRIES = 3\n\
             \n\
             def make(op, value):\n\
             \x20   return op + value\n\
             \n\
             adder = functools.partial(make, 1)\n\
             \n\
             def use():\n\
             \x20   return adder(2)\n\
             \n\
             class Session:\n\
             \x20   def __init__(self, token):\n\
             \x20       self.token = token\n\
             \x20       self.count = 0\n\
             \n\
             \x20   def bump(self):\n\
             \x20       step = 1\n\
             \x20       self.count += step\n\
             \x20       return self.count\n",
        )
        .expect("fixture");
        dir
    }

    fn analyze_fixture(dir: &Path, include_locals: bool) -> AnalysisResult {
        let mut config = Config::for_path(dir);
        config.analysis.include_locals = include_locals;
        Analyzer::new(config)
            .analyze()
            .expect("analysis should succeed")
    }

    fn named<'a>(result: &'a AnalysisResult, name: &str) -> Vec<&'a CodeEntity> {
        result.entities.iter().filter(|e| e.name == name).collect()
    }

    /// CFG-005. One entity per assignment is 72% of a real graph and no
    /// consumer lists it — `is_listed` hides `Variable` from every agent
    /// answer and the canvas draws at most 2,000 nodes.
    #[test]
    fn a_default_analysis_drops_the_names_nothing_navigates() {
        let dir = assignments_fixture("drops");
        let result = analyze_fixture(&dir, false);

        assert!(
            named(&result, "MAX_RETRIES").is_empty(),
            "module constant survived"
        );
        assert!(named(&result, "step").is_empty(), "function local survived");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The first carve-out. The canvas draws `class_field` Variables as a
    /// class's field column, so they are displayed, not merely stored.
    #[test]
    fn a_class_keeps_its_fields_and_the_count_they_report() {
        let dir = assignments_fixture("fields");
        let with = analyze_fixture(&dir, true);
        let without = analyze_fixture(&dir, false);

        for field in ["token", "count"] {
            let kept = named(&without, field);
            assert!(
                kept.iter().any(|e| e.tags.contains("class_field")),
                "class field `{field}` was dropped with the locals"
            );
        }

        let count_of = |r: &AnalysisResult| {
            r.entities
                .iter()
                .find(|e| e.name == "Session")
                .and_then(|e| e.metrics.field_count)
        };
        assert_eq!(
            count_of(&with),
            count_of(&without),
            "field_count moved with the cut"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The second carve-out, and the one that keeps this a cost cut rather
    /// than a fidelity cut. `adder = functools.partial(make, 1)` is a
    /// function alias: drop it and the call to it resolves to `ghost:adder`
    /// instead of the real name. The exemption has to match what
    /// `DependencyGraph::from_analysis` matches — names, not just ids —
    /// because at filter time a call's target is still the name as written.
    #[test]
    fn an_assignment_something_calls_is_not_a_leaf() {
        let dir = assignments_fixture("alias");
        let result = analyze_fixture(&dir, false);

        assert!(
            !named(&result, "adder").is_empty(),
            "a called module-level name was dropped as a leaf"
        );

        let graph = crate::graph::DependencyGraph::from_analysis(&result);
        assert!(
            !graph.entities().any(|e| e.id == "ghost:adder"),
            "the call fell through to a ghost, which is the defect this exemption exists for"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No consumer should have to guard against an edge naming an entity the
    /// payload does not hold.
    #[test]
    fn the_cut_leaves_no_edge_pointing_at_a_dropped_name() {
        let dir = assignments_fixture("dangling");
        let result = analyze_fixture(&dir, false);

        let ids: HashSet<&str> = result.entities.iter().map(|e| e.id.as_str()).collect();
        let dangling: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| {
                // Unresolved call targets are names, not ids, and are the
                // graph builder's business (it ghosts them). Only edges that
                // *had* an entity and lost it are this pass's fault.
                r.source_id.contains(".py:") && !ids.contains(r.source_id.as_str())
                    || r.target_id.contains(".py:") && !ids.contains(r.target_id.as_str())
            })
            .map(|r| (r.source_id.clone(), r.target_id.clone()))
            .collect();
        assert!(
            dangling.is_empty(),
            "edges left pointing nowhere: {dangling:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The flag is the whole escape hatch: a focused reading gets everything
    /// back, without the parse store having to know which mode it is in.
    #[test]
    fn include_locals_restores_every_assignment() {
        let dir = assignments_fixture("restores");
        let result = analyze_fixture(&dir, true);

        assert!(
            !named(&result, "MAX_RETRIES").is_empty(),
            "constant missing"
        );
        assert!(!named(&result, "step").is_empty(), "local missing");
        assert!(!named(&result, "adder").is_empty(), "alias missing");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PY-028 end-to-end: a subscripted Python annotation must reach the
    /// class inside it. Before the Python delimiter arm, `Optional[User]`
    /// survived the split as one token, matched nothing, and became a
    /// ghost — so the real `User` class was never reached and typed Python
    /// lost a large share of its `Returns` edges.
    #[test]
    fn python_subscripted_return_type_reaches_the_real_class() {
        use crate::models::RelationshipKind;

        let dir = std::env::temp_dir().join(format!("mezz-py028-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("svc.py"),
            "from typing import Optional\n\nclass User:\n    pass\n\nclass Repo:\n    def find(self, key: str) -> Optional[User]:\n        return None\n\n    def label(self) -> str:\n        return \"x\"\n",
        )
        .expect("fixture");

        let config = Config::for_path(&dir);
        let mut analyzer = Analyzer::new(config);
        let result = analyzer.analyze().expect("analysis should succeed");

        let user_id = result
            .entities
            .iter()
            .find(|e| e.name == "User")
            .map(|e| e.id.clone())
            .expect("User class entity");
        let returns: Vec<&str> = result
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Returns)
            .map(|r| r.target_id.as_str())
            .collect();

        assert!(
            returns.contains(&user_id.as_str()),
            "no Returns edge onto the real User class: {:?}",
            returns
        );
        // The primitive edge is the regression this fix must not trade away.
        assert!(
            returns.contains(&"str"),
            "`-> str` lost its Returns edge: {:?}",
            returns
        );
        assert!(
            !returns.iter().any(|t| t.contains('[')),
            "annotation syntax leaked into a target name: {:?}",
            returns
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// KT-003 / DA-001 end-to-end, on the two tickets' own fixtures: a
    /// nullable return type must reach the class it makes nullable. Before
    /// the Kotlin/Dart arm, `?` was not a delimiter the splitter knew, so
    /// `User?` survived as one token and became a ghost of that literal
    /// name — standing beside the real `User`, which was never reached.
    ///
    /// Table-driven across both languages because the defect, the fixture
    /// and the assertion are the same one twice; only the syntax differs.
    #[test]
    fn a_nullable_return_type_reaches_the_real_class() {
        use crate::models::RelationshipKind;

        for (label, file, source, class) in [
            (
                "kt003",
                "Svc.kt",
                "data class User(val id: String)\ninterface Repo { suspend fun get(id: String): User? }\n",
                "User",
            ),
            (
                "da001",
                "shop.dart",
                "class Order { final String id; Order(this.id); }\nabstract class Repo { Future<Order?> find(String id); }\n",
                "Order",
            ),
        ] {
            let dir = std::env::temp_dir().join(format!("mezz-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            std::fs::write(dir.join(file), source).expect("fixture");

            let config = Config::for_path(&dir);
            let mut analyzer = Analyzer::new(config);
            let result = analyzer.analyze().expect("analysis should succeed");

            let class_id = result
                .entities
                .iter()
                .find(|e| e.name == class)
                .map(|e| e.id.clone())
                .unwrap_or_else(|| panic!("{label}: no {class} entity"));
            let returns: Vec<&str> = result
                .relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::Returns)
                .map(|r| r.target_id.as_str())
                .collect();

            assert!(
                returns.contains(&class_id.as_str()),
                "{label}: no Returns edge onto the real {class} class: {returns:?}"
            );
            // The ghost the tickets open with is minted from an unresolved
            // target, so no target may still carry the marker.
            assert!(
                !returns.iter().any(|t| t.contains('?')),
                "{label}: nullability syntax leaked into a target name: {returns:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// AN-003: a parse-store hit must reconstruct byte-for-byte what a
    /// cold parse produced. Parse a real source file with an empty store
    /// (miss → persist), then again with the same store (hit → reload),
    /// and assert the two `ParsedFile`s serialize identically. This is the
    /// determinism argument (AN-002) applied to the cache: identical
    /// content + parser ⇒ identical entities, so the hit is safe.
    #[test]
    fn parse_store_hit_matches_cold_parse() {
        use crate::parser;
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/analyzer/mod.rs");
        let cache = std::env::temp_dir().join(format!("mezz-an003-hit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let store = ParseStore::open_at(cache.clone());
        let lang = parser::detect_language(&file);

        let cold = Analyzer::parse_file_standalone(&file, lang, &store)
            .expect("cold parse should succeed");
        assert_eq!(store.stats(), (0, 1), "first parse must be a miss");

        let warm = Analyzer::parse_file_standalone(&file, lang, &store)
            .expect("warm parse should succeed");
        assert_eq!(store.stats(), (1, 1), "second parse must be a hit");

        // Compare as `Value` (object keys are BTreeMap-sorted) rather than as
        // a string: relationship `metadata` is a HashMap whose string
        // serialization order is nondeterministic, so two semantically equal
        // parses can stringify differently. The store's on-disk bytes are
        // identical; what must match is the reconstructed structure.
        assert_eq!(
            serde_json::to_value(&cold).unwrap(),
            serde_json::to_value(&warm).unwrap(),
            "store hit diverged from cold parse"
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    /// AN-014: the field report's case, end to end. A TypeScript getter
    /// declares `SessionItem[]`; the only `SessionItem` in the tree is a
    /// Rust struct in `backend/`. Locality ranking cannot separate them —
    /// something always wins — so before the language guard the graph
    /// reported a TypeScript getter depending on a Rust struct.
    ///
    /// Runs the whole pipeline rather than a resolver in isolation
    /// deliberately: two independent resolvers can bind a name
    /// (`graph::pick_nearest` and `DependencyResolver::find_entity_by_name_near`),
    /// and AN-011 showed that fixing one while missing the other looks
    /// exactly like a working fix.
    #[test]
    fn names_do_not_bind_across_a_language_boundary() {
        let dir = std::env::temp_dir().join(format!("mezz-an014-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("frontend")).unwrap();
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        std::fs::write(
            dir.join("frontend/vm.ts"),
            "export class ViewModel {\n  get bySide(): SessionItem[] { return []; }\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("backend/session.rs"),
            "pub struct SessionItem { pub id: String }\n",
        )
        .unwrap();

        let mut analyzer = Analyzer::new(Config::for_path(&dir));
        let result = analyzer.analyze().expect("analysis should succeed");
        let graph = crate::graph::DependencyGraph::from_analysis(&result);
        let ext_of: std::collections::HashMap<&str, String> = graph
            .entities()
            .map(|e| {
                (
                    e.id.as_str(),
                    e.file_path
                        .extension()
                        .and_then(|x| x.to_str())
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .collect();

        let crossings: Vec<String> = graph
            .relationships()
            .filter_map(|r| {
                let (s, t) = (
                    ext_of.get(r.source_id.as_str())?,
                    ext_of.get(r.target_id.as_str())?,
                );
                ((s == "ts" && t == "rs") || (s == "rs" && t == "ts"))
                    .then(|| format!("{} -> {}", r.source_id, r.target_id))
            })
            .collect();

        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            crossings.is_empty(),
            "TypeScript and Rust bound to each other: {crossings:?}"
        );
    }

    /// Pre-flipping the cancel flag must abort `analyze_with_cancel`
    /// before any real work runs, surfacing as a `Cancelled` error
    /// downcastable on the returned `anyhow::Error`. The handler in
    /// `server/analysis_handler.rs` relies on this branch to skip
    /// JSON writes + graph swaps when a re-scope races a running
    /// analysis.
    #[test]
    fn analyze_with_cancel_returns_cancelled_when_preflagged() {
        let config = Config::for_path(env!("CARGO_MANIFEST_DIR"));
        let mut analyzer = Analyzer::new(config);
        let cancel = Arc::new(AtomicBool::new(true));

        let err = analyzer
            .analyze_with_cancel(&cancel)
            .expect_err("preset flag should abort");

        assert!(err.is::<Cancelled>(), "expected Cancelled, got: {:?}", err);
    }

    /// Sanity check: with a fresh flag, the cancellable path runs to
    /// completion. Asserts the wrapper doesn't accidentally short-
    /// circuit when the flag is unset.
    #[test]
    fn analyze_with_cancel_clean_flag_runs() {
        let config = Config::for_path(env!("CARGO_MANIFEST_DIR"));
        let mut analyzer = Analyzer::new(config);
        let cancel = Arc::new(AtomicBool::new(false));

        let result = analyzer.analyze_with_cancel(&cancel);
        assert!(
            result.is_ok(),
            "fresh flag must not abort: {:?}",
            result.err()
        );
    }

    /// AN-012 end to end: a call whose receiver walks a field of a struct
    /// declared in *another* file must reach the right entity.
    ///
    /// This is the half neither the parser tests nor `receiver_index`'s unit
    /// tests can prove on their own — it needs two files in one analysis, and
    /// it is the shape `impact` was under-reporting: `entity.kind.is_callable()`
    /// where `CodeEntity` lives in one file and the call site in another.
    #[test]
    fn a_field_type_from_another_file_resolves_the_call() {
        let dir = std::env::temp_dir().join(format!("mezz-an012-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        std::fs::write(
            dir.join("models.rs"),
            "pub struct CodeEntity {\n    pub kind: EntityKind,\n}\n\
             pub enum EntityKind { Function }\n\
             impl EntityKind {\n    pub fn is_callable(&self) -> bool { true }\n}\n",
        )
        .expect("write models");
        std::fs::write(
            dir.join("parser.rs"),
            "pub fn count(entity: &CodeEntity) -> u32 {\n\
             \x20   if entity.kind.is_callable() { 1 } else { 0 }\n}\n\
             pub fn absent(entity: &Mystery) -> u32 {\n\
             \x20   if entity.kind.is_callable() { 1 } else { 0 }\n}\n",
        )
        .expect("write parser");

        let mut analyzer = Analyzer::new(Config::for_path(&dir));
        let result = analyzer.analyze().expect("analysis should succeed");
        let graph = crate::graph::DependencyGraph::from_analysis(&result);

        let target_of = |caller: &str| -> CodeEntity {
            let source = result
                .entities
                .iter()
                .find(|e| e.name == caller)
                .unwrap_or_else(|| panic!("no `{caller}` entity"));
            let (target, _) = graph
                .dependencies(&source.id)
                .into_iter()
                .find(|(_, r)| r.kind == RelationshipKind::Calls)
                .unwrap_or_else(|| panic!("no call edge out of `{caller}`"));
            target.clone()
        };

        // Resolving case: `entity` is a `CodeEntity`, whose `kind` field is
        // declared in the other file as an `EntityKind`. The edge must land on
        // the real method, not on a ghost that merely reads like it.
        let resolved = target_of("count");
        assert!(
            !resolved.tags.contains("ghost") && resolved.file_path.ends_with("models.rs"),
            "cross-file field type did not resolve the call: {} ({:?})",
            resolved.qualified_name,
            resolved.file_path
        );
        assert_eq!(resolved.name, "is_callable");
        // Non-resolving case: same call shape, but nothing declares `Mystery`,
        // so the edge stays on a ghost rather than being guessed onto the one
        // `is_callable` in the tree.
        let unresolved = target_of("absent");
        assert!(unresolved.tags.contains("ghost"), "{unresolved:?}");
        assert_eq!(
            unresolved.qualified_name, "entity.kind::is_callable",
            "an unresolvable receiver must not be guessed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --------------------------------------------------------------
    //  An imported constant is a dependency (AN-028)
    // --------------------------------------------------------------

    /// Two files and one constant: the declaring module, and a function in
    /// another file that imports the constant and reads it.
    fn constant_fixture(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mezz-an028-{}-{}-{}",
            name,
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for (file, body) in files {
            std::fs::write(src.join(file), body).unwrap();
        }
        root
    }

    /// The reader's edge onto the constant, or a panic naming what was found
    /// instead. Read off the built graph rather than the raw relationships,
    /// so the assertion covers the name resolving to the declaring file and
    /// not merely an edge being recorded.
    fn value_edge_target(result: &AnalysisResult, reader: &str) -> CodeEntity {
        let graph = crate::graph::DependencyGraph::from_analysis(result);
        let source = result
            .entities
            .iter()
            .find(|e| e.name == reader)
            .unwrap_or_else(|| panic!("no `{reader}` entity in {:?}", result.entities));
        let (target, _) = graph
            .dependencies(&source.id)
            .into_iter()
            .find(|(_, r)| r.kind == RelationshipKind::UsesValue)
            .unwrap_or_else(|| {
                panic!(
                    "no UsesValue edge out of `{reader}`: {:?}",
                    graph.dependencies(&source.id)
                )
            });
        target.clone()
    }

    /// The ticket's TypeScript fixture. Two constants, one reader, and the
    /// default analysis — `include_locals` off, so this also covers the
    /// constant surviving `drop_local_assignments` on the strength of the
    /// edge alone.
    #[test]
    fn a_typescript_constant_read_across_files_is_a_dependency() {
        let root = constant_fixture(
            "ts",
            &[
                (
                    "settings.ts",
                    "export const DEFAULTS = { level: 1 };\nexport const LIMIT = 5;\n",
                ),
                (
                    "ui.ts",
                    "import { DEFAULTS, LIMIT } from './settings';\n\
                     export function show(): number { return DEFAULTS.level + LIMIT; }\n",
                ),
            ],
        );
        let result = analyse(&root);
        let target = value_edge_target(&result, "show");

        assert!(!target.tags.contains("ghost"), "{target:?}");
        assert_eq!(target.name, "LIMIT");
        assert!(
            target.file_path.ends_with("settings.ts"),
            "the edge must land on the declaring file, not {:?}",
            target.file_path
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same shape in Rust, which reaches the constant through `use`
    /// rather than through a relative specifier.
    #[test]
    fn a_rust_constant_read_across_files_is_a_dependency() {
        let root = constant_fixture(
            "rs",
            &[
                ("vocab.rs", "pub const LIMIT: u32 = 5;\n"),
                (
                    "user.rs",
                    "use crate::vocab::LIMIT;\n\
                     pub fn show() -> u32 { LIMIT + 1 }\n",
                ),
                ("main.rs", "mod user;\nmod vocab;\nfn main() {}\n"),
            ],
        );
        let result = analyse(&root);
        let target = value_edge_target(&result, "show");

        assert!(!target.tags.contains("ghost"), "{target:?}");
        assert_eq!(target.name, "LIMIT");
        assert!(
            target.file_path.ends_with("vocab.rs"),
            "the edge must land on the declaring file, not {:?}",
            target.file_path
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A name the reader also declares is not an import it reads. Without
    /// this cut the shadowing parameter below bound to `repo.rs`, drawing a
    /// dependency the body never has.
    #[test]
    fn a_shadowed_import_name_is_not_read_from_the_module_it_came_from() {
        let root = constant_fixture(
            "shadow",
            &[
                ("repo.rs", "pub fn clone_dir() -> u32 { 1 }\n"),
                (
                    "jobs.rs",
                    "use crate::repo::clone_dir;\n\
                     pub fn sanitize(clone_dir: u32) -> u32 { clone_dir + 1 }\n",
                ),
                ("main.rs", "mod jobs;\nmod repo;\nfn main() {}\n"),
            ],
        );
        let result = analyse(&root);
        let graph = crate::graph::DependencyGraph::from_analysis(&result);
        let source = result
            .entities
            .iter()
            .find(|e| e.name == "sanitize")
            .expect("no `sanitize` entity");

        assert!(
            !graph
                .dependencies(&source.id)
                .iter()
                .any(|(_, r)| r.kind == RelationshipKind::UsesValue),
            "a shadowed name must not be read as the import it hides"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // --------------------------------------------------------------
    //  A module reached only through an enum variant (AN-029)
    // --------------------------------------------------------------

    /// The ticket's Rust fixture: a vocabulary module whose only reader
    /// names it through a variant, in no signature and no call. ADR 0021
    /// dropped `Type::variant` paths because "`UsesType` already carries"
    /// the owner; here nothing does, so before ADR 0028 the only edges into
    /// `vocab.rs` were its own `contains`.
    #[test]
    fn a_variant_read_through_an_imported_type_is_a_dependency() {
        let root = constant_fixture(
            "variant",
            &[
                ("vocab.rs", "pub enum Kind { A, B }\n"),
                (
                    "user.rs",
                    "use crate::vocab::Kind;\n\
                     pub fn pick() -> u32 { match Kind::A { Kind::A => 1, Kind::B => 2 } }\n",
                ),
                ("main.rs", "mod user;\nmod vocab;\nfn main() {}\n"),
            ],
        );
        let result = analyse(&root);
        let target = value_edge_target(&result, "pick");

        assert!(!target.tags.contains("ghost"), "{target:?}");
        assert_eq!(target.name, "Kind");
        assert!(
            target.file_path.ends_with("vocab.rs"),
            "the edge must land on the declaring file, not {:?}",
            target.file_path
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other half of the CamelCase cut. `vocab::LIMIT` reaches the
    /// constant through the *module*, whose only entity is the `mod vocab;`
    /// line in `main.rs` — so admitting a lowercase owner would draw
    /// `user.rs -> main.rs`, a file the reader does not depend on.
    #[test]
    fn a_constant_reached_through_a_module_path_draws_no_edge_to_the_mod_line() {
        let root = constant_fixture(
            "modpath",
            &[
                ("vocab.rs", "pub const LIMIT: u32 = 5;\n"),
                (
                    "user.rs",
                    "use crate::vocab;\n\
                     pub fn show() -> u32 { vocab::LIMIT + 1 }\n",
                ),
                ("main.rs", "mod user;\nmod vocab;\nfn main() {}\n"),
            ],
        );
        let result = analyse(&root);
        let graph = crate::graph::DependencyGraph::from_analysis(&result);
        let source = result
            .entities
            .iter()
            .find(|e| e.name == "show")
            .expect("no `show` entity");

        assert!(
            !graph
                .dependencies(&source.id)
                .iter()
                .any(|(t, r)| r.kind == RelationshipKind::UsesValue
                    && t.file_path.ends_with("main.rs")),
            "a module-qualified read must not land on the `mod` line"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

