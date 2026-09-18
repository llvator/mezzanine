//! Graph representation and manipulation using petgraph.

use crate::analyzer::AnalysisResult;
use crate::models::{
    CodeEntity, EntityKind, FileMetrics, FolderPicture, ImportSite, FolderMetrics, Relationship,
    RelationshipKind, ScopeMetrics,
};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::{HashMap, HashSet};

/// Entities the parser mints to describe structure rather than to name a
/// declaration: a parameter, the arm of a conditional, a loop body, a local
/// binding. They carry real edges and belong in the file map; they are not
/// things a reader counts.
fn is_synthetic(e: &CodeEntity) -> bool {
    matches!(
        e.kind,
        EntityKind::Parameter | EntityKind::Branch | EntityKind::Loop
    ) || e.tags.contains("local_var")
}

/// Which immediate child of `folder` holds `path`, or `None` when `path` is
/// outside the folder entirely.
///
/// The unit a folder is drawn in: a file directly inside it is its own node, a
/// file deeper down is the subfolder standing for it, and two paths answering
/// the same child are one node as far as this folder's picture is concerned.
/// Missing imports counted per folder, worst first — the body of
/// [`DependencyGraph::unresolved_imports_by_folder`], taken as an argument so
/// the coverage pair and this ranking can come out of one graph scan.
fn folders_of(missing: &std::collections::BTreeSet<(String, String)>) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for (from, to) in missing {
        let (a, b) = (folder_of(from), folder_of(to));
        *counts.entry(a).or_insert(0) += 1;
        if b != a {
            *counts.entry(b).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(folder, n)| (folder.to_string(), n))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// The folder a file sits in — `.` for one at the analysis root.
fn folder_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

fn immediate_child<'a>(folder: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(folder)?.strip_prefix('/')?;
    let end = rest.find('/').unwrap_or(rest.len());
    Some(&path[..folder.len() + 1 + end])
}

/// Wrapper around petgraph for dependency visualization.
#[derive(Clone)]
pub struct DependencyGraph {
    /// The underlying directed graph
    graph: DiGraph<CodeEntity, Relationship>,
    /// Map from entity ID to node index
    node_map: HashMap<String, NodeIndex>,
    /// Map from node index to entity ID
    reverse_map: HashMap<NodeIndex, String>,
    /// Per-file rollup metrics keyed by full file path (as seen on entities).
    /// Renderers strip the project root for display.
    file_metrics: Vec<FileMetrics>,
    /// Per-module (directory) rollup metrics keyed by full directory path.
    folder_metrics: Vec<FolderMetrics>,
    /// What each file says it is for, keyed the same way as `file_metrics`.
    /// Prose rather than a rollup, so it sits beside the metrics instead of
    /// inside them, and only files that carry a header appear.
    file_docs: HashMap<String, String>,
    /// Where each cross-file import was written (AN-024), carried through
    /// from the analysis rather than derived here.
    ///
    /// Beside the graph for the reason [`ImportSite`] gives: an import
    /// resolves to a file, and files are only nodes for a Groovy script —
    /// so the resolver's `Imports` edges exist for that one case and no
    /// other. Sorted, so anything printed from it is stable across runs.
    import_sites: Vec<ImportSite>,
}

/// Per-file entity counts and composite scores collected during a single
/// pass over the graph's nodes. Intermediate data for scope-metric assembly.
struct FileTally {
    /// Maps each non-parameter entity to its normalised file path.
    entity_file: HashMap<NodeIndex, String>,
    entity_count: HashMap<String, u32>,
    callable_count: HashMap<String, u32>,
    container_count: HashMap<String, u32>,
    loc: HashMap<String, u32>,
    /// Composite scores and LOC of every entity per file (for quality rollup).
    /// Each entry is `(composite_score, loc)` so we can compute LOC-weighted averages.
    scores: HashMap<String, Vec<(f32, u32)>>,
    /// Files holding nothing but module declarations — a `mod.rs` that names
    /// its siblings and says nothing else. Folder shape draws these as the
    /// folder rather than as a child of it.
    declaration_only: HashSet<String>,
}

/// Per-file tallies for one *family* of edges. The shape is the same
/// whichever family it holds, so the rollup helpers below take a bucket
/// rather than the whole of [`FileEdgeData`] and run over either.
#[derive(Default)]
struct EdgeBuckets {
    /// Number of edges where source and target share a file.
    internal: HashMap<String, u32>,
    /// Distinct other files pointing at each file (incoming cross-file).
    fan_in: HashMap<String, HashSet<String>>,
    /// Distinct other files each file points at (outgoing cross-file).
    fan_out: HashMap<String, HashSet<String>>,
    /// All cross-file (source_file, target_file) pairs for SCC + module rollup.
    pairs: Vec<(String, String)>,
}

impl EdgeBuckets {
    /// File one edge into whichever tally its endpoints call for.
    fn record(&mut self, sf: &str, tf: &str) {
        if sf == tf {
            *self.internal.entry(sf.to_string()).or_insert(0) += 1;
            return;
        }
        self.fan_out
            .entry(sf.to_string())
            .or_default()
            .insert(tf.to_string());
        self.fan_in
            .entry(tf.to_string())
            .or_default()
            .insert(sf.to_string());
        self.pairs.push((sf.to_string(), tf.to_string()));
    }

    fn fan_in_of(&self, path: &str) -> u32 {
        self.fan_in.get(path).map(|s| s.len() as u32).unwrap_or(0)
    }

    fn fan_out_of(&self, path: &str) -> u32 {
        self.fan_out.get(path).map(|s| s.len() as u32).unwrap_or(0)
    }
}

/// The file-level dependency graph — what a folder's drawing is made of.
///
/// The same four inputs every folder score is computed from, handed out
/// whole so a caller asking a question about the tree asks it over the
/// edges the shape scores were drawn over rather than over a second
/// derivation that agrees most of the time.
pub struct FileGraph {
    /// Every analysed file, by the one path spelling this graph uses.
    pub files: Vec<String>,
    /// Every cross-file dependency edge, as `(source file, target file)`.
    /// Duplicates are expected — several entity edges between one pair of
    /// files are several entries — and every consumer dedupes.
    pub pairs: Vec<(String, String)>,
    /// Every folder holding an analysed file, up to their common root.
    pub folders: HashSet<String>,
    /// Files holding nothing but module declarations — the folder
    /// speaking rather than a child of it (ADR 0022).
    pub declaration_only: HashSet<String>,
    /// Every cross-file import statement, in the same path spelling. The
    /// only part of the model that knows the build erases an edge, and
    /// what lets a folder's scores be taken over the graph that ships
    /// (ADR 0026).
    pub imports: Vec<ImportSite>,
}

/// Per-file edge data, split by what the edges *mean*.
///
/// UI-091. Coupling is measured over dependency edges, and a scope whose
/// edges are all `References` — a Markdown link, an Elevator or Impex
/// reference, a folded SQL foreign key — used to report a fan-out of `0`
/// while the canvas drew arrows leaving it. Tallying references alongside
/// (never inside) the dependency counts lets the reader be told the
/// difference between "measured, and it is zero" and "not measured here".
struct FileEdgeData {
    /// Edges that imply a dependency (`RelationshipKind::is_dependency`).
    /// Every existing metric is computed from this bucket alone.
    deps: EdgeBuckets,
    /// `References` edges. Reported separately; they feed no ratio, no
    /// cycle and no composite score.
    refs: EdgeBuckets,
}

/// Raise `OverfullHead` when a callable holds more distinct names in view
/// than a reader can keep — parameters, locals and explicitly-received
/// fields, against the red line the same `Thresholds` draws for the metric.
///
/// A free function rather than another arm inside `detect_smells` because
/// the rule needs no cross-entity pre-pass, and because the smell it raises
/// is the only one here that says nothing about control flow: a body can
/// trip it at `cyclomatic 1`.
///
/// `None` for a callable the parser could not measure — a language whose
/// declaration kinds are not in `parser::working_set::BINDING_FIELDS`
/// reports parameters only, and inferring a smell from a floor known to be
/// low would flag the wrong bodies.
fn overfull_head(
    m: &crate::models::entity::EntityMetrics,
    t: &crate::models::Thresholds,
) -> Option<crate::models::SmellKind> {
    let ws = m.working_set? as f32;
    (ws > t.working_set.bad).then_some(crate::models::SmellKind::OverfullHead)
}

impl DependencyGraph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_map: HashMap::new(),
            reverse_map: HashMap::new(),
            file_metrics: Vec::new(),
            folder_metrics: Vec::new(),
            file_docs: HashMap::new(),
            import_sites: Vec::new(),
        }
    }

    /// Per-file quality rollups (empty until `from_analysis` runs).
    pub fn file_metrics(&self) -> &[FileMetrics] {
        &self.file_metrics
    }

    /// What a file says it is for — its `//!` header — or `None` when it
    /// says nothing. Accepts either spelling of the path (`./src/foo.rs`,
    /// `src/foo.rs`); both normalise to the `file_metrics` key.
    pub fn file_documentation(&self, path: &std::path::Path) -> Option<&str> {
        self.file_docs
            .get(&Self::normalize_path(path))
            .map(String::as_str)
    }

    /// Per-directory quality rollups (empty until `from_analysis` runs).
    pub fn folder_metrics(&self) -> &[FolderMetrics] {
        &self.folder_metrics
    }

    /// Where the analysis saw each cross-file import written (AN-024).
    /// Empty until `from_analysis` runs, and empty for a language whose
    /// imports name packages rather than paths.
    pub fn import_sites(&self) -> &[ImportSite] {
        &self.import_sites
    }

    /// How much of what the parsers read actually reached the graph.
    ///
    /// `(landed, seen)` over cross-file import statements: `seen` is every
    /// [`ImportSite`] a parser recorded — a statement whose specifier it
    /// resolved to a file — and `landed` is those with a dependency edge
    /// between the same two files to show for it.
    ///
    /// The gap is silent everywhere else. `resolve_import_target` drops an
    /// import it cannot bind to an entity without a word, so a folder whose
    /// incoming edges went missing is reported as quiet rather than
    /// unreadable, and every verdict computed over it — shape, doors, exits,
    /// a `check` rule — is confidently wrong with nothing attached to say so.
    /// A reader cannot tell "clean" from "did not see it", which is the one
    /// distinction that decides whether to trust the answer.
    ///
    /// Sites are deduped by `(from, to)` because the count is of dependencies
    /// that should be drawable, not of statements: three `use` lines between
    /// one pair of files are one edge in every consumer downstream.
    ///
    /// **Build-erased statements are left out of both halves.** A TypeScript
    /// `import type` is deleted by the compiler and no bundler resolves it
    /// (AN-022), and ADR 0026 already keeps those arrows out of every score.
    /// The sentence this number is printed under is about *verdicts* being
    /// unsound, and verdicts are computed over the scored graph — so counting
    /// an erased statement as a missing dependency would raise an alarm about
    /// something no verdict reads. Seven of `ui/src`'s 32 were of that kind.
    pub fn import_coverage(&self) -> (usize, usize) {
        let seen = self.import_dependencies();
        let missing = self.unresolved_imports();
        (seen.len().saturating_sub(missing.len()), seen.len())
    }

    /// Every cross-file dependency an import statement asks for, deduped and
    /// with the build-erased ones dropped. The denominator of
    /// [`Self::import_coverage`], and the set the unresolved ones come out of.
    fn import_dependencies(&self) -> std::collections::BTreeSet<(String, String)> {
        self.import_sites
            .iter()
            .filter(|site| !site.is_type_only)
            .map(|site| {
                (
                    site.from.display().to_string(),
                    site.to.display().to_string(),
                )
            })
            .collect()
    }

    /// The import statements the graph holds no dependency edge for.
    pub fn unresolved_imports(&self) -> std::collections::BTreeSet<(String, String)> {
        let pairs: std::collections::HashSet<(String, String)> =
            self.file_graph().pairs.into_iter().collect();
        self.import_dependencies()
            .into_iter()
            .filter(|pair| !pairs.contains(pair))
            .collect()
    }

    /// The unresolved imports that would have changed `folder`'s drawing, as
    /// the statements a reader can go and look at.
    ///
    /// The count alone was not enough. A missing import is what *creates* a
    /// child with no parent in the drawing, so `reshape` would print "these
    /// children have no parent" and offer three diagnoses of a folder whose
    /// parent exists in the source — a fabricated task, worded as
    /// confidently as a real one, with only a "treat this as provisional"
    /// banner above it. Provisional reads as "the number may be off", not
    /// "the instruction is invented".
    ///
    /// Naming them collapses that: a reader who sees
    /// `algorithms.ts:4 → nanoid.ts` knows immediately which finding is an
    /// artifact, without deriving it from the source.
    pub fn unresolved_imports_in(&self, folder: &str) -> Vec<&ImportSite> {
        let missing = self.unresolved_imports();
        let mut sites: Vec<&ImportSite> = self
            .import_sites
            .iter()
            .filter(|site| !site.is_type_only)
            .filter(|site| {
                let pair = (
                    site.from.display().to_string(),
                    site.to.display().to_string(),
                );
                missing.contains(&pair)
                    && immediate_child(folder, &pair.0) != immediate_child(folder, &pair.1)
            })
            .collect();
        sites.sort_by(|a, b| (&a.from, a.line, &a.to).cmp(&(&b.from, b.line, &b.to)));
        sites.dedup_by(|a, b| (&a.from, a.line, &a.to) == (&b.from, b.line, &b.to));
        sites
    }

    /// How many of them would have changed `folder`'s **drawing**.
    ///
    /// The count that belongs next to a verdict about that folder. A repo
    /// figure in the footer says the graph has holes somewhere; it cannot tell
    /// a reader whether the folder they are being given an answer about is one
    /// of the holed ones.
    ///
    /// "Would have changed the drawing", not "is somewhere beneath it". A
    /// folder is scored over its immediate children with each subfolder
    /// collapsed to one node (ADR 0012), so an unresolved import matters here
    /// only if it is an edge in *that* picture: between two different
    /// immediate children, or across the boundary. One between two files
    /// inside a single child is invisible at this level — it is that child's
    /// business — and the first version counted it anyway, by asking only
    /// whether an endpoint sat beneath the folder.
    ///
    /// Two symptoms, both reported from the field: `src/uid/generators`, whose
    /// three files hold no relative import at all, was marked as having two
    /// unresolved; and the analysis root was marked with *every* unresolved
    /// import in the project, since every path is beneath it. Both told a
    /// reader to distrust a verdict that was sound, and both sat in the
    /// "Start here" list, which is the first thing acted on.
    pub fn unresolved_imports_touching(&self, folder: &str) -> usize {
        self.unresolved_imports()
            .iter()
            .filter(|(from, to)| immediate_child(folder, from) != immediate_child(folder, to))
            .count()
    }

    /// Where the missing imports are, worst folder first.
    ///
    /// The footer that ships the count says "any verdict over the folders they
    /// cross is unsound" and then names no folder, which a field report
    /// (2026-08-27) could not spend:
    ///
    /// > I published `fan_out 38→34 (-4)` … as measured evidence that a
    /// > refactor reduced coupling — while holding, unresolved, the tool's own
    /// > statement that verdicts of that kind may be unsound over folders it
    /// > would not name. … A caveat that cannot be localised gets applied
    /// > either everywhere or nowhere, and both are wrong.
    ///
    /// Their guess was that the per-import detail is discarded before the
    /// footer renders. It is not — [`Self::unresolved_imports`] holds the
    /// pairs, and this only groups them.
    ///
    /// Attributed to **both** ends, so the counts overlap and do not sum to
    /// the total. A dropped edge understates the source folder's fan-out and
    /// the target's fan-in equally, and a reader scanning for their own folder
    /// has to find it whichever side of the arrow it sits on.
    pub fn unresolved_imports_by_folder(&self) -> Vec<(String, usize)> {
        folders_of(&self.unresolved_imports())
    }

    /// [`Self::import_coverage`] and [`Self::unresolved_imports_by_folder`]
    /// from the one expensive pass they share.
    ///
    /// The footer that wants both is appended to *every* MCP response, and
    /// `file_graph` is rebuilt from two scans and a clone of every import site
    /// on each call — so asking the two questions separately doubles the cost
    /// of every answer the server gives.
    pub fn import_coverage_by_folder(&self) -> (usize, usize, Vec<(String, usize)>) {
        let seen = self.import_dependencies().len();
        let missing = self.unresolved_imports();
        (seen.saturating_sub(missing.len()), seen, folders_of(&missing))
    }

    /// The graph one folder draws, with a verdict on every node and edge —
    /// the evidence behind the `shape` its [`FolderMetrics`] reports.
    ///
    /// Recomputed on demand from the same two scans `populate_scope_metrics`
    /// runs, rather than kept from that pass: the scalars are four floats a
    /// folder and free to hold, where a picture is the edge list again, and
    /// only one folder is ever being looked at. Feeding it the identical
    /// inputs is what makes it the picture the score was computed over
    /// instead of one that usually agrees.
    ///
    /// `None` for a path that is not an analysed folder.
    pub fn folder_picture(&self, folder: &str) -> Option<FolderPicture> {
        let fg = self.file_graph();
        crate::analyzer::folder_shape::picture(
            &Self::normalize_path(std::path::Path::new(folder)),
            fg.files.iter().map(String::as_str),
            &fg.pairs,
            &fg.imports,
            &fg.folders,
            &fg.declaration_only,
        )
    }

    /// The file-level dependency graph, as the folder scores see it.
    ///
    /// Recomputed on demand from the same two scans `populate_scope_metrics`
    /// runs. A caller wanting to count something over the tree — how many
    /// files import one file, how many doors a folder has — starts here, so
    /// its answer and the folder's score are claims about one graph.
    pub fn file_graph(&self) -> FileGraph {
        let tally = self.scan_entity_files();
        let edges = self.scan_file_edges(&tally.entity_file);
        let folders = Self::enumerate_folder_paths(&tally);
        FileGraph {
            files: tally.entity_count.keys().cloned().collect(),
            pairs: edges.deps.pairs,
            folders,
            declaration_only: tally.declaration_only,
            imports: self.import_sites.clone(),
        }
    }

    /// Build a graph from analysis results
    pub fn from_analysis(result: &AnalysisResult) -> Self {
        let mut graph = Self::new();

        // Add all entities as nodes
        for entity in &result.entities {
            graph.add_entity(entity.clone());
        }

        // Build lookup maps for resolving relationships.
        // All passes are O(E); per-edge resolution stays O(1).
        // Bare names collide constantly — 23 entities are called `parse` in
        // this repo — so every candidate is kept and ranked by locality at
        // lookup time (AN-011). Taking the first-registered one resolved every
        // per-parser test helper `fn parse` to whichever file happened to be
        // walked first.
        let mut name_to_ids: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        // Same treatment as `name_to_ids`: most parsers set `qualified_name`
        // to the bare name, so this map collides exactly as hard and answers
        // *before* the bare-name branch. Leaving it first-wins meant the
        // locality ranking below could never run for a plain `parse()`.
        let mut qualified_to_ids: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        // `TypeName::methodName` → method entity ID. Disambiguates common method
        // names like `new` when the callee was emitted qualified by the parser.
        let mut typed_method_to_id: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let id_to_entity: std::collections::HashMap<&str, &CodeEntity> =
            result.entities.iter().map(|e| (e.id.as_str(), e)).collect();

        for entity in &result.entities {
            name_to_ids
                .entry(entity.name.clone())
                .or_default()
                .push(entity.id.clone());
            qualified_to_ids
                .entry(entity.qualified_name.clone())
                .or_default()
                .push(entity.id.clone());
            if let Some(parent_id) = &entity.parent_id {
                // `parent_id` is usually a real entity ID, but the Rust parser
                // stores it as a bare type name for impl-block methods. Accept
                // both so `TypeName::methodName` keys get registered either way.
                let parent_name = id_to_entity
                    .get(parent_id.as_str())
                    .map(|p| p.name.as_str())
                    .unwrap_or(parent_id.as_str());
                // Register under both `::` (Rust) and `.` (Java) separators so
                // qualified callees from either parser resolve correctly.
                let rust_key = format!("{}::{}", parent_name, entity.name);
                typed_method_to_id
                    .entry(rust_key)
                    .or_insert_with(|| entity.id.clone());
                let java_key = format!("{}.{}", parent_name, entity.name);
                typed_method_to_id
                    .entry(java_key)
                    .or_insert_with(|| entity.id.clone());
            }
        }

        // Candidates were pushed in entity order; sort so the locality
        // tie-break below is a deterministic function of the tree (AN-002).
        for ids in name_to_ids.values_mut() {
            ids.sort();
        }
        for ids in qualified_to_ids.values_mut() {
            ids.sort();
        }

        // AN-006. Rust module paths: a free function in `src/diff.rs` is called
        // as `diff::resolve_git_ref`, but its `qualified_name` is the bare
        // `resolve_git_ref` and it has no parent entity — so neither
        // `qualified_to_id` nor `typed_method_to_id` ever holds that key, and
        // every path-qualified call to it lands on a ghost. Register
        // `<module>::<name>` for Rust entities, where the module is the file
        // stem (or the directory name for `mod.rs`).
        //
        // Ambiguity is ranked, not declined. Two `calls.rs` files under
        // different parser folders both claim `calls::extract_calls`, and this
        // pass used to store `None` for such a key and skip it — trading the
        // precision bug for a total loss of the edge. But the caller's own
        // position already decides it: `declarations/mod.rs` calling
        // `inference::collect_struct_fields` means the `inference.rs` beside it,
        // not the one in an unrelated prototype folder. Every claimant is kept
        // and `pick_nearest` chooses, which is the rule bare names have had
        // since AN-011; when nothing is nearer than anything else it still
        // answers, deterministically, rather than dropping the edge.
        //
        // AN-028 extends the same index to the web languages, where a value
        // read through `import { LIMIT } from './settings'` is recorded as
        // `settings::LIMIT`. The alternative there was a bare `LIMIT`, and
        // measured over `ui/` that bound 57 of 449 reads to a same-named
        // local in an unrelated file. Nothing already emitted can collide:
        // every other web-language target is bare or dot-qualified, and this
        // key is spelled with `::`.
        let mut module_qualified: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entity in &result.entities {
            let Some(stem) = entity.file_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(module) = module_segment(&entity.file_path, stem) else {
                continue;
            };
            let ids = module_qualified.entry(format!("{}::{}", module, entity.name));
            let ids = ids.or_default();
            if !ids.contains(&entity.id) {
                ids.push(entity.id.clone());
            }
        }
        // Same reason the maps above are sorted: candidates were pushed in
        // entity order, and every fallback below has to be a function of the
        // tree rather than of the walk (AN-002).
        for ids in module_qualified.values_mut() {
            ids.sort();
        }

        // `id_to_entity` covers the same key set as `graph.node_map` (every
        // entity was added as a node above), so checking it avoids a borrow
        // conflict with the later `graph.add_relationship` calls.
        // `from_file` is the calling entity's file, used to break bare-name
        // ties. `None` disables locality (used when resolving the edge's own
        // source, where there is no caller yet).
        let resolve = |raw: &str, from_file: Option<&std::path::Path>| -> Option<String> {
            if id_to_entity.contains_key(raw) {
                return Some(raw.to_string());
            }
            // AN-014: every lookup below is a *strategy*, not a verdict. A
            // strategy whose only answer is in an unrelated language declines
            // and the next one gets its turn; if they all decline the name
            // becomes a ghost, which is the honest outcome.
            let accept = |id: &String| accept_interoperable(id, from_file, &id_to_entity);
            if let Some(id) = typed_method_to_id.get(raw).and_then(accept) {
                return Some(id);
            }
            if let Some(id) = qualified_to_ids
                .get(raw)
                .and_then(|c| pick_nearest(c, from_file, &id_to_entity))
            {
                return Some(id);
            }
            // AN-006: `module::function`, and the `crate::`/`super::`-prefixed
            // forms of it, resolved against the file-stem index above. Several
            // modules may claim the name; the caller's position picks between
            // them when it is nearer to one, and declines when it is not.
            if let Some(id) = module_qualified
                .get(raw)
                .and_then(|c| pick_nearest_unambiguous(c, from_file, &id_to_entity))
            {
                return Some(id);
            }
            let segments: Vec<&str> = raw.split("::").collect();
            if segments.len() > 2 {
                let key = format!(
                    "{}::{}",
                    segments[segments.len() - 2],
                    segments[segments.len() - 1]
                );
                if let Some(id) = module_qualified
                    .get(&key)
                    .and_then(|c| pick_nearest_unambiguous(c, from_file, &id_to_entity))
                {
                    return Some(id);
                }
            }
            // Bare-name callees (`foo()`) fall back to `name_to_id`. Qualified
            // callees (`Type::method` or `Type.method`) that didn't hit
            // `typed_method_to_id` are almost certainly external (e.g.
            // `Pattern.compile` from the JDK, `Pattern::new` from a Rust
            // crate). Collapsing them onto an arbitrary project method with
            // the same last segment produces ghost edges, so we drop them.
            if !raw.contains("::") && !raw.contains('.') {
                if let Some(candidates) = name_to_ids.get(raw) {
                    return pick_nearest(candidates, from_file, &id_to_entity);
                }
            }
            None
        };

        // Track ghost entities to deduplicate (target_name → ghost_id)
        let mut ghosts: HashMap<String, String> = HashMap::new();

        for rel in &result.relationships {
            let source_id = match resolve(&rel.source_id, None) {
                Some(id) => id,
                None => continue, // If source is unresolved, skip entirely
            };
            // Resolve the target relative to the caller's file, so a bare name
            // binds to the nearest definition rather than an arbitrary one.
            let from_entity = id_to_entity.get(source_id.as_str()).copied();
            let from_file = from_entity.map(|e| e.file_path.as_path());
            let target_id = match resolve_callee(rel, from_entity, from_file, &resolve) {
                Some(id) => id,
                None => {
                    // Create a ghost entity for the unresolved target
                    let ghost_id = ghosts.entry(rel.target_id.clone()).or_insert_with(|| {
                        let gid = format!("ghost:{}", rel.target_id);
                        let name = rel
                            .target_id
                            .rsplit("::")
                            .next()
                            .unwrap_or(&rel.target_id)
                            .to_string();
                        // Force a type-ish kind for known built-in type
                        // names so `int`/`str`/`Vec` don't end up
                        // tagged as functions just because a Calls edge
                        // happened to be processed first. `ghost_type_kind`
                        // returns the language-appropriate label —
                        // Class for Python builtins, Struct for Rust
                        // primitives — so users see their idiom.
                        let kind = Self::ghost_type_kind(&rel.target_id)
                            .unwrap_or_else(|| Self::infer_ghost_kind(rel.kind));
                        let category = Self::ghost_category(&rel.target_id);

                        let mut entity = CodeEntity::new(
                            name,
                            kind,
                            std::path::PathBuf::from(""),
                            crate::models::Span::default(),
                        );
                        entity.id = gid.clone();
                        entity.qualified_name = rel.target_id.clone();
                        entity.tags.insert("ghost".to_string());
                        entity.tags.insert(category.to_string());
                        graph.add_entity(entity);
                        gid
                    });
                    ghost_id.clone()
                }
            };

            // Create a new relationship with resolved IDs, preserving metadata
            let mut resolved_rel =
                Relationship::new(&source_id, &target_id, rel.kind).with_weight(rel.weight);
            resolved_rel.metadata = rel.metadata.clone();
            if let Some(ref label) = rel.label {
                resolved_rel.label = Some(label.clone());
            }
            // Preserve the AN-004 precision label. Every call edge carries
            // one: the LSP tracer sets `Exact`; anything it didn't upgrade
            // defaults to `Heuristic` so no call edge renders unlabeled.
            resolved_rel.precision = match rel.precision {
                Some(p) => Some(p),
                None if rel.kind == RelationshipKind::Calls => {
                    Some(crate::models::Precision::Heuristic)
                }
                None => None,
            };

            graph.add_relationship(resolved_rel);
        }

        // Re-number call orders per source entity to eliminate gaps
        graph.renumber_call_orders();

        // Populate per-entity coupling metrics (fan-in, fan-out, cycle membership).
        // Single pass over edges + one SCC computation, both O(V + E).
        graph.populate_coupling_metrics();

        // Structural metrics that ride on the dependency graph: WMC per
        // container, longest outbound call-chain depth, and PageRank
        // centrality. Run after fan-in/out so they see final edge sets.
        graph.populate_wmc();
        graph.populate_chain_depth();
        graph.populate_pagerank();

        // Compute per-entity composite quality scores. Runs after coupling
        // metrics are finalised so fan-out and cycle membership are available.
        graph.populate_composite_scores();

        // Detect code smells (anti-pattern signals) from metric combinations.
        graph.detect_smells();

        // Import sites are file-level and join here rather than falling
        // out of the entity walk: no entity owns them. Before the rollups
        // below, which is not incidental — folder shape reads them to
        // decide which arrows the build erases (ADR 0026).
        graph.import_sites = result.import_sites.clone();

        // Scope-level rollups: per-file and per-module. Independent of the
        // entity-level pass so entity metrics are finalized before we
        // aggregate.
        graph.populate_scope_metrics();

        // File-level docs ride on `FileInfo` rather than on any entity, so
        // they are joined here instead of falling out of the entity walk.
        for file in &result.files {
            if let Some(doc) = &file.documentation {
                graph
                    .file_docs
                    .insert(Self::normalize_path(&file.path), doc.clone());
            }
        }

        graph
    }

    /// Populate per-entity fan-in, fan-out, and cycle membership. Counts only
    /// dependency edges (calls, imports, uses-type, etc.) so containment and
    /// inheritance don't inflate coupling signals. Runs once at build time —
    /// renderers read the precomputed values and never recompute.
    fn populate_coupling_metrics(&mut self) {
        use std::collections::{HashMap, HashSet};

        let mut fan_in: HashMap<NodeIndex, HashSet<NodeIndex>> = HashMap::new();
        let mut fan_out: HashMap<NodeIndex, HashSet<NodeIndex>> = HashMap::new();
        // Method count = number of callables contained by a given entity.
        // Contains edges already encode the struct → method hierarchy, so
        // this is a single O(E) pass.
        let mut method_count: HashMap<NodeIndex, u32> = HashMap::new();

        for edge_idx in self.graph.edge_indices() {
            let rel = &self.graph[edge_idx];
            if let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) {
                if rel.kind == RelationshipKind::Contains
                    && self.graph[tgt].kind.is_callable()
                    && src != tgt
                {
                    *method_count.entry(src).or_insert(0) += 1;
                }
            }
            if !rel.kind.is_dependency() {
                continue;
            }
            if let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) {
                if src == tgt {
                    continue;
                }
                fan_out.entry(src).or_default().insert(tgt);
                fan_in.entry(tgt).or_default().insert(src);
            }
        }

        // Cycle membership on the dependency subgraph — same source of truth
        // as `find_cycles`, so a node's `in_cycle` flag can never disagree
        // with the reported cycle list.
        let mut in_cycle: HashSet<NodeIndex> = HashSet::new();
        for scc in self.dependency_sccs() {
            in_cycle.extend(scc);
        }

        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        for idx in indices {
            let entity = &mut self.graph[idx];
            let fi = fan_in.get(&idx).map(|s| s.len() as u32).unwrap_or(0);
            let fo = fan_out.get(&idx).map(|s| s.len() as u32).unwrap_or(0);
            entity.metrics.fan_in = fi;
            entity.metrics.fan_out = fo;
            entity.metrics.instability = if fi + fo > 0 {
                Some(fo as f32 / (fi + fo) as f32)
            } else {
                None
            };
            entity.metrics.in_cycle = in_cycle.contains(&idx);
            entity.metrics.method_count = method_count.get(&idx).copied().unwrap_or(0);
        }
    }

    /// Weighted Methods per Class: for each container, sum the cyclomatic
    /// complexities of its directly-contained callables. Nothing else
    /// moves without coupling metrics first, so we run after those. O(E).
    fn populate_wmc(&mut self) {
        use crate::models::RelationshipKind;
        let mut sums: HashMap<NodeIndex, u32> = HashMap::new();
        for edge in self.graph.edge_references() {
            if !matches!(edge.weight().kind, RelationshipKind::Contains) {
                continue;
            }
            let container = edge.source();
            let child = edge.target();
            if let Some(cc) = self.graph[child].metrics.cyclomatic {
                *sums.entry(container).or_insert(0) += cc;
            }
        }
        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        for idx in indices {
            let entity = &mut self.graph[idx];
            if !entity.kind.is_container() {
                continue;
            }
            entity.metrics.wmc = Some(sums.get(&idx).copied().unwrap_or(0));
        }
    }

    /// Longest outbound call-chain depth per entity: the maximum length
    /// of any dependency path starting from the entity, counted in hops.
    ///
    /// Cycles are common in real code, so we can't rely on a topological
    /// DAG-path algorithm. Instead we do a bounded DFS with per-start
    /// visit tracking, capped at `MAX_DEPTH` hops. Memoised across starts
    /// only when the result we'd reuse wasn't computed inside a cycle —
    /// otherwise the memoised value would be wrong for a deeper start.
    fn populate_chain_depth(&mut self) {
        const MAX_DEPTH: u32 = 16;

        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        // For each node, compute the depth independently — a full memo
        // across starts requires topological order which isn't available
        // on a cyclic graph. This is O(V * (V + E)) worst case but in
        // practice the MAX_DEPTH cap keeps it bounded per start.
        for start in &indices {
            let mut visited: HashSet<NodeIndex> = HashSet::new();
            let depth = dfs_max_depth(&self.graph, *start, &mut visited, 0, MAX_DEPTH);
            self.graph[*start].metrics.chain_depth = Some(depth);
        }

        fn dfs_max_depth(
            graph: &petgraph::graph::DiGraph<
                crate::models::CodeEntity,
                crate::models::Relationship,
            >,
            node: NodeIndex,
            visited: &mut HashSet<NodeIndex>,
            current: u32,
            max_depth: u32,
        ) -> u32 {
            if current >= max_depth || !visited.insert(node) {
                return current;
            }
            let mut best = current;
            for edge in graph.edges(node) {
                if !edge.weight().kind.is_dependency() {
                    continue;
                }
                let target = edge.target();
                if target == node {
                    continue;
                }
                let candidate = dfs_max_depth(graph, target, visited, current + 1, max_depth);
                if candidate > best {
                    best = candidate;
                }
            }
            visited.remove(&node);
            best
        }
    }

    /// PageRank centrality on the dependency subgraph.
    /// Standard iterative algorithm:
    ///   r_new(v) = (1 - d) / N + d · Σ_{u→v} r_old(u) / out_deg(u)
    /// with sinks (no outbound edges) redistributed to all nodes so
    /// probability mass is preserved.
    fn populate_pagerank(&mut self) {
        let n_usize = self.graph.node_count();
        if n_usize == 0 {
            return;
        }
        let n = n_usize as f32;
        let damping: f32 = 0.85;
        let base = (1.0 - damping) / n;
        const ITERATIONS: usize = 40;
        const TOL: f32 = 1e-6;

        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        // Build index → position map for array-based iteration.
        let idx_pos: HashMap<NodeIndex, usize> =
            indices.iter().enumerate().map(|(i, n)| (*n, i)).collect();

        // Outbound dependency edges per node (distinct targets).
        let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n_usize];
        for (i, node) in indices.iter().enumerate() {
            let mut seen: HashSet<NodeIndex> = HashSet::new();
            for edge in self.graph.edges(*node) {
                if !edge.weight().kind.is_dependency() {
                    continue;
                }
                let tgt = edge.target();
                if tgt == *node || !seen.insert(tgt) {
                    continue;
                }
                if let Some(&pos) = idx_pos.get(&tgt) {
                    out_edges[i].push(pos);
                }
            }
        }

        let mut rank: Vec<f32> = vec![1.0 / n; n_usize];
        let mut next: Vec<f32> = vec![0.0; n_usize];

        for _ in 0..ITERATIONS {
            // Sinks: distribute their rank uniformly to preserve mass.
            let sink_mass: f32 = out_edges
                .iter()
                .enumerate()
                .filter_map(|(i, v)| if v.is_empty() { Some(rank[i]) } else { None })
                .sum::<f32>()
                * damping
                / n;

            for v in next.iter_mut() {
                *v = base + sink_mass;
            }
            for (i, targets) in out_edges.iter().enumerate() {
                if targets.is_empty() {
                    continue;
                }
                let share = damping * rank[i] / targets.len() as f32;
                for &t in targets {
                    next[t] += share;
                }
            }

            // Early-exit on convergence.
            let delta: f32 = rank
                .iter()
                .zip(next.iter())
                .map(|(a, b)| (a - b).abs())
                .sum();
            std::mem::swap(&mut rank, &mut next);
            if delta < TOL {
                break;
            }
        }

        for (i, node) in indices.iter().enumerate() {
            self.graph[*node].metrics.pagerank = Some(rank[i]);
        }
    }

    /// Compute a composite "refactor pressure" score for every entity.
    /// Mirrors the formula originally in the frontend (`quality.ts`):
    ///
    /// **Callables** (function / method):
    ///   `0.25·CC + 0.15·cognitive + 0.25·fan_out + 0.15·LOC + 0.10·params + 0.10·nest + cycle`
    ///
    /// **Containers** (struct / enum / trait / module / class):
    ///   `0.3·norm(fields,R) + 0.2·norm(methods,25) + 0.15·norm(loc,200)
    ///    + 0.15·norm(fan_out,15) + 0.2·norm(encaps,0.8) + cycle`
    ///
    /// where `norm(v, red) = min(v / red, 2)` and `cycle = 0.5` if in_cycle.
    fn populate_composite_scores(&mut self) {
        use crate::models::{EntityKind, Thresholds};

        fn norm(v: f32, red: f32) -> f32 {
            (v / red).min(2.0)
        }

        let t = Thresholds::default();
        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        for idx in indices {
            let e = &self.graph[idx];
            let m = &e.metrics;
            let cycle = if m.in_cycle { 0.5_f32 } else { 0.0 };

            let score = if e.kind.is_callable() {
                let cc = norm(m.cyclomatic.unwrap_or(0) as f32, t.cc.bad);
                let cog = norm(m.cognitive_complexity.unwrap_or(0) as f32, t.cognitive.bad);
                let nest = norm(m.max_nesting.unwrap_or(0) as f32, t.nest.bad);
                let loc = norm(m.loc as f32, t.loc_callable.bad);
                let fo = norm(m.fan_out as f32, t.fan_out.bad);
                let params = norm(m.param_count.unwrap_or(0) as f32, t.params.bad);
                0.25 * cc
                    + 0.15 * cog
                    + 0.25 * fo
                    + 0.15 * loc
                    + 0.10 * params
                    + 0.10 * nest
                    + cycle
            } else {
                let is_enum = e.kind == EntityKind::Enum;
                let field_red = if is_enum {
                    t.variants.bad
                } else {
                    t.fields.bad
                };
                let fields = norm(m.field_count.unwrap_or(0) as f32, field_red);
                let methods = norm(m.method_count as f32, t.method_count.bad);
                let loc = norm(m.loc as f32, t.loc_container.bad);
                let fo = norm(m.fan_out as f32, t.fan_out.bad);
                let encaps = if m.method_count > 3 {
                    norm(
                        m.public_field_ratio.unwrap_or(0.0),
                        t.public_field_ratio.bad,
                    )
                } else {
                    0.0
                };
                0.3 * fields + 0.2 * methods + 0.15 * loc + 0.15 * fo + 0.2 * encaps + cycle
            };

            self.graph[idx].metrics.composite_score = score;
        }
    }

    /// Detect code smells by checking metric combinations on each entity.
    /// Runs after all per-entity metrics (including composite score) are
    /// finalised, so every field is available for rule evaluation.
    fn detect_smells(&mut self) {
        use crate::models::{SmellKind, Thresholds};

        let t = Thresholds::default();

        // --- Feature Envy pre-pass: group outgoing dependency edges by
        //     target entity's parent, per source entity. ---
        let mut outgoing_by_parent: HashMap<NodeIndex, HashMap<Option<String>, u32>> =
            HashMap::new();
        let mut outgoing_total: HashMap<NodeIndex, u32> = HashMap::new();
        for edge_idx in self.graph.edge_indices() {
            let rel = &self.graph[edge_idx];
            if !rel.kind.is_dependency() {
                continue;
            }
            let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) else {
                continue;
            };
            if src == tgt {
                continue;
            }
            let target_parent = self.graph[tgt].parent_id.clone();
            *outgoing_by_parent
                .entry(src)
                .or_default()
                .entry(target_parent)
                .or_insert(0) += 1;
            *outgoing_total.entry(src).or_insert(0) += 1;
        }

        // --- Main detection pass ---
        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        for idx in indices {
            let e = &self.graph[idx];
            // Skip ghost/external entities — their metrics aren't real and
            // flagging stdlib functions as "shotgun surgery" is just noise.
            if e.tags.contains("ghost") {
                continue;
            }
            let m = &e.metrics;
            let mut smells: Vec<SmellKind> = Vec::new();

            if e.kind.is_container() {
                let fc = m.field_count.unwrap_or(0) as f32;
                let mc = m.method_count as f32;
                if fc > t.god_class_fields
                    && mc > t.god_class_methods
                    && m.fan_out as f32 > t.god_class_fan_out
                {
                    smells.push(SmellKind::GodClass);
                } else if fc >= t.data_bag_fields && mc <= t.data_bag_max_methods {
                    // Data bag: many fields, little behaviour. Distinct
                    // from god class — flagged only when the god-class
                    // rule did NOT fire, so the two are mutually exclusive.
                    smells.push(SmellKind::DataBag);
                }
            }

            if e.kind.is_callable() {
                let cc = m.cyclomatic.unwrap_or(0);
                if cc as f32 > t.dispatcher_cc
                    && m.fan_out as f32 > t.dispatcher_fan_out
                    && m.fan_out as f32 > cc as f32 * t.dispatcher_fan_out_cc_ratio
                {
                    let loc_per_branch = m.loc as f32 / cc as f32;
                    if loc_per_branch < t.dispatcher_loc_per_branch {
                        smells.push(SmellKind::Dispatcher);
                    }
                }

                smells.extend(overfull_head(m, &t));

                let total = outgoing_total.get(&idx).copied().unwrap_or(0) as usize;
                if total >= t.feature_envy_min_edges as usize {
                    if let Some(by_parent) = outgoing_by_parent.get(&idx) {
                        let own_parent = &e.parent_id;
                        for (target_parent, &count) in by_parent {
                            if target_parent == own_parent {
                                continue;
                            }
                            if target_parent.is_none() {
                                continue;
                            }
                            if (count as f32 / total as f32) >= t.feature_envy_ratio {
                                smells.push(SmellKind::FeatureEnvy);
                                break;
                            }
                        }
                    }
                }
            }

            if m.fan_in as f32 > t.shotgun_fan_in && !m.in_cycle {
                smells.push(SmellKind::ShotgunSurgery);
            }

            if !smells.is_empty() {
                self.graph[idx].metrics.smells = smells;
            }
        }
    }

    /// Populate per-file and per-module scope metrics. Thin orchestrator
    /// that delegates to focused helper methods.
    fn populate_scope_metrics(&mut self) {
        let tally = self.scan_entity_files();
        let edges = self.scan_file_edges(&tally.entity_file);
        let cycles = Self::compute_file_cycles(&tally, &edges);
        let files = Self::assemble_file_metrics(&tally, &edges, &cycles);
        let folders = Self::compute_folder_metrics(&tally, &edges, &cycles, &self.import_sites);
        self.file_metrics = files;
        self.folder_metrics = folders;
    }

    // ------------------------------------------------------------------
    //  Scope-metric helpers — each owns one phase of the pipeline.
    // ------------------------------------------------------------------

    /// Normalise a raw file path: strip `./` (CurDir) components so that
    /// `./src/foo.rs`, `src/foo.rs`, and `./src/./foo.rs` all bucket to
    /// the same key.
    fn normalize_path(p: &std::path::Path) -> String {
        use std::path::{Component, PathBuf};
        let mut buf = PathBuf::new();
        for c in p.components() {
            if !matches!(c, Component::CurDir) {
                buf.push(c.as_os_str());
            }
        }
        buf.display().to_string()
    }

    /// Aggregate `(score, loc)` pairs into quality-tier counts with a
    /// LOC-weighted average. Larger entities contribute proportionally more
    /// to the average so small healthy getters don't mask large complex functions.
    /// Returns `(weighted_avg, max, ok, warn, bad)`.
    fn quality_rollup(scores: &[(f32, u32)]) -> (f32, f32, u32, u32, u32) {
        if scores.is_empty() {
            return (0.0, 0.0, 0, 0, 0);
        }
        let mut weighted_sum = 0.0_f32;
        let mut total_loc = 0u32;
        let mut max = 0.0_f32;
        let (mut ok, mut warn, mut bad) = (0u32, 0u32, 0u32);
        for &(s, loc) in scores {
            // Weight by LOC, with a minimum of 1 so zero-LOC entities still count.
            let w = loc.max(1);
            weighted_sum += s * w as f32;
            total_loc += w;
            if s > max {
                max = s;
            }
            if s <= 0.5 {
                ok += 1;
            } else if s <= 1.0 {
                warn += 1;
            } else {
                bad += 1;
            }
        }
        let avg = if total_loc > 0 {
            weighted_sum / total_loc as f32
        } else {
            0.0
        };
        (avg, max, ok, warn, bad)
    }

    /// Phase 1: walk every entity and tally per-file counts + scores.
    fn scan_entity_files(&self) -> FileTally {
        use crate::models::EntityKind;

        let mut tally = FileTally {
            entity_file: HashMap::new(),
            entity_count: HashMap::new(),
            callable_count: HashMap::new(),
            container_count: HashMap::new(),
            loc: HashMap::new(),
            scores: HashMap::new(),
            declaration_only: HashSet::new(),
        };
        let mut max_end_line: HashMap<String, usize> = HashMap::new();
        let mut substantive: HashSet<String> = HashSet::new();

        for idx in self.graph.node_indices() {
            let e = &self.graph[idx];
            let path = Self::normalize_path(&e.file_path);
            if path.is_empty() {
                continue;
            }
            // Every entity is mapped to its file, synthetic or not. This map is
            // what `scan_file_edges` resolves an edge's two endpoints through,
            // and a call written inside an `if` or a `try` is attributed to the
            // Branch entity for that arm — so excluding synthetic entities here
            // dropped those edges out of the file graph completely.
            //
            // The consequence was not cosmetic. The file graph is what folder
            // shape, `mezz check`'s door and importer rules, and
            // `import_coverage` are all computed over, so a dependency created
            // inside a conditional body was invisible to every one of them: a
            // folder read as having no edge between two children that call each
            // other, and the import that carried it read as unresolved. Five
            // field reports in a row correlated their missing edges with calls
            // inside `if` and `try` bodies, which is exactly this.
            tally.entity_file.insert(idx, path.clone());
            // Counts are a different question, and synthetic entities are
            // rendering aids rather than program structure — a Branch is not a
            // declaration a reader meets on opening the file.
            if is_synthetic(e) {
                continue;
            }
            *tally.entity_count.entry(path.clone()).or_insert(0) += 1;
            if e.kind != EntityKind::Module {
                substantive.insert(path.clone());
            }
            if e.kind.is_callable() {
                *tally.callable_count.entry(path.clone()).or_insert(0) += 1;
            }
            if e.kind.is_container() {
                *tally.container_count.entry(path.clone()).or_insert(0) += 1;
            }
            tally
                .scores
                .entry(path.clone())
                .or_default()
                .push((e.metrics.composite_score, e.metrics.loc));
            let slot = max_end_line.entry(path).or_insert(0);
            if e.span.end.line > *slot {
                *slot = e.span.end.line;
            }
        }
        for (path, &max_line) in &max_end_line {
            tally.loc.insert(path.clone(), (max_line + 1) as u32);
        }
        tally.declaration_only = tally
            .entity_count
            .keys()
            .filter(|path| !substantive.contains(*path))
            .cloned()
            .collect();
        tally
    }

    /// Phase 2: scan dependency edges and bucket them per file.
    fn scan_file_edges(&self, entity_file: &HashMap<NodeIndex, String>) -> FileEdgeData {
        let mut data = FileEdgeData {
            deps: EdgeBuckets::default(),
            refs: EdgeBuckets::default(),
        };
        for edge_idx in self.graph.edge_indices() {
            let rel = &self.graph[edge_idx];
            // Only these two families are tallied. Structural kinds like
            // `Contains` are containment rather than coupling, and counting
            // them would put the new number in exactly the position the old
            // one was in (UI-091).
            let bucket = if rel.kind.is_dependency() {
                &mut data.deps
            } else if rel.kind == RelationshipKind::References {
                &mut data.refs
            } else {
                continue;
            };
            let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) else {
                continue;
            };
            let (Some(sf), Some(tf)) = (entity_file.get(&src), entity_file.get(&tgt)) else {
                continue;
            };
            bucket.record(sf, tf);
        }
        data
    }

    /// Phase 3: build a tiny directed graph over file names and find SCCs.
    fn compute_file_cycles(tally: &FileTally, edges: &FileEdgeData) -> HashSet<String> {
        let mut file_graph: DiGraph<String, ()> = DiGraph::new();
        let mut idx_map: HashMap<String, NodeIndex> = HashMap::new();
        for path in tally.entity_count.keys() {
            let idx = file_graph.add_node(path.clone());
            idx_map.insert(path.clone(), idx);
        }
        for (a, b) in &edges.deps.pairs {
            if let (Some(&ai), Some(&bi)) = (idx_map.get(a), idx_map.get(b)) {
                file_graph.add_edge(ai, bi, ());
            }
        }
        let mut in_cycle = HashSet::new();
        for scc in petgraph::algo::tarjan_scc(&file_graph) {
            if scc.len() > 1 {
                for idx in scc {
                    in_cycle.insert(file_graph[idx].clone());
                }
            }
        }
        in_cycle
    }

    /// Phase 4: join tally + edge data + cycles into `FileMetrics`.
    fn assemble_file_metrics(
        tally: &FileTally,
        edges: &FileEdgeData,
        cycles: &HashSet<String>,
    ) -> Vec<FileMetrics> {
        let mut paths: Vec<&String> = tally.entity_count.keys().collect();
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let internal = edges.deps.internal.get(path).copied().unwrap_or(0);
            let fi = edges.deps.fan_in_of(path);
            let fo = edges.deps.fan_out_of(path);
            let external = fi + fo;
            let total = internal + external;
            let cohesion = if total == 0 {
                None
            } else {
                Some(internal as f32 / total as f32)
            };
            let (avg_q, max_q, q_ok, q_warn, q_bad) =
                Self::quality_rollup(tally.scores.get(path).map(|v| v.as_slice()).unwrap_or(&[]));
            let mut metrics = ScopeMetrics {
                entity_count: *tally.entity_count.get(path).unwrap_or(&0),
                callable_count: *tally.callable_count.get(path).unwrap_or(&0),
                container_count: *tally.container_count.get(path).unwrap_or(&0),
                loc: *tally.loc.get(path).unwrap_or(&0),
                internal_edges: internal,
                external_edges: external,
                cohesion,
                fan_in: fi,
                fan_out: fo,
                in_cycle: cycles.contains(path),
                instability: if fi + fo > 0 {
                    Some(fo as f32 / (fi + fo) as f32)
                } else {
                    None
                },
                ref_fan_in: edges.refs.fan_in_of(path),
                ref_fan_out: edges.refs.fan_out_of(path),
                avg_quality: avg_q,
                max_quality: max_q,
                quality_ok: q_ok,
                quality_warn: q_warn,
                quality_bad: q_bad,
                ..Default::default()
            };
            metrics.compute_composite_score(path, false);
            files.push(FileMetrics {
                path: path.clone(),
                metrics,
            });
        }
        files
    }

    /// Phase 5: roll file-level data up to directory (folder) granularity.
    fn compute_folder_metrics(
        tally: &FileTally,
        edges: &FileEdgeData,
        cycles: &HashSet<String>,
        imports: &[ImportSite],
    ) -> Vec<FolderMetrics> {
        let folder_paths = Self::enumerate_folder_paths(tally);
        if folder_paths.is_empty() {
            return Vec::new();
        }

        // Folder shape reads the same dependency pairs the coupling numbers
        // above are built from, but asks a different question of them —
        // whether the picture each folder draws can be followed. It is
        // scored once for the whole tree because the measure is recursive:
        // a folder's standing depends on its subfolders'.
        let shapes = crate::analyzer::folder_shape::compute(
            tally.entity_count.keys().map(String::as_str),
            &edges.deps.pairs,
            imports,
            &folder_paths,
            &tally.declaration_only,
        );

        let mut sorted: Vec<String> = folder_paths.into_iter().collect();
        sorted.sort();
        let mut folders = Vec::with_capacity(sorted.len());

        for mp in &sorted {
            let descendants: Vec<&String> = tally
                .entity_count
                .keys()
                .filter(|f| Self::is_descendant(mp, f))
                .collect();

            let mut m = Self::aggregate_file_counts(tally, &descendants);
            let (internal, external, fan_in, fan_out) =
                Self::classify_folder_edges(&edges.deps, &descendants);
            // Same classification over the reference bucket. Only the fan
            // counts are kept: references feed no ratio and no score, they
            // exist so the reader can tell an unmeasured scope from a
            // decoupled one (UI-091).
            let (_, _, ref_fan_in, ref_fan_out) =
                Self::classify_folder_edges(&edges.refs, &descendants);
            m.ref_fan_in = ref_fan_in;
            m.ref_fan_out = ref_fan_out;
            m.internal_edges = internal;
            m.external_edges = external;
            let total = internal + external;
            m.cohesion = if total == 0 {
                None
            } else {
                Some(internal as f32 / total as f32)
            };
            m.fan_in = fan_in;
            m.fan_out = fan_out;
            m.instability = if fan_in + fan_out > 0 {
                Some(fan_out as f32 / (fan_in + fan_out) as f32)
            } else {
                None
            };
            m.in_cycle =
                descendants.iter().any(|f| cycles.contains(*f)) && m.fan_out > 0 && m.fan_in > 0;

            let mut all_scores: Vec<(f32, u32)> = Vec::new();
            for f in &descendants {
                if let Some(scores) = tally.scores.get(*f) {
                    all_scores.extend(scores.iter().copied());
                }
            }
            // Descendants iterate in hash order; float summation is not
            // associative, so pin the order or avg_quality drifts in the
            // last bits between identical runs (AN-002).
            all_scores.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let (avg_q, max_q, q_ok, q_warn, q_bad) = Self::quality_rollup(&all_scores);
            m.avg_quality = avg_q;
            m.max_quality = max_q;
            m.quality_ok = q_ok;
            m.quality_warn = q_warn;
            m.quality_bad = q_bad;

            m.compute_composite_score(mp, true);
            // Assigned after the composite score, and never read by it:
            // organisation is not code quality (see `ScopeMetrics::shape`).
            m.shape = shapes.get(mp).cloned();
            folders.push(FolderMetrics {
                path: mp.clone(),
                metrics: m,
            });
        }
        folders
    }

    /// Parent directory of a file path (everything before the last separator).
    fn parent_dir(file_path: &str) -> String {
        match file_path.rfind(std::path::MAIN_SEPARATOR) {
            Some(i) => file_path[..i].to_string(),
            None => String::new(),
        }
    }

    /// Does `file_path` live under `folder_path` (possibly in a subdirectory)?
    fn is_descendant(folder_path: &str, file_path: &str) -> bool {
        if folder_path.is_empty() {
            return true;
        }
        file_path
            .strip_prefix(folder_path)
            .map_or(false, |rest| rest.starts_with(std::path::MAIN_SEPARATOR))
    }

    /// The deepest directory every analysed file sits under, or `None` when
    /// nothing was analysed.
    ///
    /// Compared component by component rather than as a string prefix, so
    /// `src/parser/` and `src/parsed/` reduce to `src` and not to the
    /// characters they happen to share.
    fn common_parent_dir(tally: &FileTally) -> Option<String> {
        use std::path::{Path, PathBuf};

        let components = |p: &str| -> Vec<std::ffi::OsString> {
            Path::new(&Self::parent_dir(p))
                .components()
                .map(|c| c.as_os_str().to_owned())
                .collect()
        };

        let mut iter = tally.entity_count.keys();
        let mut prefix = components(iter.next()?);
        for path in iter {
            let keep = prefix
                .iter()
                .zip(components(path).iter())
                .take_while(|(a, b)| a == b)
                .count();
            prefix.truncate(keep);
        }

        let mut buf = PathBuf::new();
        for c in &prefix {
            buf.push(c);
        }
        Some(buf.display().to_string())
    }

    /// Every ancestor directory from [`Self::common_parent_dir`] down to the
    /// one holding each analysed file, inclusive. The full set of folder
    /// paths the rollups are computed over.
    fn enumerate_folder_paths(tally: &FileTally) -> HashSet<String> {
        let Some(lcp) = Self::common_parent_dir(tally) else {
            return HashSet::new();
        };

        let mut paths: HashSet<String> = HashSet::new();
        paths.insert(lcp.clone());
        for file_path in tally.entity_count.keys() {
            let mut cur = Self::parent_dir(file_path);
            loop {
                paths.insert(cur.clone());
                if cur == lcp || cur.is_empty() {
                    break;
                }
                cur = Self::parent_dir(&cur);
            }
        }
        paths
    }

    /// Sum file-level entity/callable/container/LOC counts for a set of
    /// descendant files into a fresh `ScopeMetrics` (edge fields left at 0).
    fn aggregate_file_counts(tally: &FileTally, descendants: &[&String]) -> ScopeMetrics {
        let mut m = ScopeMetrics::default();
        for f in descendants {
            m.entity_count += tally.entity_count.get(*f).copied().unwrap_or(0);
            m.callable_count += tally.callable_count.get(*f).copied().unwrap_or(0);
            m.container_count += tally.container_count.get(*f).copied().unwrap_or(0);
            m.loc += tally.loc.get(*f).copied().unwrap_or(0);
        }
        m
    }

    /// Classify cross-file edge pairs as internal or external to a module
    /// defined by `descendants`. Returns `(internal_edges, external_edges,
    /// fan_in_count, fan_out_count)`.
    fn classify_folder_edges(edges: &EdgeBuckets, descendants: &[&String]) -> (u32, u32, u32, u32) {
        // Intra-file edges are always internal to any ancestor module.
        let mut internal = 0u32;
        for f in descendants {
            internal += edges.internal.get(*f).copied().unwrap_or(0);
        }
        let desc_set: HashSet<&String> = descendants.iter().copied().collect();
        let mut fan_in_set: HashSet<String> = HashSet::new();
        let mut fan_out_set: HashSet<String> = HashSet::new();
        let mut external = 0u32;
        for (a, b) in &edges.pairs {
            match (desc_set.contains(a), desc_set.contains(b)) {
                (true, true) => internal += 1,
                (true, false) => {
                    fan_out_set.insert(b.clone());
                    external += 1;
                }
                (false, true) => {
                    fan_in_set.insert(a.clone());
                    external += 1;
                }
                _ => {}
            }
        }
        (
            internal,
            external,
            fan_in_set.len() as u32,
            fan_out_set.len() as u32,
        )
    }

    /// Re-number call order metadata so they're sequential (1, 2, 3, ...)
    /// per source entity, eliminating gaps from filtered-out calls.
    fn renumber_call_orders(&mut self) {
        use std::collections::HashMap;

        // Group edge indices by source node, collecting (original_order, edge_index)
        let mut source_edges: HashMap<NodeIndex, Vec<(u32, petgraph::graph::EdgeIndex)>> =
            HashMap::new();

        for edge_idx in self.graph.edge_indices() {
            if let Some((src, _)) = self.graph.edge_endpoints(edge_idx) {
                let rel = &self.graph[edge_idx];
                if let Some(order_str) = rel.metadata.get("order") {
                    if let Ok(order) = order_str.parse::<u32>() {
                        source_edges.entry(src).or_default().push((order, edge_idx));
                    }
                }
            }
        }

        // Sort each group by original order and assign new sequential numbers
        for (_, edges) in &mut source_edges {
            edges.sort_by_key(|(order, _)| *order);
            for (new_order, (_, edge_idx)) in edges.iter().enumerate() {
                self.graph[*edge_idx]
                    .metadata
                    .insert("order".to_string(), (new_order as u32 + 1).to_string());
            }
        }
    }

    /// Infer the entity kind for a ghost node based on the relationship type.
    fn infer_ghost_kind(rel_kind: RelationshipKind) -> EntityKind {
        match rel_kind {
            RelationshipKind::Calls => EntityKind::Function,
            RelationshipKind::Implements => EntityKind::Trait,
            RelationshipKind::Inherits => EntityKind::Class,
            RelationshipKind::UsesType
            | RelationshipKind::Returns
            | RelationshipKind::Instantiates => EntityKind::Struct,
            RelationshipKind::Imports => EntityKind::Module,
            _ => EntityKind::Unknown,
        }
    }

    /// Return the EntityKind a ghost for this name should take — or
    /// None if the name isn't a known built-in type. A Python
    /// programmer writes `int(x)` (call) and `-> int` (type) for the
    /// same identifier, so we can't rely on which relationship
    /// discovered the ghost first. Matching by name fixes the label:
    /// Python built-ins become `Class` (their true Python kind), Rust
    /// primitives and owned types become `Struct` so Rust users see
    /// the idiomatic label.
    fn ghost_type_kind(name: &str) -> Option<EntityKind> {
        // Strip qualified prefixes (`std::string::String` → `String`).
        let last = name.rsplit("::").next().unwrap_or(name);
        // Python built-in TYPES — everything in Python is a class, so
        // `int` / `str` / `list` are Classes, not Structs.
        if matches!(
            last,
            "int"
                | "str"
                | "float"
                | "bool"
                | "bytes"
                | "bytearray"
                | "list"
                | "dict"
                | "set"
                | "tuple"
                | "frozenset"
                | "complex"
                | "type"
                | "object"
                | "None"
        ) {
            return Some(EntityKind::Class);
        }
        // Rust primitives and common owned / smart-pointer / collection
        // types — all Structs in Rust's model.
        if matches!(
            last,
            "u8" | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "f32"
                | "f64"
                | "char"
                | "String"
                | "Vec"
                | "Option"
                | "Result"
                | "Box"
                | "Rc"
                | "Arc"
                | "Cow"
                | "HashMap"
                | "HashSet"
                | "BTreeMap"
                | "BTreeSet"
        ) {
            return Some(EntityKind::Struct);
        }
        None
    }

    /// Categorize a ghost entity based on its name.
    fn ghost_category(name: &str) -> &'static str {
        // Rust stdlib
        if name.starts_with("std::") || name.starts_with("core::") || name.starts_with("alloc::") {
            return "ghost_stdlib";
        }
        // Common Rust built-in types and traits
        const BUILTINS: &[&str] = &[
            // Primitives — must include lowercase names now that
            // `extract_type_names` no longer drops them.
            "str",
            "bool",
            "char",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "usize",
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "isize",
            "f32",
            "f64",
            // Common owned / smart-pointer types
            "String",
            "Vec",
            "Option",
            "Result",
            "Box",
            "Rc",
            "Arc",
            "Cow",
            "HashMap",
            "HashSet",
            "BTreeMap",
            "BTreeSet",
            "PhantomData",
            // Widespread traits
            "Display",
            "Debug",
            "Clone",
            "Copy",
            "Default",
            "Iterator",
            "IntoIterator",
            "From",
            "Into",
            "TryFrom",
            "TryInto",
            "AsRef",
            "AsMut",
            "Send",
            "Sync",
            "Sized",
            "Drop",
            "Fn",
            "FnMut",
            "FnOnce",
            "Eq",
            "PartialEq",
            "Ord",
            "PartialOrd",
            "Hash",
            "Read",
            "Write",
            "Seek",
            "BufRead",
            "ToString",
            "ToOwned",
            "Borrow",
            "BorrowMut",
            // Bang-macros commonly called bare
            "println",
            "eprintln",
            "format",
            "panic",
            "assert",
            "assert_eq",
            "todo",
            "unimplemented",
            "unreachable",
        ];
        let last_segment = name.rsplit("::").next().unwrap_or(name);
        if BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        // Python builtins — keep in sync with python_parser::is_bare_builtin.
        const PY_BUILTINS: &[&str] = &[
            // Printing, I/O, introspection
            "print",
            "input",
            "open",
            "format",
            "repr",
            "vars",
            "dir",
            "help",
            "locals",
            "globals",
            // Collection constructors / coercions
            "len",
            "range",
            "str",
            "int",
            "float",
            "list",
            "dict",
            "set",
            "tuple",
            "frozenset",
            "bool",
            "bytes",
            "bytearray",
            "complex",
            // Iteration
            "iter",
            "next",
            "map",
            "filter",
            "zip",
            "enumerate",
            "sorted",
            "reversed",
            "any",
            "all",
            // Numeric
            "abs",
            "round",
            "min",
            "max",
            "sum",
            "pow",
            "divmod",
            "hex",
            "oct",
            "bin",
            "ord",
            "chr",
            "ascii",
            "hash",
            "id",
            // Type system
            "type",
            "super",
            "object",
            "callable",
            "isinstance",
            "issubclass",
            "hasattr",
            "getattr",
            "setattr",
            "delattr",
            "staticmethod",
            "classmethod",
            "property",
            // Metaprogramming
            "compile",
            "eval",
            "exec",
            "breakpoint",
            "exit",
            "quit",
            // Exceptions
            "Exception",
            "BaseException",
            "ValueError",
            "TypeError",
            "KeyError",
            "IndexError",
            "AttributeError",
            "RuntimeError",
            "StopIteration",
            "StopAsyncIteration",
            "NotImplementedError",
            "FileNotFoundError",
            "OSError",
            "IOError",
            "ZeroDivisionError",
            "ArithmeticError",
            "AssertionError",
            "LookupError",
            "NameError",
            "UnicodeError",
            "UnicodeDecodeError",
            "UnicodeEncodeError",
            // Common stdlib module attributes that sometimes surface
            "None",
            "True",
            "False",
            "NotImplemented",
            "Ellipsis",
        ];
        if PY_BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        // Go predeclared types and built-in functions. `error`, `string`
        // and `int` are as common in a Go graph as `str` is in a Python
        // one, and without this they read as third-party.
        const GO_BUILTINS: &[&str] = &[
            "error",
            "string",
            "rune",
            "byte",
            "int",
            "int8",
            "int16",
            "int32",
            "int64",
            "uint",
            "uint8",
            "uint16",
            "uint32",
            "uint64",
            "uintptr",
            "float32",
            "float64",
            "complex64",
            "complex128",
            "any",
            "comparable",
            "append",
            "make",
            "len",
            "cap",
            "copy",
            "delete",
            "recover",
        ];
        if GO_BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        // JS/TS builtins
        const JS_BUILTINS: &[&str] = &[
            "console",
            "Math",
            "JSON",
            "Promise",
            "Array",
            "Object",
            "Map",
            "Set",
            "Date",
            "Error",
            "RegExp",
            "Symbol",
            "Number",
            "Boolean",
            "setTimeout",
            "setInterval",
            "fetch",
            "parseInt",
            "parseFloat",
        ];
        if JS_BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        "ghost_external"
    }

    /// Add an entity to the graph
    pub fn add_entity(&mut self, entity: CodeEntity) -> NodeIndex {
        if let Some(&idx) = self.node_map.get(&entity.id) {
            return idx;
        }

        let id = entity.id.clone();
        let idx = self.graph.add_node(entity);
        self.node_map.insert(id.clone(), idx);
        self.reverse_map.insert(idx, id);
        idx
    }

    /// Add a relationship to the graph
    pub fn add_relationship(&mut self, rel: Relationship) -> bool {
        let source_idx = self.node_map.get(&rel.source_id);
        let target_idx = self.node_map.get(&rel.target_id);

        match (source_idx, target_idx) {
            (Some(&src), Some(&tgt)) => {
                self.graph.add_edge(src, tgt, rel);
                true
            }
            _ => false,
        }
    }

    /// Get an entity by ID
    pub fn get_entity(&self, id: &str) -> Option<&CodeEntity> {
        self.node_map.get(id).map(|&idx| &self.graph[idx])
    }

    /// How much of each entity's `fan_out` points at something mezz could
    /// not identify, keyed by entity id.
    ///
    /// The number behind a ranking artefact the `quality` header has been
    /// disclaiming in the abstract since it was written: *"a builder chain
    /// scores its every link as a dependency"*. On this repo 56% of
    /// dependency edges land on a ghost, and a fluent chain against an
    /// external library — `new Setting(el).setName().setDesc().addText()` —
    /// is four of them, on a function whose real dependency set is one
    /// class.
    ///
    /// Reported rather than corrected, deliberately. Collapsing a chain to
    /// its receiver needs the receiver, and an external chain resolves to
    /// parentless ghosts carrying no receiver at all (AN-031) — so the
    /// correction is a parser change, and a scoring change made without
    /// one would re-rank every repo while leaving this exact case alone.
    /// A reader who can see *which* rows are mostly unidentified does not
    /// have to learn to skim the whole list.
    ///
    /// Distinct targets, matching how `fan_out` itself counts, so the two
    /// numbers are on the same scale and "17 of 20" means what it looks
    /// like.
    pub fn unresolved_fan_out(&self) -> HashMap<&str, u32> {
        let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
        for edge in self.graph.edge_indices() {
            let rel = &self.graph[edge];
            if !rel.kind.is_dependency() {
                continue;
            }
            let Some((src, tgt)) = self.graph.edge_endpoints(edge) else {
                continue;
            };
            if src == tgt || !self.graph[tgt].tags.contains("ghost") {
                continue;
            }
            seen.entry(self.graph[src].id.as_str())
                .or_default()
                .insert(self.graph[tgt].id.as_str());
        }
        seen.into_iter()
            .map(|(id, targets)| (id, targets.len() as u32))
            .collect()
    }


    /// Get all entities
    /// Get all entities
    pub fn entities(&self) -> impl Iterator<Item = &CodeEntity> {
        self.graph.node_weights()
    }

    /// Get all relationships
    pub fn relationships(&self) -> impl Iterator<Item = &Relationship> {
        self.graph.edge_weights()
    }

    /// Get the number of nodes
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Get the number of edges
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Get direct dependencies of an entity
    pub fn dependencies(&self, entity_id: &str) -> Vec<(&CodeEntity, &Relationship)> {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };

        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .filter(|e| e.weight().kind.is_dependency())
            .map(|e| (&self.graph[e.target()], e.weight()))
            .collect()
    }

    /// Get entities that depend on this entity
    pub fn dependents(&self, entity_id: &str) -> Vec<(&CodeEntity, &Relationship)> {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };

        self.graph
            .edges_directed(idx, Direction::Incoming)
            .filter(|e| e.weight().kind.is_dependency())
            .map(|e| (&self.graph[e.source()], e.weight()))
            .collect()
    }

    /// Get children of an entity (containment relationship)
    pub fn children(&self, entity_id: &str) -> Vec<&CodeEntity> {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };

        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .filter(|e| e.weight().kind == RelationshipKind::Contains)
            .map(|e| &self.graph[e.target()])
            .collect()
    }

    /// Get the parent of an entity (if any)
    pub fn parent(&self, entity_id: &str) -> Option<&CodeEntity> {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return None,
        };

        self.graph
            .edges_directed(idx, Direction::Incoming)
            .find(|e| e.weight().kind == RelationshipKind::Contains)
            .map(|e| &self.graph[e.source()])
    }

    /// True when this entity declares a base type or interface the
    /// analysis never saw. `resolve_inheritance` marks such an edge
    /// `external` precisely because it could not find the declaration in
    /// the tree; anything reaching the entity through that contract —
    /// a framework calling a lifecycle hook it declared — is outside the
    /// graph by construction.
    pub fn has_external_supertype(&self, entity_id: &str) -> bool {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return false,
        };

        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .filter(|e| {
                matches!(
                    e.weight().kind,
                    RelationshipKind::Inherits | RelationshipKind::Implements
                )
            })
            .any(|e| {
                e.weight()
                    .metadata
                    .get("external")
                    .is_some_and(|v| v == "true")
            })
    }

    /// Get related entities by relationship kind and direction.
    /// Returns (entity, relationship) pairs for all matching edges.
    pub fn related_by_kind(
        &self,
        entity_id: &str,
        kind: RelationshipKind,
        direction: Direction,
    ) -> Vec<(&CodeEntity, &Relationship)> {
        let idx = match self.node_map.get(entity_id) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };

        self.graph
            .edges_directed(idx, direction)
            .filter(|e| e.weight().kind == kind)
            .map(|e| {
                let peer = match direction {
                    Direction::Outgoing => e.target(),
                    Direction::Incoming => e.source(),
                };
                (&self.graph[peer], e.weight())
            })
            .collect()
    }

    /// Get all entities whose file_path matches a set of paths.
    pub fn entities_in_files(&self, paths: &std::collections::HashSet<String>) -> Vec<&CodeEntity> {
        self.graph
            .node_weights()
            .filter(|e| paths.contains(&e.file_path.display().to_string()))
            .collect()
    }

    /// Get entities of a specific kind
    pub fn entities_of_kind(&self, kind: EntityKind) -> Vec<&CodeEntity> {
        self.graph
            .node_weights()
            .filter(|e| e.kind == kind)
            .collect()
    }

    /// Filter the graph to create a subgraph
    pub fn filter<F>(&self, predicate: F) -> DependencyGraph
    where
        F: Fn(&CodeEntity) -> bool,
    {
        let mut new_graph = DependencyGraph::new();

        // Add filtered nodes
        for entity in self.entities() {
            if predicate(entity) {
                new_graph.add_entity(entity.clone());
            }
        }

        // Add edges where both endpoints exist
        for rel in self.relationships() {
            new_graph.add_relationship(rel.clone());
        }

        new_graph
    }

    /// Get transitive dependencies up to a depth
    pub fn transitive_dependencies(
        &self,
        entity_id: &str,
        max_depth: usize,
    ) -> HashMap<usize, Vec<String>> {
        let mut result: HashMap<usize, Vec<String>> = HashMap::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut current_level = vec![entity_id.to_string()];

        for depth in 1..=max_depth {
            let mut next_level = Vec::new();

            for current_id in &current_level {
                if visited.contains(current_id) {
                    continue;
                }
                visited.insert(current_id.clone());

                for (dep, _) in self.dependencies(current_id) {
                    if !visited.contains(&dep.id) {
                        next_level.push(dep.id.clone());
                    }
                }
            }

            if !next_level.is_empty() {
                result.insert(depth, next_level.clone());
            }
            current_level = next_level;

            if current_level.is_empty() {
                break;
            }
        }

        result
    }

    /// Strongly-connected components of the *dependency* subgraph, each with
    /// two or more members. The single definition of "these entities form a
    /// cycle"; both `find_cycles` and per-entity `in_cycle` read it.
    ///
    /// Containment and inheritance edges are excluded on purpose. A `Contains`
    /// edge from a type to its own method, plus any dependency edge back — a
    /// `new() -> Self`, a trait method taking `&self` — closes a loop that
    /// says nothing about the design. Over the whole graph those artifacts
    /// outnumber genuine cycles, so we detect over the edges that actually
    /// mean "depends on": calls, imports, uses-type (see
    /// `RelationshipKind::is_dependency`). Mutual recursion and modules that
    /// import each other still surface, because those edges are dependencies.
    ///
    /// Filtering by predicate rather than materialising a second `DiGraph`
    /// keeps this O(V + E) with no extra allocation.
    fn dependency_sccs(&self) -> Vec<Vec<NodeIndex>> {
        let filtered = petgraph::visit::EdgeFiltered::from_fn(&self.graph, |e| {
            e.weight().kind.is_dependency()
        });
        petgraph::algo::tarjan_scc(&filtered)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect()
    }

    /// Find dependency cycles in the graph. Each entry is the set of entity
    /// IDs in one cycle; self-recursion (a one-node loop) is not reported.
    ///
    /// Members are sorted by entity ID and the cycles by their first member,
    /// so the output never depends on traversal order (AN-002).
    pub fn find_cycles(&self) -> Vec<Vec<String>> {
        let mut cycles: Vec<Vec<String>> = self
            .dependency_sccs()
            .into_iter()
            .map(|scc| {
                let mut cycle: Vec<String> = scc
                    .iter()
                    .filter_map(|idx| self.reverse_map.get(idx).cloned())
                    .collect();
                cycle.sort();
                cycle
            })
            .filter(|cycle| cycle.len() > 1)
            .collect();
        cycles.sort();
        cycles
    }

    /// Calculate metrics for the graph
    pub fn metrics(&self) -> GraphMetrics {
        let node_count = self.node_count();
        let edge_count = self.edge_count();

        // Compute degree once per node, reuse for average + most connected
        let mut connections: Vec<(String, usize)> = self
            .node_map
            .iter()
            .map(|(id, &idx)| {
                let degree = self.graph.edges_directed(idx, Direction::Outgoing).count()
                    + self.graph.edges_directed(idx, Direction::Incoming).count();
                (id.clone(), degree)
            })
            .collect();

        let total_degree: usize = connections.iter().map(|(_, d)| d).sum();
        let avg_degree = if node_count > 0 {
            total_degree as f64 / node_count as f64
        } else {
            0.0
        };

        connections.sort_unstable_by(|a, b| b.1.cmp(&a.1));

        let cycles = self.find_cycles();

        GraphMetrics {
            node_count,
            edge_count,
            average_degree: avg_degree,
            most_connected: connections.into_iter().take(10).collect(),
            cycle_count: cycles.len(),
        }
    }
}

/// Pick the candidate nearest to `from_file` (AN-011).
///
/// Bare names collide heavily — nine per-parser `fn parse` test helpers exist
/// here, plus a `parse` method on every parser — and the caller's own file is
/// overwhelmingly the right answer. Ranking: same file, then same directory,
/// then deepest shared path prefix, then the lowest id.
///
/// This is the rule `DependencyResolver::find_entity_by_name_near` already
/// applies to imports and inheritance. Call edges never got it, which is how a
/// parser test's local `parse` helper bound to `src/mcp/push.rs`.
///
/// The final fallback is the *sorted-first* candidate rather than the
/// first-registered one, so the result stays a deterministic function of the
/// tree (AN-002) even when nothing distinguishes the candidates.
fn pick_nearest(
    candidates: &[String],
    from_file: Option<&std::path::Path>,
    id_to_entity: &std::collections::HashMap<&str, &CodeEntity>,
) -> Option<String> {
    nearest(candidates, from_file, id_to_entity).map(|(_, id)| id)
}

/// `pick_nearest`, but silent when nearness does not actually decide.
///
/// Used where a wrong answer is worse than none: a `<module>::<name>` key is
/// built from a file *stem*, so two `calls.rs` under different parser folders
/// collide on `calls::extract_calls`. When the caller sits beside one of them
/// that is evidence and the edge should be drawn; when it is equidistant from
/// both there is nothing to go on, and inventing an edge between two
/// unrelated parsers is the precision bug AN-006 refused to trade for. The
/// old index expressed that refusal by dropping the key entirely, which threw
/// away the callers who *did* have a reason to prefer one.
fn pick_nearest_unambiguous(
    candidates: &[String],
    from_file: Option<&std::path::Path>,
    id_to_entity: &std::collections::HashMap<&str, &CodeEntity>,
) -> Option<String> {
    match nearest(candidates, from_file, id_to_entity) {
        Some((true, id)) => Some(id),
        _ => None,
    }
}

/// The module name a `<module>::<name>` key uses for one file, or `None`
/// when the file is not one a `::` path can name.
///
/// The entry-point spellings resolve to their directory for the reason
/// [`crate::analyzer::dependency_resolver`]'s `ENTRY_STEMS` gives: a Rust
/// `mod.rs`, a TypeScript `index.ts` and a Python `__init__.py` are the same
/// thing, and none of them is referred to by its stem — `from .shapes import
/// Circle` names the *package* `shapes`, whose code is in
/// `shapes/__init__.py`. `lib.rs` and `main.rs` are crate roots rather than
/// modules — nothing is referenced as `lib::foo`.
fn module_segment<'a>(path: &'a std::path::Path, stem: &'a str) -> Option<&'a str> {
    const QUALIFIED: [&str; 9] = [
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "svelte", "py",
    ];
    let extension = path.extension().and_then(|e| e.to_str())?;
    if !QUALIFIED.contains(&extension) {
        return None;
    }
    match stem {
        "lib" | "main" if extension == "rs" => None,
        "mod" | "index" | "__init__" => path.parent()?.file_name()?.to_str(),
        other => Some(other),
    }
}

/// The nearest candidate, and whether nearness actually chose it — `false`
/// when the winner only won a tie, which the fallback settles arbitrarily.
fn nearest(
    candidates: &[String],
    from_file: Option<&std::path::Path>,
    id_to_entity: &std::collections::HashMap<&str, &CodeEntity>,
) -> Option<(bool, String)> {
    let file_of = |id: &&String| id_to_entity.get(id.as_str()).map(|e| e.file_path.clone());

    // AN-014: a candidate in an unrelated language is not a candidate.
    // Locality ranks what survives; it cannot tell a TypeScript
    // `SessionItem` from a Rust one, because path distance is all it sees.
    let candidates = interoperable_candidates(candidates, from_file, id_to_entity);
    if candidates.len() <= 1 {
        return candidates.first().map(|id| (true, (*id).clone()));
    }
    let Some(from_file) = from_file else {
        return candidates.first().map(|id| (false, (*id).clone()));
    };
    let from_components: Vec<_> = from_file.components().collect();

    // The three passes as one comparable key, ordered the way they were asked:
    // the caller's own file beats its directory beats the deepest shared path
    // prefix. Ranking rather than short-circuiting is what lets the tie be
    // *seen* — a pass that matches two candidates has not chosen between them,
    // and the old `.find()` could not tell that from having chosen.
    let rank = |id: &&String| match file_of(id) {
        Some(p) => (
            p == from_file,
            p.parent() == from_file.parent(),
            p.components()
                .zip(from_components.iter())
                .take_while(|(a, b)| a == *b)
                .count(),
        ),
        None => (false, false, 0),
    };
    let best = candidates.iter().map(&rank).max()?;
    let winners: Vec<_> = candidates.iter().filter(|id| rank(id) == best).collect();
    // Last of equal bests, which is what `max_by_key` returned before this
    // was factored out — the candidates are sorted, so it stays a function of
    // the tree (AN-002) either way.
    winners
        .last()
        .map(|id| (winners.len() == 1, (**id).clone()))
}

/// The candidates whose language can answer a name written in `from_file`
/// (AN-014). An id with no entity behind it is kept: unknown is not evidence.
fn interoperable_candidates<'a>(
    candidates: &'a [String],
    from_file: Option<&std::path::Path>,
    id_to_entity: &std::collections::HashMap<&str, &CodeEntity>,
) -> Vec<&'a String> {
    candidates
        .iter()
        .filter(|id| {
            id_to_entity
                .get(id.as_str())
                .is_none_or(|e| interoperable(from_file, &e.file_path))
        })
        .collect()
}

/// Resolve one edge's callee, having first refused a bare call that the
/// caller's own signature already answers (AN-033).
///
/// `resolve` is the strategy chain built in `from_analysis`; every edge but a
/// shadowed call goes straight to it.
fn resolve_callee(
    rel: &Relationship,
    from_entity: Option<&CodeEntity>,
    from_file: Option<&std::path::Path>,
    resolve: impl Fn(&str, Option<&std::path::Path>) -> Option<String>,
) -> Option<String> {
    if rel.kind == RelationshipKind::Calls
        && from_entity.is_some_and(|caller| binds_callee_as_parameter(caller, &rel.target_id))
    {
        return None;
    }
    resolve(&rel.target_id, from_file)
}

/// Whether the caller binds `callee` as one of its own parameters, which
/// makes a bare call to that name the parameter rather than a same-named
/// definition elsewhere in the tree (AN-033).
///
/// Declining leaves the call a ghost, which is the honest answer: the body
/// behind a callback parameter is chosen by whoever passes it, and the graph
/// does not know which. Dropping the edge instead would hide fan-out that
/// really happens.
///
/// Qualified callees are left alone — `produce.run()` names a member of the
/// parameter, not the parameter, and that edge is resolved on its own terms.
fn binds_callee_as_parameter(caller: &CodeEntity, callee: &str) -> bool {
    if callee.contains("::") || callee.contains('.') {
        return false;
    }
    if !parameters_shadow_callables(&caller.file_path) {
        return false;
    }
    caller.parameters.iter().any(|p| p.name == callee)
}

/// Languages where a parameter shadows a same-named callable at a bare call
/// site, so `produce()` written inside a body that takes `produce` can only
/// be the parameter.
///
/// Conservative on purpose, for the reason AN-029 gives: a lost true edge
/// costs more than a kept false one. Java, Kotlin, C# and Scala resolve a
/// call against methods before variables, Ruby lets the parens force the
/// method, and PHP spells the variable with a sigil the parameter name may
/// not carry — in all of them the call may genuinely name the callable, so
/// the guard stays out.
fn parameters_shadow_callables(file: &std::path::Path) -> bool {
    use crate::models::file_info::Language;
    matches!(
        Language::from_path(file),
        Language::Python
            | Language::JavaScript
            | Language::TypeScript
            | Language::Svelte
            | Language::Rust
            | Language::Go
            | Language::Dart
            | Language::Swift
    )
}

/// A single-candidate lookup's answer, kept only if the languages agree
/// (AN-014). `None` means the strategy declines and the next one may try.
fn accept_interoperable(
    id: &str,
    from_file: Option<&std::path::Path>,
    id_to_entity: &std::collections::HashMap<&str, &CodeEntity>,
) -> Option<String> {
    let target = id_to_entity.get(id)?;
    interoperable(from_file, &target.file_path).then(|| id.to_string())
}

/// Whether a definition in `target_file` may answer a bare name written in
/// `from_file` (AN-014).
///
/// Permissive by default: with no caller context, or against a ghost (whose
/// path is empty and whose language is therefore unknown), the guard steps
/// aside. It only ever *removes* a candidate we have positive evidence
/// against.
///
/// Detection is extension-only rather than `parser::detect_language`: this
/// runs per candidate per edge, and the one language that needs path
/// classification (ansible-deploy, whose `.yml`/`.j2` files nobody owns)
/// lands on `Unknown`, which interoperates with everything anyway.
fn interoperable(from_file: Option<&std::path::Path>, target_file: &std::path::Path) -> bool {
    let Some(from_file) = from_file else {
        return true;
    };
    use crate::models::file_info::Language;
    Language::from_path(from_file).interoperates_with(Language::from_path(target_file))
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics about the dependency graph.
#[derive(Debug, Clone)]
pub struct GraphMetrics {
    pub node_count: usize,
    pub edge_count: usize,
    pub average_degree: f64,
    pub most_connected: Vec<(String, usize)>,
    pub cycle_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Span;

    /// An entity whose ID is `<file>:<line>:<name>`, so tests can assert on
    /// the sorted cycle output without guessing at IDs.
    fn entity(file: &str, line: usize, name: &str, kind: EntityKind) -> CodeEntity {
        CodeEntity::new(name, kind, file, Span::from_positions(line, 0, line, 0))
    }

    /// The `OverfullHead` rule reads one metric, so it is testable directly
    /// rather than through a whole graph.
    #[test]
    fn overfull_head_fires_only_over_the_red_line() {
        let t = crate::models::Thresholds::default();
        let with_ws = |n: Option<u32>| {
            let m = crate::models::entity::EntityMetrics {
                working_set: n,
                ..Default::default()
            };
            overfull_head(&m, &t)
        };
        assert_eq!(with_ws(Some(12)), None, "at the red line is not over it");
        assert_eq!(
            with_ws(Some(13)),
            Some(crate::models::SmellKind::OverfullHead)
        );
        assert_eq!(
            with_ws(None),
            None,
            "a callable the parser could not measure must not be guessed at"
        );
    }

    /// The point of the metric: a body can be over its working-set line while
    /// every control-flow metric reads its best possible value. Before this
    /// existed such a function carried no smell at all.
    #[test]
    fn a_branchless_function_can_still_be_overfull() {
        let t = crate::models::Thresholds::default();
        let m = crate::models::entity::EntityMetrics {
            cyclomatic: Some(1),
            cognitive_complexity: Some(0),
            max_nesting: Some(0),
            working_set: Some(14),
            ..Default::default()
        };
        assert_eq!(
            overfull_head(&m, &t),
            Some(crate::models::SmellKind::OverfullHead)
        );
    }

    /// Build a graph from synthetic entities and `(source, target, kind)`
    /// edges given as indices into `entities`.
    fn graph_of(
        entities: Vec<CodeEntity>,
        edges: &[(usize, usize, RelationshipKind)],
    ) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|&(s, t, kind)| Relationship::new(&entities[s].id, &entities[t].id, kind))
            .collect();
        let result = AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        DependencyGraph::from_analysis(&result)
    }

    /// The point of the count: a statement a parser read and resolved to a
    /// file, with no edge between those two files to show for it, is a hole —
    /// and a hole is what makes a verdict computed over the graph confidently
    /// wrong rather than merely imprecise.
    #[test]
    fn an_import_with_no_edge_behind_it_is_counted_as_missing() {
        let entities = vec![
            entity("a.ts", 1, "callsB", EntityKind::Function),
            entity("b.ts", 1, "fromB", EntityKind::Function),
            entity("c.ts", 1, "fromC", EntityKind::Function),
        ];
        // a -> b landed as a real edge; a -> c did not.
        let mut result = AnalysisResult {
            relationships: vec![Relationship::new(
                &entities[0].id,
                &entities[1].id,
                RelationshipKind::Calls,
            )],
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![import_site("a.ts", "b.ts"), import_site("a.ts", "c.ts")];
        let graph = DependencyGraph::from_analysis(&result);

        assert_eq!(graph.import_coverage(), (1, 2));
    }

    /// Several statements between one pair of files are one dependency, not
    /// three. The count is of edges that should be drawable, because that is
    /// what every consumer downstream reads.
    #[test]
    fn repeated_imports_between_two_files_count_once() {
        let entities = vec![
            entity("a.ts", 1, "callsB", EntityKind::Function),
            entity("b.ts", 1, "fromB", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: Vec::new(),
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![
            import_site("a.ts", "b.ts"),
            import_site("a.ts", "b.ts"),
            import_site("a.ts", "b.ts"),
        ];
        let graph = DependencyGraph::from_analysis(&result);

        assert_eq!(graph.import_coverage(), (0, 1));
    }

    /// A repo-wide figure cannot tell a reader whether the folder they are
    /// being given a verdict about is one of the holed ones. This is the
    /// count that belongs beside the verdict.
    #[test]
    fn unresolved_imports_are_attributable_to_the_folder_they_touch() {
        let entities = vec![
            entity("src/a/one.ts", 1, "usesTwo", EntityKind::Function),
            entity("src/b/two.ts", 1, "fromTwo", EntityKind::Function),
            entity("src/c/three.ts", 1, "fromThree", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: Vec::new(),
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![
            import_site("src/a/one.ts", "src/b/two.ts"),
            import_site("src/c/three.ts", "src/b/two.ts"),
        ];
        let graph = DependencyGraph::from_analysis(&result);

        assert_eq!(graph.import_coverage(), (0, 2));
        assert_eq!(graph.unresolved_imports_touching("src/a"), 1);
        assert_eq!(graph.unresolved_imports_touching("src/c"), 1);
        // Named by both, so it counts both.
        assert_eq!(graph.unresolved_imports_touching("src/b"), 2);
        // A folder none of them touch is not warned about.
        assert_eq!(graph.unresolved_imports_touching("src/d"), 0);
    }

    /// The two false alarms from the field. A folder is scored over its
    /// immediate children, so an import that lives wholly inside one of them
    /// is not an edge in this folder's picture and cannot have changed its
    /// verdict. Counting it told a reader to distrust an answer that was
    /// sound — and the root, under which every path sits, was told to
    /// distrust everything.
    #[test]
    fn an_import_inside_one_child_does_not_taint_the_parents_verdict() {
        let entities = vec![
            entity("src/uid/model/a.ts", 1, "a", EntityKind::Function),
            entity("src/uid/model/b.ts", 1, "b", EntityKind::Function),
            entity("src/uid/generators/x.ts", 1, "x", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: Vec::new(),
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        // Unresolved, and wholly inside `src/uid/model`.
        result.import_sites = vec![import_site("src/uid/model/a.ts", "src/uid/model/b.ts")];
        let graph = DependencyGraph::from_analysis(&result);

        // `model` draws it — a.ts and b.ts are two of its children.
        assert_eq!(graph.unresolved_imports_touching("src/uid/model"), 1);
        // `uid` collapses model to one node, so the edge is inside that node.
        assert_eq!(graph.unresolved_imports_touching("src/uid"), 0);
        // And the root does not inherit every hole beneath it.
        assert_eq!(graph.unresolved_imports_touching("src"), 0);
        // A sibling with no relative imports of its own stays unmarked.
        assert_eq!(graph.unresolved_imports_touching("src/uid/generators"), 0);
    }

    /// An import that crosses two children *is* an edge in the parent's
    /// drawing, and one crossing the boundary changes its doors — both still
    /// count, or the marker would never fire where it matters.
    #[test]
    fn an_import_across_children_or_the_boundary_still_counts() {
        let entities = vec![
            entity("src/uid/model/a.ts", 1, "a", EntityKind::Function),
            entity("src/uid/generators/x.ts", 1, "x", EntityKind::Function),
            entity("src/plugin/p.ts", 1, "p", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: Vec::new(),
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![
            import_site("src/uid/model/a.ts", "src/uid/generators/x.ts"),
            import_site("src/plugin/p.ts", "src/uid/model/a.ts"),
        ];
        let graph = DependencyGraph::from_analysis(&result);

        // Between two of uid's children, and one arriving from outside it.
        assert_eq!(graph.unresolved_imports_touching("src/uid"), 2);
        // At the root only one of them does: model → generators is inside
        // `src/uid`, which the root draws as a single node.
        assert_eq!(graph.unresolved_imports_touching("src"), 1);
    }

    /// A statement the build erases is not a missing dependency. ADR 0026
    /// already keeps those arrows out of every score, and the sentence this
    /// count is printed under is about verdicts being unsound — so counting
    /// one would raise an alarm about something no verdict reads.
    #[test]
    fn a_build_erased_import_is_not_counted_as_missing() {
        let entities = vec![
            entity("a.ts", 1, "usesB", EntityKind::Function),
            entity("b.ts", 1, "fromB", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: Vec::new(),
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        let mut erased = import_site("a.ts", "b.ts");
        erased.is_type_only = true;
        result.import_sites = vec![erased];
        let graph = DependencyGraph::from_analysis(&result);

        // Neither half counts it: no edge is expected, so none is missing.
        assert_eq!(graph.import_coverage(), (0, 0));
    }

    /// A whole graph says nothing, so a sound analysis costs no tokens and
    /// the footer line means something when it does appear.
    #[test]
    fn a_graph_with_every_import_behind_an_edge_reports_no_gap() {
        let entities = vec![
            entity("a.ts", 1, "callsB", EntityKind::Function),
            entity("b.ts", 1, "fromB", EntityKind::Function),
        ];
        let mut result = AnalysisResult {
            relationships: vec![Relationship::new(
                &entities[0].id,
                &entities[1].id,
                RelationshipKind::Calls,
            )],
            entities,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![import_site("a.ts", "b.ts")];
        let graph = DependencyGraph::from_analysis(&result);

        let (landed, seen) = graph.import_coverage();
        assert_eq!((landed, seen), (1, 1));
        assert!(landed >= seen, "a whole graph must not read as holed");
    }

    /// Field report, 2026-08-27: the footer said 74 imports were missing and
    /// that "any verdict over the folders they cross is unsound", then named
    /// no folder — so a reader holding three `fan_out` deltas could not tell
    /// whether the caveat applied to any of them.
    #[test]
    fn the_missing_imports_can_be_attributed_to_folders() {
        let mut result = AnalysisResult {
            entities: Vec::new(),
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![
            // Neither target exists as a file, so neither edge lands.
            import_site("web/ui/a.ts", "web/lib/missing.ts"),
            import_site("web/ui/b.ts", "web/lib/gone.ts"),
            import_site("api/main.rs", "api/absent.rs"),
        ];
        let graph = DependencyGraph::from_analysis(&result);

        let ranked = graph.unresolved_imports_by_folder();
        assert_eq!(
            ranked,
            vec![
                ("web/lib".to_string(), 2),
                ("web/ui".to_string(), 2),
                // Both ends of this one are the same folder: counted once.
                ("api".to_string(), 1),
            ],
            "both ends of every dropped edge are named, worst folder first"
        );
    }

    /// A file at the analysis root has no parent directory, and the folder it
    /// is attributed to must still be printable.
    #[test]
    fn an_unresolved_import_at_the_root_is_attributed_to_the_root() {
        let mut result = AnalysisResult {
            entities: Vec::new(),
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        result.import_sites = vec![import_site("a.ts", "b.ts")];
        let graph = DependencyGraph::from_analysis(&result);

        assert_eq!(
            graph.unresolved_imports_by_folder(),
            vec![(".".to_string(), 1)],
            "one edge inside one folder is counted once, not twice"
        );
    }

    fn import_site(from: &str, to: &str) -> crate::models::ImportSite {
        crate::models::ImportSite {
            from: std::path::PathBuf::from(from),
            to: std::path::PathBuf::from(to),
            line: 0,
            is_reexport: false,
            is_type_only: false,
        }
    }

    #[test]
    fn a_files_own_description_survives_the_path_spelling() {
        use crate::models::file_info::Language;
        use crate::models::FileInfo;

        // The walker hands out `./src/tmp.rs`; entities normalise to
        // `src/tmp.rs`. The lookup has to bridge that, or a file node
        // never finds the header its own file wrote.
        let result = AnalysisResult {
            entities: vec![entity("./src/tmp.rs", 1, "TmpDir", EntityKind::Struct)],
            relationships: Vec::new(),
            files: vec![FileInfo {
                path: std::path::PathBuf::from("./src/tmp.rs"),
                language: Language::Rust,
                size: 0,
                line_count: 1,
                content_hash: None,
                documentation: Some("Scratch directories.".to_string()),
            }],
            import_sites: Vec::new(),
            warnings: Vec::new(),
        };
        let g = DependencyGraph::from_analysis(&result);

        let key = &g.file_metrics()[0].path;
        assert_eq!(key, "src/tmp.rs");
        assert_eq!(
            g.file_documentation(std::path::Path::new(key)),
            Some("Scratch directories.")
        );
        assert_eq!(
            g.file_documentation(std::path::Path::new("./src/tmp.rs")),
            Some("Scratch directories.")
        );
        assert_eq!(
            g.file_documentation(std::path::Path::new("src/other.rs")),
            None
        );
    }

    #[test]
    fn constructor_returning_its_own_type_is_not_a_cycle() {
        // `struct TmpDir` + `impl TmpDir { fn new() -> Self }`: a Contains
        // edge down to the method and a UsesType edge back to the struct.
        // Ordinary Rust, not a design problem — must not be reported (AN-007).
        let g = graph_of(
            vec![
                entity("src/tmp.rs", 1, "TmpDir", EntityKind::Struct),
                entity("src/tmp.rs", 5, "new", EntityKind::Function),
            ],
            &[
                (0, 1, RelationshipKind::Contains),
                (1, 0, RelationshipKind::UsesType),
            ],
        );
        assert!(g.find_cycles().is_empty());
        assert_eq!(g.metrics().cycle_count, 0);
        assert!(g.entities().all(|e| !e.metrics.in_cycle));
    }

    #[test]
    fn trait_methods_referencing_their_trait_are_not_a_cycle() {
        // The three-node shape from the ticket: a trait containing two
        // methods that each take `&self`.
        let g = graph_of(
            vec![
                entity("src/lang.rs", 1, "LanguageParser", EntityKind::Trait),
                entity("src/lang.rs", 3, "language", EntityKind::Method),
                entity("src/lang.rs", 7, "can_parse", EntityKind::Method),
            ],
            &[
                (0, 1, RelationshipKind::Contains),
                (0, 2, RelationshipKind::Contains),
                (1, 0, RelationshipKind::UsesType),
                (2, 0, RelationshipKind::UsesType),
            ],
        );
        assert!(g.find_cycles().is_empty());
    }

    #[test]
    fn mutual_recursion_across_files_is_still_a_cycle() {
        let g = graph_of(
            vec![
                entity("src/ping.rs", 1, "ping", EntityKind::Function),
                entity("src/pong.rs", 2, "pong", EntityKind::Function),
            ],
            &[
                (0, 1, RelationshipKind::Calls),
                (1, 0, RelationshipKind::Calls),
            ],
        );
        // Members sort by entity ID, not traversal order (AN-002).
        assert_eq!(
            g.find_cycles(),
            vec![vec![
                "src/ping.rs:1:ping".to_string(),
                "src/pong.rs:2:pong".to_string(),
            ]]
        );
        assert!(g.entities().all(|e| e.metrics.in_cycle));
        // The file-level rollup reads the same dependency edges, so it must
        // agree with the entity-level flag.
        assert!(g.file_metrics().iter().all(|f| f.metrics.in_cycle));
    }

    #[test]
    fn modules_that_import_each_other_are_still_a_cycle() {
        let g = graph_of(
            vec![
                entity("src/alpha.rs", 1, "alpha", EntityKind::Module),
                entity("src/beta.rs", 1, "beta", EntityKind::Module),
            ],
            &[
                (0, 1, RelationshipKind::Imports),
                (1, 0, RelationshipKind::Imports),
            ],
        );
        assert_eq!(g.find_cycles().len(), 1);
    }

    #[test]
    fn self_recursion_is_not_a_dependency_cycle() {
        // A one-node loop is a different question and out of scope for
        // AN-007 — pinned here so it doesn't leak into the cycle list.
        let g = graph_of(
            vec![entity("src/rec.rs", 1, "walk", EntityKind::Function)],
            &[(0, 0, RelationshipKind::Calls)],
        );
        assert!(g.find_cycles().is_empty());
    }
}

#[cfg(test)]
mod module_path_tests {
    //! AN-006: `module::function` callee resolution.
    //!
    //! A free function in `src/diff.rs` is called as `diff::resolve_git_ref`,
    //! but its `qualified_name` is the bare name and it has no parent entity,
    //! so before AN-006 that callee matched nothing and landed on a ghost —
    //! which is why `impact` under-reported dependents.

    use super::DependencyGraph;
    use crate::analyzer::AnalysisResult;
    use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Span};

    fn entity(name: &str, path: &str, line: usize) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = line;
        CodeEntity::new(name, EntityKind::Function, path, span)
    }

    fn analysis(entities: Vec<CodeEntity>, callee: &str) -> AnalysisResult {
        analysis_from("src/main.rs", entities, callee)
    }

    /// `analysis`, with the calling file named — locality is the whole
    /// question in the ambiguity tests below, and `src/main.rs` is equidistant
    /// from everything.
    fn analysis_from(caller_path: &str, entities: Vec<CodeEntity>, callee: &str) -> AnalysisResult {
        let caller = entity("run_diff", caller_path, 10);
        let mut entities = entities;
        let rel = Relationship::new(caller.id.clone(), callee, RelationshipKind::Calls);
        entities.push(caller);
        AnalysisResult {
            entities,
            relationships: vec![rel],
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn call_target(graph: &DependencyGraph) -> String {
        graph
            .relationships()
            .find(|r| r.kind == RelationshipKind::Calls)
            .map(|r| r.target_id.clone())
            .expect("a call edge")
    }

    #[test]
    fn module_qualified_callee_resolves_to_the_free_function() {
        let target = entity("resolve_git_ref", "src/diff.rs", 306);
        let expected = target.id.clone();
        let graph =
            DependencyGraph::from_analysis(&analysis(vec![target], "diff::resolve_git_ref"));
        assert_eq!(call_target(&graph), expected);
    }

    #[test]
    fn crate_prefixed_module_path_also_resolves() {
        let target = entity("resolve_git_ref", "src/diff.rs", 306);
        let expected = target.id.clone();
        let graph =
            DependencyGraph::from_analysis(&analysis(vec![target], "crate::diff::resolve_git_ref"));
        assert_eq!(call_target(&graph), expected);
    }

    #[test]
    fn ambiguous_module_stem_is_declined_not_guessed() {
        // Two `calls.rs` files under different parser folders both claim
        // `calls::extract_calls`. Picking one would trade the recall bug this
        // ticket fixes for a precision bug, so a caller with no reason to
        // prefer either must decline.
        let a = entity("extract_calls", "src/parser/rust/calls.rs", 24);
        let b = entity("extract_calls", "src/parser/python/calls.rs", 20);
        let graph = DependencyGraph::from_analysis(&analysis(vec![a, b], "calls::extract_calls"));
        assert!(
            call_target(&graph).starts_with("ghost:"),
            "ambiguous stem must not resolve to an arbitrary candidate"
        );
    }

    #[test]
    fn an_ambiguous_stem_resolves_for_a_caller_that_sits_beside_one() {
        // The same collision, asked by someone with a stake in it. Declining
        // here dropped a real edge: `declarations/mod.rs` calling
        // `inference::collect_struct_fields` meant the `inference.rs` in its
        // own parser, and the folder graph lost an entry point because of it.
        let a = entity("extract_calls", "src/parser/rust/calls.rs", 24);
        let expected = a.id.clone();
        let b = entity("extract_calls", "src/parser/python/calls.rs", 20);
        let graph = DependencyGraph::from_analysis(&analysis_from(
            "src/parser/rust/declarations/mod.rs",
            vec![a, b],
            "calls::extract_calls",
        ));
        assert_eq!(call_target(&graph), expected);
    }

    #[test]
    fn a_module_qualified_tie_still_declines_for_an_equidistant_caller() {
        // The caller sits in neither parser, so nearness has nothing to say
        // and the edge must not be invented. This is the half of AN-006's
        // refusal that survives: decline when there is no evidence, not when
        // there is.
        let a = entity("extract_calls", "src/parser/rust/calls.rs", 24);
        let b = entity("extract_calls", "src/parser/python/calls.rs", 20);
        let graph = DependencyGraph::from_analysis(&analysis_from(
            "src/output/json_renderer.rs",
            vec![a, b],
            "calls::extract_calls",
        ));
        assert!(call_target(&graph).starts_with("ghost:"));
    }

    #[test]
    fn mod_rs_takes_its_directory_name() {
        let target = entity("extract_entities", "src/parser/rust/mod.rs", 40);
        let expected = target.id.clone();
        let graph =
            DependencyGraph::from_analysis(&analysis(vec![target], "rust::extract_entities"));
        assert_eq!(call_target(&graph), expected);
    }
}

#[cfg(test)]
mod locality_tests {
    //! AN-011: bare-name call edges bind to the nearest definition.
    //!
    //! The measured failure: every per-parser test helper `fn parse` resolved
    //! to whichever same-named entity was registered first — `src/mcp/push.rs`
    //! — because the bare-name lookup had no locality rule at all.

    use super::DependencyGraph;
    use crate::analyzer::AnalysisResult;
    use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Span};

    pub(super) fn func(name: &str, path: &str, line: usize) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = line;
        CodeEntity::new(name, EntityKind::Function, path, span)
    }

    /// Resolve one `Calls` edge and return the id it bound to. A name that
    /// resolved to nothing comes back as a `ghost:` id, which is how the
    /// AN-014 tests assert a *declined* binding.
    pub(super) fn resolve_call(
        entities: Vec<CodeEntity>,
        caller: &CodeEntity,
        callee: &str,
    ) -> String {
        let rel = Relationship::new(caller.id.clone(), callee, RelationshipKind::Calls);
        let graph = DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships: vec![rel],
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        });
        let target = graph
            .relationships()
            .find(|r| r.kind == RelationshipKind::Calls)
            .map(|r| r.target_id.clone())
            .expect("a call edge");
        target
    }

    #[test]
    fn a_bare_name_binds_to_the_same_file_definition() {
        let caller = func("roundtrips", "src/parser/python/tests.rs", 40);
        let local = func("parse", "src/parser/python/tests.rs", 9);
        let far = func("parse", "src/mcp/push.rs", 100);
        let expected = local.id.clone();
        // `far` first, so first-registered-wins would pick the wrong one.
        let got = resolve_call(vec![far, local, caller.clone()], &caller, "parse");
        assert_eq!(got, expected);
    }

    #[test]
    fn a_bare_name_prefers_the_same_directory_when_no_local_definition() {
        let caller = func("run", "src/parser/python/calls.rs", 20);
        let sibling = func("helper", "src/parser/python/functions.rs", 5);
        let far = func("helper", "src/mcp/push.rs", 100);
        let expected = sibling.id.clone();
        let got = resolve_call(vec![far, sibling, caller.clone()], &caller, "helper");
        assert_eq!(got, expected);
    }

    #[test]
    fn a_bare_name_falls_back_to_the_deepest_shared_path() {
        let caller = func("run", "src/parser/python/calls.rs", 20);
        let nearer = func("shared", "src/parser/rust/helpers.rs", 5);
        let farther = func("shared", "src/mcp/push.rs", 100);
        let expected = nearer.id.clone();
        let got = resolve_call(vec![farther, nearer, caller.clone()], &caller, "shared");
        assert_eq!(got, expected);
    }

    #[test]
    fn an_unambiguous_name_is_unaffected() {
        let caller = func("run", "src/a.rs", 1);
        let only = func("unique_thing", "src/very/far/away.rs", 9);
        let expected = only.id.clone();
        let got = resolve_call(vec![only, caller.clone()], &caller, "unique_thing");
        assert_eq!(got, expected);
    }

    #[test]
    fn ties_resolve_deterministically_by_id() {
        // Two equally-distant candidates: the choice must not depend on entity
        // order, or identical trees would produce different graphs (AN-002).
        let caller = func("run", "src/a/caller.rs", 1);
        let x = func("dup", "src/b/one.rs", 5);
        let y = func("dup", "src/c/two.rs", 5);
        let forward = resolve_call(vec![x.clone(), y.clone(), caller.clone()], &caller, "dup");
        let reversed = resolve_call(vec![y, x, caller.clone()], &caller, "dup");
        assert_eq!(forward, reversed);
    }
}

#[cfg(test)]
mod language_guard_tests {
    //! AN-014: a bare name binds only within its own language family.
    //!
    //! The measured failure came from a polyglot monorepo: a TypeScript
    //! getter declaring `SessionItem[]`, no TypeScript `SessionItem`
    //! anywhere, and a Rust `struct SessionItem` in `backend/` — which
    //! locality happily returned, because something always has to win.

    use super::locality_tests::{func, resolve_call};

    #[test]
    fn a_name_defined_only_in_another_language_becomes_a_ghost() {
        let caller = func("bySide", "frontend/src/vm.ts", 3);
        let rust = func("SessionItem", "backend/vocab-core/src/session.rs", 49);
        let got = resolve_call(vec![rust, caller.clone()], &caller, "SessionItem");
        assert!(
            got.starts_with("ghost:"),
            "expected an unresolved name, got {got}"
        );
    }

    #[test]
    fn a_foreign_candidate_loses_to_a_farther_same_language_one() {
        // The guard has to outrank locality, not merely break its ties: the
        // Rust definition sits in the caller's own directory and still must
        // not win.
        let caller = func("bySide", "app/vm.ts", 3);
        let near_foreign = func("SessionItem", "app/session.rs", 49);
        let far_native = func("SessionItem", "shared/types/session.ts", 7);
        let expected = far_native.id.clone();
        let got = resolve_call(
            vec![near_foreign, far_native, caller.clone()],
            &caller,
            "SessionItem",
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn svelte_binds_to_typescript() {
        // A `.svelte` file's script members *are* TypeScript — same family.
        let caller = func("render", "ui/src/Card.svelte", 4);
        let helper = func("formatDate", "ui/src/lib/date.ts", 1);
        let expected = helper.id.clone();
        let got = resolve_call(vec![helper, caller.clone()], &caller, "formatDate");
        assert_eq!(got, expected);
    }

    #[test]
    fn groovy_binds_to_java() {
        // GR-010 / IM-001 resolve deliberately into the Java entity space.
        let caller = func("execute", "hybris/job.groovy", 4);
        let bean = func("CatalogService", "hybris/CatalogService.java", 1);
        let expected = bean.id.clone();
        let got = resolve_call(vec![bean, caller.clone()], &caller, "CatalogService");
        assert_eq!(got, expected);
    }

    #[test]
    fn an_unclaimed_extension_still_binds() {
        // Unknown interoperates with everything: the generic parser's output
        // and path-classified languages must not start disappearing.
        let caller = func("deploy", "ops/playbook.yml", 2);
        let target = func("render_chart", "src/chart.rs", 8);
        let expected = target.id.clone();
        let got = resolve_call(vec![target, caller.clone()], &caller, "render_chart");
        assert_eq!(got, expected);
    }

    #[test]
    fn the_guard_does_not_disturb_same_language_ties() {
        // AN-002: identical trees, identical graphs, whatever the entity order.
        let caller = func("run", "src/a/caller.rs", 1);
        let x = func("dup", "src/b/one.rs", 5);
        let y = func("dup", "src/c/two.rs", 5);
        let forward = resolve_call(vec![x.clone(), y.clone(), caller.clone()], &caller, "dup");
        let reversed = resolve_call(vec![y, x, caller.clone()], &caller, "dup");
        assert_eq!(forward, reversed);
    }
}

#[cfg(test)]
mod parameter_shadow_tests {
    //! AN-033: a call naming the caller's own parameter is that parameter.
    //!
    //! The measured failure came from a field report: a generic Python cache
    //! helper `def cached(kind, produce)` calling `produce()` bound to an
    //! unrelated nested `def produce()` in another module, and the edge closed
    //! a three-node loop that does not exist in the source. `reshape` then
    //! graded the folder **cyclic** and told the reader to break the loop.

    use super::locality_tests::{func, resolve_call};
    use crate::models::{CodeEntity, Parameter};

    /// `func`, with one parameter of the given name — the whole trigger.
    fn taking(mut f: CodeEntity, param: &str) -> CodeEntity {
        f.parameters.push(Parameter {
            name: param.to_string(),
            ..Default::default()
        });
        f
    }

    #[test]
    fn a_call_naming_the_callers_parameter_does_not_bind_a_stranger() {
        let caller = taking(func("cached", "pkg/cache.py", 3), "produce");
        let stranger = func("produce", "pkg/measures.py", 8);
        let got = resolve_call(vec![stranger, caller.clone()], &caller, "produce");
        assert!(
            got.starts_with("ghost:"),
            "expected the callback parameter to stay unresolved, got {got}"
        );
    }

    #[test]
    fn a_call_naming_no_parameter_still_binds() {
        // The guard must cost nothing to every other call in the same body.
        let caller = taking(func("cached", "pkg/cache.py", 3), "produce");
        let helper = func("cache_key", "pkg/keys.py", 4);
        let expected = helper.id.clone();
        let got = resolve_call(vec![helper, caller.clone()], &caller, "cache_key");
        assert_eq!(got, expected);
    }

    #[test]
    fn a_qualified_call_through_the_parameter_is_left_to_the_other_strategies() {
        // `produce.run()` names a member of the parameter's type, not the
        // parameter. Whatever the ordinary strategies make of it — here a
        // ghost, since a dotted callee nothing registers is treated as
        // external — the guard must not be what decided it.
        let plain = func("cached", "pkg/cache.py", 3);
        let shadowing = taking(plain.clone(), "produce");
        let method = func("run", "pkg/runner.py", 2);
        let with_param = resolve_call(
            vec![method.clone(), shadowing.clone()],
            &shadowing,
            "produce.run",
        );
        let without_param = resolve_call(vec![method, plain.clone()], &plain, "produce.run");
        assert_eq!(with_param, without_param);
    }

    #[test]
    fn a_rust_function_typed_parameter_shadows_too() {
        // AN-029 left exactly this survivor: `json_renderer.rs` calling
        // `rel(path)` where `rel: &F` bound to a free function elsewhere.
        let caller = taking(func("render", "src/output/json_renderer.rs", 40), "rel");
        let stranger = func("rel", "src/mcp/reshape.rs", 100);
        let got = resolve_call(vec![stranger, caller.clone()], &caller, "rel");
        assert!(got.starts_with("ghost:"), "expected a ghost, got {got}");
    }

    #[test]
    fn a_java_parameter_does_not_suppress_the_method_it_shares_a_name_with() {
        // Java resolves a call against methods before variables, so the call
        // may genuinely name the method — suppressing it would lose a true
        // edge, which AN-029 rates the more expensive mistake.
        let caller = taking(func("run", "src/Job.java", 10), "produce");
        let method = func("produce", "src/Factory.java", 5);
        let expected = method.id.clone();
        let got = resolve_call(vec![method, caller.clone()], &caller, "produce");
        assert_eq!(got, expected);
    }
}

#[cfg(test)]
mod reference_edge_tests {
    //! UI-091: reference edges are counted, apart from coupling.
    //!
    //! A document graph's edges are all `References`, which
    //! `RelationshipKind::is_dependency` excludes — so every scope used to
    //! report a fan-out of `0` while the canvas drew arrows leaving it. Zero
    //! is the flattering end of that scale, so an unmeasured folder read as a
    //! perfectly decoupled one.

    use super::DependencyGraph;
    use crate::analyzer::AnalysisResult;
    use crate::models::{
        CodeEntity, EntityKind, Relationship, RelationshipKind, ScopeMetrics, Span,
    };

    fn note(path: &str, name: &str) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = 1;
        CodeEntity::new(name, EntityKind::Note, path, span)
    }

    fn graph_of(
        entities: Vec<CodeEntity>,
        edges: &[(usize, usize, RelationshipKind)],
    ) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|(a, b, k)| {
                Relationship::new(entities[*a].id.clone(), entities[*b].id.clone(), *k)
            })
            .collect();
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn file<'a>(g: &'a DependencyGraph, path: &str) -> &'a ScopeMetrics {
        &g.file_metrics()
            .iter()
            .find(|f| f.path == path)
            .expect("a file rollup")
            .metrics
    }

    fn folder<'a>(g: &'a DependencyGraph, path: &str) -> &'a ScopeMetrics {
        &g.folder_metrics()
            .iter()
            .find(|m| m.path == path)
            .expect("a folder rollup")
            .metrics
    }

    /// `docs/a.md` links its neighbour and one note in another folder.
    fn note_graph() -> DependencyGraph {
        graph_of(
            vec![
                note("docs/a.md", "A"),
                note("docs/b.md", "B"),
                note("guide/c.md", "C"),
            ],
            &[
                (0, 1, RelationshipKind::References),
                (0, 2, RelationshipKind::References),
            ],
        )
    }

    #[test]
    fn a_note_that_links_two_others_reports_no_coupling_and_two_references() {
        let g = note_graph();
        let m = file(&g, "docs/a.md");
        // Unchanged: coupling is still dependency-only, so it is still zero.
        assert_eq!((m.fan_in, m.fan_out), (0, 0));
        assert_eq!((m.internal_edges, m.external_edges), (0, 0));
        // New: and now the graph can say that zero measured nothing.
        assert_eq!(m.ref_fan_out, 2);
        assert_eq!(m.ref_fan_in, 0);
    }

    #[test]
    fn the_linked_note_carries_the_incoming_reference() {
        let g = note_graph();
        assert_eq!(file(&g, "docs/b.md").ref_fan_in, 1);
        assert_eq!(file(&g, "docs/b.md").ref_fan_out, 0);
        assert_eq!(file(&g, "guide/c.md").ref_fan_in, 1);
    }

    #[test]
    fn a_folder_of_notes_counts_only_the_references_that_leave_it() {
        // `docs/a.md -> docs/b.md` stays inside; only the `guide/` link is a
        // reference out of the folder. Same rule the dependency rollup uses.
        let g = note_graph();
        assert_eq!(folder(&g, "docs").ref_fan_out, 1);
        assert_eq!(folder(&g, "docs").ref_fan_in, 0);
        assert_eq!(folder(&g, "guide").ref_fan_in, 1);
        assert_eq!(folder(&g, "guide").ref_fan_out, 0);
    }

    #[test]
    fn cohesion_and_score_are_left_exactly_as_they_were() {
        // The point of the separate tally: references inform the reader,
        // they do not quietly become a second opinion on coupling.
        let g = note_graph();
        assert_eq!(file(&g, "docs/a.md").cohesion, None);
        assert_eq!(file(&g, "docs/a.md").instability, None);
        assert_eq!(folder(&g, "docs").cohesion, None);
        assert!(!folder(&g, "docs").in_cycle);
    }

    #[test]
    fn a_measured_zero_stays_a_zero() {
        // A code graph must be untouched by any of this: real coupling on
        // the dependency counts, nothing on the reference ones.
        let g = graph_of(
            vec![note("src/a.rs", "a"), note("src/b.rs", "b")],
            &[(0, 1, RelationshipKind::Calls)],
        );
        let a = file(&g, "src/a.rs");
        assert_eq!((a.fan_out, a.ref_fan_out), (1, 0));
        let b = file(&g, "src/b.rs");
        assert_eq!((b.fan_in, b.ref_fan_in), (1, 0));
    }
}
