//! Graph representation and manipulation using petgraph.

use crate::analyzer::AnalysisResult;
use crate::models::{CodeEntity, FileMetrics, ModuleMetrics, Relationship, RelationshipKind, EntityKind, ScopeMetrics};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::{HashMap, HashSet};

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
    module_metrics: Vec<ModuleMetrics>,
    /// What each file says it is for, keyed the same way as `file_metrics`.
    /// Prose rather than a rollup, so it sits beside the metrics instead of
    /// inside them, and only files that carry a header appear.
    file_docs: HashMap<String, String>,
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
        self.fan_out.entry(sf.to_string()).or_default().insert(tf.to_string());
        self.fan_in.entry(tf.to_string()).or_default().insert(sf.to_string());
        self.pairs.push((sf.to_string(), tf.to_string()));
    }

    fn fan_in_of(&self, path: &str) -> u32 {
        self.fan_in.get(path).map(|s| s.len() as u32).unwrap_or(0)
    }

    fn fan_out_of(&self, path: &str) -> u32 {
        self.fan_out.get(path).map(|s| s.len() as u32).unwrap_or(0)
    }
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

impl DependencyGraph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_map: HashMap::new(),
            reverse_map: HashMap::new(),
            file_metrics: Vec::new(),
            module_metrics: Vec::new(),
            file_docs: HashMap::new(),
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
    pub fn module_metrics(&self) -> &[ModuleMetrics] {
        &self.module_metrics
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
        let mut typed_method_to_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let id_to_entity: std::collections::HashMap<&str, &CodeEntity> =
            result.entities.iter().map(|e| (e.id.as_str(), e)).collect();

        for entity in &result.entities {
            name_to_ids.entry(entity.name.clone()).or_default().push(entity.id.clone());
            qualified_to_ids.entry(entity.qualified_name.clone()).or_default().push(entity.id.clone());
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
                typed_method_to_id.entry(rust_key).or_insert_with(|| entity.id.clone());
                let java_key = format!("{}.{}", parent_name, entity.name);
                typed_method_to_id.entry(java_key).or_insert_with(|| entity.id.clone());
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
        // Ambiguity is declined, not guessed: two `calls.rs` files under
        // different parser folders both claim `calls::extract_calls`, and
        // picking one would trade this recall bug for a precision bug. A
        // colliding key stores `None` and the lookup skips it, so this pass can
        // only ever add exact matches.
        let mut module_qualified: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        for entity in &result.entities {
            if entity.file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Some(stem) = entity.file_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // `lib.rs` / `main.rs` are crate roots, not modules — nothing is
            // referenced as `lib::foo`. `mod.rs` takes its directory's name.
            let module = match stem {
                "lib" | "main" => continue,
                "mod" => match entity.file_path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
                    Some(dir) => dir,
                    None => continue,
                },
                other => other,
            };
            let key = format!("{}::{}", module, entity.name);
            match module_qualified.get(&key) {
                None => {
                    module_qualified.insert(key, Some(entity.id.clone()));
                }
                Some(Some(existing)) if existing != &entity.id => {
                    module_qualified.insert(key, None);
                }
                Some(_) => {}
            }
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
            // forms of it, resolved against the file-stem index above. Only
            // unambiguous keys answer here — `Some(None)` means two modules
            // claim the name and we decline rather than pick.
            if let Some(id) = module_qualified.get(raw).and_then(|o| o.as_ref()).and_then(accept) {
                return Some(id);
            }
            let segments: Vec<&str> = raw.split("::").collect();
            if segments.len() > 2 {
                let key = format!(
                    "{}::{}",
                    segments[segments.len() - 2],
                    segments[segments.len() - 1]
                );
                if let Some(id) = module_qualified.get(&key).and_then(|o| o.as_ref()).and_then(accept) {
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
            let from_file = id_to_entity
                .get(source_id.as_str())
                .map(|e| e.file_path.as_path());
            let target_id = match resolve(&rel.target_id, from_file) {
                Some(id) => id,
                None => {
                    // Create a ghost entity for the unresolved target
                    let ghost_id = ghosts.entry(rel.target_id.clone()).or_insert_with(|| {
                        let gid = format!("ghost:{}", rel.target_id);
                        let name = rel.target_id.rsplit("::").next()
                            .unwrap_or(&rel.target_id).to_string();
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
            let mut resolved_rel = Relationship::new(&source_id, &target_id, rel.kind)
                .with_weight(rel.weight);
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
            graph: &petgraph::graph::DiGraph<crate::models::CodeEntity, crate::models::Relationship>,
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
                0.25 * cc + 0.15 * cog + 0.25 * fo + 0.15 * loc + 0.10 * params + 0.10 * nest + cycle
            } else {
                let is_enum = e.kind == EntityKind::Enum;
                let field_red = if is_enum { t.variants.bad } else { t.fields.bad };
                let fields = norm(m.field_count.unwrap_or(0) as f32, field_red);
                let methods = norm(m.method_count as f32, t.method_count.bad);
                let loc = norm(m.loc as f32, t.loc_container.bad);
                let fo = norm(m.fan_out as f32, t.fan_out.bad);
                let encaps = if m.method_count > 3 {
                    norm(m.public_field_ratio.unwrap_or(0.0), t.public_field_ratio.bad)
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
            let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) else { continue };
            if src == tgt { continue; }
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
                if fc > t.god_class_fields && mc > t.god_class_methods
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

                let total = outgoing_total.get(&idx).copied().unwrap_or(0) as usize;
                if total >= t.feature_envy_min_edges as usize {
                    if let Some(by_parent) = outgoing_by_parent.get(&idx) {
                        let own_parent = &e.parent_id;
                        for (target_parent, &count) in by_parent {
                            if target_parent == own_parent { continue; }
                            if target_parent.is_none() { continue; }
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
        let modules = Self::compute_module_metrics(&tally, &edges, &cycles);
        self.file_metrics = files;
        self.module_metrics = modules;
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
            if s > max { max = s; }
            if s <= 0.5 { ok += 1; }
            else if s <= 1.0 { warn += 1; }
            else { bad += 1; }
        }
        let avg = if total_loc > 0 { weighted_sum / total_loc as f32 } else { 0.0 };
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
        };
        let mut max_end_line: HashMap<String, usize> = HashMap::new();

        for idx in self.graph.node_indices() {
            let e = &self.graph[idx];
            // Synthetic entities (Parameter, Branch, Loop, local_var
            // Variables) are rendering aids, not real program structure
            // — exclude from per-file counts.
            if e.kind == EntityKind::Parameter
                || e.kind == EntityKind::Branch
                || e.kind == EntityKind::Loop
                || e.tags.contains("local_var")
            {
                continue;
            }
            let path = Self::normalize_path(&e.file_path);
            if path.is_empty() {
                continue;
            }
            tally.entity_file.insert(idx, path.clone());
            *tally.entity_count.entry(path.clone()).or_insert(0) += 1;
            if e.kind.is_callable() {
                *tally.callable_count.entry(path.clone()).or_insert(0) += 1;
            }
            if e.kind.is_container() {
                *tally.container_count.entry(path.clone()).or_insert(0) += 1;
            }
            tally.scores.entry(path.clone()).or_default().push((e.metrics.composite_score, e.metrics.loc));
            let slot = max_end_line.entry(path).or_insert(0);
            if e.span.end.line > *slot {
                *slot = e.span.end.line;
            }
        }
        for (path, &max_line) in &max_end_line {
            tally.loc.insert(path.clone(), (max_line + 1) as u32);
        }
        tally
    }

    /// Phase 2: scan dependency edges and bucket them per file.
    fn scan_file_edges(&self, entity_file: &HashMap<NodeIndex, String>) -> FileEdgeData {
        let mut data = FileEdgeData { deps: EdgeBuckets::default(), refs: EdgeBuckets::default() };
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
            let Some((src, tgt)) = self.graph.edge_endpoints(edge_idx) else { continue };
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
            let cohesion = if total == 0 { None } else { Some(internal as f32 / total as f32) };
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
                instability: if fi + fo > 0 { Some(fo as f32 / (fi + fo) as f32) } else { None },
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
            files.push(FileMetrics { path: path.clone(), metrics });
        }
        files
    }

    /// Phase 5: roll file-level data up to directory (module) granularity.
    /// Phase 5: roll file-level data up to directory (module) granularity.
    fn compute_module_metrics(
        tally: &FileTally,
        edges: &FileEdgeData,
        cycles: &HashSet<String>,
    ) -> Vec<ModuleMetrics> {
        let module_paths = Self::enumerate_module_paths(tally);
        if module_paths.is_empty() {
            return Vec::new();
        }

        let mut sorted: Vec<String> = module_paths.into_iter().collect();
        sorted.sort();
        let mut modules = Vec::with_capacity(sorted.len());

        for mp in &sorted {
            let descendants: Vec<&String> = tally.entity_count.keys()
                .filter(|f| Self::is_descendant(mp, f))
                .collect();

            let mut m = Self::aggregate_file_counts(tally, &descendants);
            let (internal, external, fan_in, fan_out) =
                Self::classify_module_edges(&edges.deps, &descendants);
            // Same classification over the reference bucket. Only the fan
            // counts are kept: references feed no ratio and no score, they
            // exist so the reader can tell an unmeasured scope from a
            // decoupled one (UI-091).
            let (_, _, ref_fan_in, ref_fan_out) =
                Self::classify_module_edges(&edges.refs, &descendants);
            m.ref_fan_in = ref_fan_in;
            m.ref_fan_out = ref_fan_out;
            m.internal_edges = internal;
            m.external_edges = external;
            let total = internal + external;
            m.cohesion = if total == 0 { None } else { Some(internal as f32 / total as f32) };
            m.fan_in = fan_in;
            m.fan_out = fan_out;
            m.instability = if fan_in + fan_out > 0 {
                Some(fan_out as f32 / (fan_in + fan_out) as f32)
            } else {
                None
            };
            m.in_cycle = descendants.iter().any(|f| cycles.contains(*f))
                && m.fan_out > 0
                && m.fan_in > 0;

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
            modules.push(ModuleMetrics { path: mp.clone(), metrics: m });
        }
        modules
    }

    /// Parent directory of a file path (everything before the last separator).
    fn parent_dir(file_path: &str) -> String {
        match file_path.rfind(std::path::MAIN_SEPARATOR) {
            Some(i) => file_path[..i].to_string(),
            None => String::new(),
        }
    }

    /// Does `file_path` live under `module_path` (possibly in a subdirectory)?
    fn is_descendant(module_path: &str, file_path: &str) -> bool {
        if module_path.is_empty() { return true; }
        file_path.strip_prefix(module_path)
            .map_or(false, |rest| rest.starts_with(std::path::MAIN_SEPARATOR))
    }

    /// Compute the longest common directory prefix across all file paths,
    /// then enumerate every ancestor directory down to (and including) that
    /// prefix. Returns the full set of module paths.
    fn enumerate_module_paths(tally: &FileTally) -> HashSet<String> {
        use std::path::{Path, PathBuf};

        // LCP at the component level so `src/parser/` and `src/parsed/`
        // correctly reduce to `src` instead of `""`.
        let lcp = {
            let mut iter = tally.entity_count.keys();
            match iter.next() {
                None => return HashSet::new(),
                Some(first) => {
                    let mut prefix: Vec<_> = Path::new(&Self::parent_dir(first))
                        .components()
                        .map(|c| c.as_os_str().to_owned())
                        .collect();
                    for path in iter {
                        let comps: Vec<_> = Path::new(&Self::parent_dir(path))
                            .components()
                            .map(|c| c.as_os_str().to_owned())
                            .collect();
                        let keep = prefix.iter().zip(comps.iter())
                            .take_while(|(a, b)| a == b)
                            .count();
                        prefix.truncate(keep);
                    }
                    let mut buf = PathBuf::new();
                    for c in &prefix { buf.push(c); }
                    buf.display().to_string()
                }
            }
        };

        let mut paths: HashSet<String> = HashSet::new();
        paths.insert(lcp.clone());
        for file_path in tally.entity_count.keys() {
            let mut cur = Self::parent_dir(file_path);
            loop {
                paths.insert(cur.clone());
                if cur == lcp || cur.is_empty() { break; }
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
    fn classify_module_edges(
        edges: &EdgeBuckets,
        descendants: &[&String],
    ) -> (u32, u32, u32, u32) {
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
        (internal, external, fan_in_set.len() as u32, fan_out_set.len() as u32)
    }

    /// Re-number call order metadata so they're sequential (1, 2, 3, ...)
    /// per source entity, eliminating gaps from filtered-out calls.
    fn renumber_call_orders(&mut self) {
        use std::collections::HashMap;

        // Group edge indices by source node, collecting (original_order, edge_index)
        let mut source_edges: HashMap<NodeIndex, Vec<(u32, petgraph::graph::EdgeIndex)>> = HashMap::new();

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
            RelationshipKind::UsesType | RelationshipKind::Returns | RelationshipKind::Instantiates => EntityKind::Struct,
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
            "int" | "str" | "float" | "bool" | "bytes" | "bytearray"
                | "list" | "dict" | "set" | "tuple" | "frozenset"
                | "complex" | "type" | "object" | "None"
        ) {
            return Some(EntityKind::Class);
        }
        // Rust primitives and common owned / smart-pointer / collection
        // types — all Structs in Rust's model.
        if matches!(
            last,
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
                | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
                | "f32" | "f64" | "char"
                | "String" | "Vec" | "Option" | "Result" | "Box" | "Rc"
                | "Arc" | "Cow" | "HashMap" | "HashSet" | "BTreeMap"
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
            "str", "bool", "char",
            "u8", "u16", "u32", "u64", "u128", "usize",
            "i8", "i16", "i32", "i64", "i128", "isize",
            "f32", "f64",
            // Common owned / smart-pointer types
            "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "Cow",
            "HashMap", "HashSet", "BTreeMap", "BTreeSet", "PhantomData",
            // Widespread traits
            "Display", "Debug", "Clone", "Copy", "Default",
            "Iterator", "IntoIterator",
            "From", "Into", "TryFrom", "TryInto", "AsRef", "AsMut",
            "Send", "Sync", "Sized", "Drop", "Fn", "FnMut", "FnOnce",
            "Eq", "PartialEq", "Ord", "PartialOrd", "Hash",
            "Read", "Write", "Seek", "BufRead",
            "ToString", "ToOwned", "Borrow", "BorrowMut",
            // Bang-macros commonly called bare
            "println", "eprintln", "format", "panic", "assert", "assert_eq",
            "todo", "unimplemented", "unreachable",
        ];
        let last_segment = name.rsplit("::").next().unwrap_or(name);
        if BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        // Python builtins — keep in sync with python_parser::is_bare_builtin.
        const PY_BUILTINS: &[&str] = &[
            // Printing, I/O, introspection
            "print", "input", "open", "format", "repr", "vars", "dir", "help",
            "locals", "globals",
            // Collection constructors / coercions
            "len", "range", "str", "int", "float", "list", "dict", "set",
            "tuple", "frozenset", "bool", "bytes", "bytearray", "complex",
            // Iteration
            "iter", "next", "map", "filter", "zip", "enumerate",
            "sorted", "reversed", "any", "all",
            // Numeric
            "abs", "round", "min", "max", "sum", "pow", "divmod",
            "hex", "oct", "bin", "ord", "chr", "ascii", "hash", "id",
            // Type system
            "type", "super", "object", "callable",
            "isinstance", "issubclass",
            "hasattr", "getattr", "setattr", "delattr",
            "staticmethod", "classmethod", "property",
            // Metaprogramming
            "compile", "eval", "exec", "breakpoint", "exit", "quit",
            // Exceptions
            "Exception", "BaseException", "ValueError", "TypeError",
            "KeyError", "IndexError", "AttributeError", "RuntimeError",
            "StopIteration", "StopAsyncIteration", "NotImplementedError",
            "FileNotFoundError", "OSError", "IOError", "ZeroDivisionError",
            "ArithmeticError", "AssertionError", "LookupError", "NameError",
            "UnicodeError", "UnicodeDecodeError", "UnicodeEncodeError",
            // Common stdlib module attributes that sometimes surface
            "None", "True", "False", "NotImplemented", "Ellipsis",
        ];
        if PY_BUILTINS.contains(&last_segment) {
            return "ghost_stdlib";
        }
        // JS/TS builtins
        const JS_BUILTINS: &[&str] = &[
            "console", "Math", "JSON", "Promise", "Array", "Object", "Map", "Set",
            "Date", "Error", "RegExp", "Symbol", "Number", "Boolean",
            "setTimeout", "setInterval", "fetch", "parseInt", "parseFloat",
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
        let mut connections: Vec<(String, usize)> = self.node_map
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
    let file_of = |id: &&String| id_to_entity.get(id.as_str()).map(|e| e.file_path.clone());

    // AN-014: a candidate in an unrelated language is not a candidate.
    // Locality ranks what survives; it cannot tell a TypeScript
    // `SessionItem` from a Rust one, because path distance is all it sees.
    let candidates = interoperable_candidates(candidates, from_file, id_to_entity);
    if candidates.len() <= 1 {
        return candidates.first().map(|id| (*id).clone());
    }
    let Some(from_file) = from_file else {
        return candidates.first().map(|id| (*id).clone());
    };

    if let Some(id) = candidates
        .iter()
        .find(|id| file_of(id).as_deref() == Some(from_file))
    {
        return Some((*id).clone());
    }
    if let Some(from_dir) = from_file.parent() {
        if let Some(id) = candidates
            .iter()
            .find(|id| file_of(id).as_deref().and_then(|p| p.parent()) == Some(from_dir))
        {
            return Some((*id).clone());
        }
    }
    let from_components: Vec<_> = from_file.components().collect();
    candidates
        .iter()
        .max_by_key(|id| {
            file_of(id)
                .map(|p| {
                    p.components()
                        .zip(from_components.iter())
                        .take_while(|(a, b)| a == *b)
                        .count()
                })
                .unwrap_or(0)
        })
        .or_else(|| candidates.first())
        .map(|id| (*id).clone())
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
            warnings: Vec::new(),
        };
        DependencyGraph::from_analysis(&result)
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
        assert_eq!(g.file_documentation(std::path::Path::new("src/other.rs")), None);
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
        let caller = entity("run_diff", "src/main.rs", 10);
        let mut entities = entities;
        let rel = Relationship::new(caller.id.clone(), callee, RelationshipKind::Calls);
        entities.push(caller);
        AnalysisResult {
            entities,
            relationships: vec![rel],
            files: Vec::new(),
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
        let graph = DependencyGraph::from_analysis(&analysis(vec![target], "diff::resolve_git_ref"));
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
        // ticket fixes for a precision bug, so the lookup must decline.
        let a = entity("extract_calls", "src/parser/rust/calls.rs", 24);
        let b = entity("extract_calls", "src/parser/python/calls.rs", 20);
        let graph = DependencyGraph::from_analysis(&analysis(vec![a, b], "calls::extract_calls"));
        assert!(
            call_target(&graph).starts_with("ghost:"),
            "ambiguous stem must not resolve to an arbitrary candidate"
        );
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
    use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, ScopeMetrics, Span};

    fn note(path: &str, name: &str) -> CodeEntity {
        let mut span = Span::default();
        span.start.line = 1;
        CodeEntity::new(name, EntityKind::Note, path, span)
    }

    fn graph_of(entities: Vec<CodeEntity>, edges: &[(usize, usize, RelationshipKind)]) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|(a, b, k)| Relationship::new(entities[*a].id.clone(), entities[*b].id.clone(), *k))
            .collect();
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn file<'a>(g: &'a DependencyGraph, path: &str) -> &'a ScopeMetrics {
        &g.file_metrics().iter().find(|f| f.path == path).expect("a file rollup").metrics
    }

    fn module<'a>(g: &'a DependencyGraph, path: &str) -> &'a ScopeMetrics {
        &g.module_metrics().iter().find(|m| m.path == path).expect("a module rollup").metrics
    }

    /// `docs/a.md` links its neighbour and one note in another folder.
    fn note_graph() -> DependencyGraph {
        graph_of(
            vec![note("docs/a.md", "A"), note("docs/b.md", "B"), note("guide/c.md", "C")],
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
        assert_eq!(module(&g, "docs").ref_fan_out, 1);
        assert_eq!(module(&g, "docs").ref_fan_in, 0);
        assert_eq!(module(&g, "guide").ref_fan_in, 1);
        assert_eq!(module(&g, "guide").ref_fan_out, 0);
    }

    #[test]
    fn cohesion_and_score_are_left_exactly_as_they_were() {
        // The point of the separate tally: references inform the reader,
        // they do not quietly become a second opinion on coupling.
        let g = note_graph();
        assert_eq!(file(&g, "docs/a.md").cohesion, None);
        assert_eq!(file(&g, "docs/a.md").instability, None);
        assert_eq!(module(&g, "docs").cohesion, None);
        assert!(!module(&g, "docs").in_cycle);
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
