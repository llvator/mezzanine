//! Code analysis and dependency resolution.

mod file_walker;
mod dependency_resolver;
mod parse_store;
mod lsp_tracer;
mod receiver_index;
pub mod sql_fold;

pub use file_walker::{is_test_path, FileWalker};
pub use dependency_resolver::DependencyResolver;
pub use parse_store::ParseStore;

use crate::config::Config;
use crate::models::{CodeEntity, EntityKind, Precision, Relationship, RelationshipKind, FileInfo, Position, Span};
use crate::models::file_info::Language;
use crate::parser::{self};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    fn discover_files(&self, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>> {
        let walker = FileWalker::new(&self.config);
        walker.walk_with_cancel(&self.config.root_path, cancel)
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

        let languages = &self.config.analysis.languages;
        let results: Vec<_> = files
            .par_iter()
            .filter_map(|file_path| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let language = parser::detect_language(file_path);
                if !languages.is_empty() && !languages.contains(&language) {
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
        eprintln!("  Parse store: {} hits, {} misses (re-parsed {} changed/new files)", hits, misses, misses);
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
            self.files.insert(parsed.file_info.path.clone(), parsed.file_info);
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
    /// Opt-in (`NAO_LSP_EXACT=1`): default off so it never re-costs the
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
    /// Gated on `config.analysis.allow_unsafe_passes`, which `nao serve`
    /// clears: resolution drives `cargo check`, which executes the analyzed
    /// repo's `build.rs` and proc-macros. That's fine for a tree the operator
    /// chose and unacceptable for one a visitor pasted a URL for. The check
    /// lives at the config layer, so it holds regardless of what
    /// `NAO_LSP_EXACT` says in the environment.
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
        let resolver = DependencyResolver::new(&self.config, &self.entities, &self.imports);
        let dep_relationships = resolver.resolve()?;
        self.relationships.extend(dep_relationships);

        self.validate_elevator_imports();
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
            let Some(kind) = elevator_kind_from_id(&rel.target_id) else {
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
            let qualname = elevator_qualname_from_id(&id).unwrap_or_else(|| id.clone());
            let leaf = qualname.rsplit('.').next().unwrap_or(&qualname).to_string();
            let span = Span::new(Position::new(0, 0, 0), Position::new(0, 0, 0));
            let mut entity = CodeEntity::new(leaf, kind, file_path, span);
            entity.id = id.clone();
            entity.qualified_name = qualname;
            entity.tags.insert("elevator".to_string());
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
            (&a.file_path, a.span.start.offset, &a.id)
                .cmp(&(&b.file_path, b.span.start.offset, &b.id))
        });
        let mut relationships = self.relationships.clone();
        relationships.sort_by(|a, b| {
            (&a.source_id, &a.target_id, a.kind.display_label(), &a.label)
                .cmp(&(&b.source_id, &b.target_id, b.kind.display_label(), &b.label))
        });
        let mut files: Vec<FileInfo> = self.files.values().cloned().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        AnalysisResult {
            entities,
            relationships,
            files,
            warnings: self.warnings.clone(),
        }
    }

    /// Parse a single file without requiring &mut self (suitable for
    /// parallel execution). Consults the AN-003 parse store first: on a
    /// content-hash hit it returns the stored `ParsedFile` verbatim,
    /// skipping tree-sitter entirely; on a miss it parses and persists.
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

        if let Some(parsed) = store.get(&abs_path, &content_hash) {
            return Ok(parsed);
        }

        let result = parser::parse_content(path, &content, language)?;

        let file_info = FileInfo {
            path: path.to_path_buf(),
            language,
            size: content.len() as u64,
            line_count: content.lines().count(),
            content_hash: Some(content_hash.clone()),
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
            .map(|e| (e.id.clone(), e.file_path.clone(), e.span, e.parameters.clone()))
            .collect();

        for (func_id, file_path, span, params) in callables {
            for param in &params {
                // Skip `self` / `&self` / `&mut self`
                if param.name == "self" || param.name == "&self" || param.name == "&mut self" {
                    continue;
                }

                let display_name = match &param.type_name {
                    Some(t) => format!("{}: {}", param.name, t),
                    None => param.name.clone(),
                };

                let param_id = format!("{}::param::{}", func_id, param.name);
                let mut entity = CodeEntity::new(
                    display_name,
                    EntityKind::Parameter,
                    file_path.clone(),
                    span,
                );
                entity.id = param_id.clone();
                entity.qualified_name = entity.name.clone();
                entity.parent_id = Some(func_id.clone());

                self.entities.insert(param_id.clone(), entity);

                let rel = Relationship::new(
                    param_id,
                    func_id.clone(),
                    RelationshipKind::TakesParam,
                );
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
                if field.name.is_empty()
                    || field.name.contains('[')
                    || field.name.contains('.')
                {
                    continue;
                }

                let display_name = match &field.type_name {
                    Some(t) => format!("{}: {}", field.name, t),
                    None => field.name.clone(),
                };

                let field_id = format!("{}::field::{}", owner_id, field.name);
                // De-dupe across multiple synthesis paths (e.g. same file
                // analyzed twice on watch reload).
                if self.entities.contains_key(&field_id) {
                    continue;
                }

                let mut entity = CodeEntity::new(
                    display_name,
                    EntityKind::Variable,
                    file_path.clone(),
                    span,
                );
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
                let entity_id =
                    format!("{}::branch::{}", caller_id, accumulated);
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
                    entity.qualified_name =
                        format!("{}::{}", caller_id, accumulated);
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
        let filters = &self.config.filters;
        
        // Filter entities by kind
        if !filters.entity_kinds.is_empty() {
            self.entities.retain(|_, e| filters.entity_kinds.contains(&e.kind));
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
    
    /// Analyze a specific file
    pub fn analyze_file(&mut self, path: &Path) -> Result<AnalysisResult> {
        let language = parser::detect_language(path);
        let store = ParseStore::open();
        let parsed = Self::parse_file_standalone(path, language, &store)?;
        self.files.insert(parsed.file_info.path.clone(), parsed.file_info);
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

        let file_entities: Vec<CodeEntity> = self
            .file_entities
            .get(path)
            .cloned()
            .unwrap_or_default();
        
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

/// Result of code analysis.
///
/// `Serialize`/`Deserialize` exist so `nao serve` can snapshot a finished
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
    
    /// Any warnings generated during analysis
    pub warnings: Vec<String>,
}

impl AnalysisResult {
    /// Get entities of a specific kind
    pub fn entities_of_kind(&self, kind: crate::models::EntityKind) -> Vec<&CodeEntity> {
        self.entities.iter().filter(|e| e.kind == kind).collect()
    }
    
    /// Get relationships of a specific kind
    pub fn relationships_of_kind(&self, kind: crate::models::RelationshipKind) -> Vec<&Relationship> {
        self.relationships.iter().filter(|r| r.kind == kind).collect()
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

            let ids: Vec<&str> = result.entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>();
            let rels: Vec<(String, String, &'static str)> = result
                .relationships
                .iter()
                .map(|r| (r.source_id.clone(), r.target_id.clone(), r.kind.display_label()))
                .collect();
            let fans: Vec<(String, u32, u32, usize)> = graph
                .entities()
                .map(|e| (e.id.clone(), e.metrics.fan_in, e.metrics.fan_out, e.metrics.smells.len()))
                .collect();
            (ids.iter().map(|s| s.to_string()).collect::<Vec<_>>(), rels, fans)
        };

        assert_eq!(run(), run(), "two runs on an identical tree diverged");
    }

    /// PY-028 end-to-end: a subscripted Python annotation must reach the
    /// class inside it. Before the Python delimiter arm, `Optional[User]`
    /// survived the split as one token, matched nothing, and became a
    /// ghost — so the real `User` class was never reached and typed Python
    /// lost a large share of its `Returns` edges.
    #[test]
    fn python_subscripted_return_type_reaches_the_real_class() {
        use crate::models::RelationshipKind;

        let dir = std::env::temp_dir().join(format!("nao-py028-{}", std::process::id()));
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
        let cache = std::env::temp_dir()
            .join(format!("nao-an003-hit-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("nao-an014-{}", std::process::id()));
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
                let (s, t) = (ext_of.get(r.source_id.as_str())?, ext_of.get(r.target_id.as_str())?);
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

        assert!(
            err.is::<Cancelled>(),
            "expected Cancelled, got: {:?}",
            err
        );
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
        let dir = std::env::temp_dir().join(format!("nao-an012-{}", std::process::id()));
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
}
