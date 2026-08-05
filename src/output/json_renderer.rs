//! JSON output renderer.

use super::{OutputFormat, Renderer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{EntityKind, RelationshipKind};
use anyhow::Result;
use serde::Serialize;

pub struct JsonRenderer;

impl Renderer for JsonRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Json
    }
    
    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String> {
        // Include Parameter entities so the UI can show method arguments
        // as graph nodes when a callable is focused. They're hidden by
        // default via the displayPlan; the filter panel can toggle them.
        let entities: Vec<JsonEntity> = graph
            .entities()
            .map(|e| JsonEntity::from_entity(e, &config.root_path))
            .collect();

        let entity_ids: std::collections::HashSet<&str> =
            entities.iter().map(|e| e.id.as_str()).collect();

        // Build a lookup from entity ID → language so relationships can
        // carry a language-aware label (e.g., "defines" in Rust, "declares"
        // in Java). Defaults to Unknown for IDs not found — the label falls
        // back to the generic form.
        use crate::models::file_info::Language;
        let entity_language: std::collections::HashMap<&str, Language> = graph
            .entities()
            .map(|e| (e.id.as_str(), Language::from_extension(
                e.file_path.extension().and_then(|x| x.to_str()).unwrap_or(""),
            )))
            .collect();

        // Include TakesParam relationships alongside the Parameter entities.
        // Filter out Contains edges that target a Parameter — TakesParam is
        // the more specific and meaningful relationship, and showing both
        // stacks "defines" on top of "takes param" visually.
        let param_ids: std::collections::HashSet<&str> = graph
            .entities()
            .filter(|e| e.kind == EntityKind::Parameter)
            .map(|e| e.id.as_str())
            .collect();
        let relationships: Vec<JsonRelationship> = graph
            .relationships()
            .filter(|r| {
                // Drop Contains edges targeting parameters (TakesParam covers this).
                if r.kind == RelationshipKind::Contains && param_ids.contains(r.target_id.as_str()) {
                    return false;
                }
                entity_ids.contains(r.source_id.as_str()) && entity_ids.contains(r.target_id.as_str())
            })
            .map(|r| {
                let lang = entity_language
                    .get(r.source_id.as_str())
                    .copied()
                    .unwrap_or(Language::Unknown);
                JsonRelationship::from_relationship(r, lang)
            })
            .collect();

        // Project-root-relative display paths for files and modules.
        // The graph stores full paths; stripping here keeps the UI stable
        // across build machines.
        let root = &config.root_path;
        let rel = |raw: &str| -> String {
            use std::path::Path;
            let p = Path::new(raw);
            match p.strip_prefix(root) {
                Ok(r) => r.display().to_string(),
                Err(_) => raw.to_string(),
            }
        };

        let files: Vec<JsonScopeMetrics> = graph
            .file_metrics()
            .iter()
            .map(|f| JsonScopeMetrics::from_file(&f.path, &f.metrics, &rel))
            .collect();
        let modules: Vec<JsonScopeMetrics> = graph
            .module_metrics()
            .iter()
            .map(|m| JsonScopeMetrics::from_file(&m.path, &m.metrics, &rel))
            .collect();

        let output = JsonOutput {
            metadata: OutputMetadata {
                version: env!("CARGO_PKG_VERSION").to_string(),
                generated_at: chrono_lite_timestamp(),
                root_path: config.root_path.display().to_string(),
            },
            entities,
            relationships,
            metrics: graph.metrics().into(),
            files,
            modules,
            thresholds: crate::models::Thresholds::default(),
        };

        let json = serde_json::to_string(&output)?;
        Ok(json)
    }
}

#[derive(Serialize)]
struct JsonOutput {
    metadata: OutputMetadata,
    entities: Vec<JsonEntity>,
    relationships: Vec<JsonRelationship>,
    metrics: JsonMetrics,
    files: Vec<JsonScopeMetrics>,
    modules: Vec<JsonScopeMetrics>,
    thresholds: crate::models::Thresholds,
}

/// File- or module-level rollup for JSON. Path is project-root-relative.
#[derive(Serialize)]
struct JsonScopeMetrics {
    path: String,
    entity_count: u32,
    callable_count: u32,
    container_count: u32,
    loc: u32,
    internal_edges: u32,
    external_edges: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    cohesion: Option<f32>,
    fan_in: u32,
    fan_out: u32,
    in_cycle: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instability: Option<f32>,
    avg_quality: f32,
    max_quality: f32,
    quality_ok: u32,
    quality_warn: u32,
    quality_bad: u32,
    composite_score: f32,
}

impl JsonScopeMetrics {
    fn from_file<F: Fn(&str) -> String>(
        path: &str,
        m: &crate::models::ScopeMetrics,
        rel: &F,
    ) -> Self {
        Self {
            path: rel(path),
            entity_count: m.entity_count,
            callable_count: m.callable_count,
            container_count: m.container_count,
            loc: m.loc,
            internal_edges: m.internal_edges,
            external_edges: m.external_edges,
            cohesion: m.cohesion,
            fan_in: m.fan_in,
            fan_out: m.fan_out,
            in_cycle: m.in_cycle,
            instability: m.instability,
            avg_quality: m.avg_quality,
            max_quality: m.max_quality,
            quality_ok: m.quality_ok,
            quality_warn: m.quality_warn,
            quality_bad: m.quality_bad,
            composite_score: m.composite_score,
        }
    }
}

/// Slim entity for the graph JSON — omits source_code, impl_blocks, fields
/// which are only needed in the detail panel and served separately.
#[derive(Serialize)]
struct JsonEntity {
    id: String,
    name: String,
    qualified_name: String,
    kind: String,
    visibility: String,
    file_path: String,
    span: crate::models::Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    parameters: Vec<JsonParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
    implements: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extends: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    /// Parser-emitted `key:value` attribute strings (e.g. `k8s_kind:…`,
    /// `bean:…`, `caught:…`). Small and needed by the UI to render the
    /// structured detail panel, so — unlike source_code/fields — they
    /// ride the graph JSON rather than the separate detail payload.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<String>,
    metrics: JsonEntityMetrics,
}

#[derive(Serialize)]
struct JsonEntityMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    cyclomatic: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cognitive_complexity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_nesting: Option<u32>,
    loc: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    param_count: Option<u32>,
    fan_in: u32,
    fan_out: u32,
    in_cycle: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    field_count: Option<u32>,
    method_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_field_ratio: Option<f32>,
    composite_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    instability: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_complexity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wmc: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chain_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pagerank: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    smells: Vec<String>,
}

#[derive(Serialize)]
struct JsonParam {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_value: Option<String>,
}

#[derive(Serialize)]
struct JsonRelationship {
    source_id: String,
    target_id: String,
    kind: String,
    /// Language-aware display label (e.g., "defines" in Rust, "declares" in Java).
    label: String,
    /// Passive / incoming form (e.g., "defined in" for Rust Contains).
    /// Used when viewing from the target's perspective.
    incoming_label: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    metadata: std::collections::HashMap<String, String>,
    /// Resolution precision (AN-004): `exact` when a language server resolved
    /// the call site, `heuristic` when nao's name-based resolver did. Omitted
    /// where precision doesn't apply. Exposed here for AN-005: an unchanged
    /// target between an exact-mode and a heuristic-mode run is otherwise
    /// ambiguous — it could mean the oracle confirmed the guess, or that the
    /// oracle never resolved that call site at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    precision: Option<String>,
}

impl JsonEntity {
    fn from_entity(e: &crate::models::CodeEntity, root_path: &std::path::Path) -> Self {
        let file_path = e
            .file_path
            .strip_prefix(root_path)
            .unwrap_or(&e.file_path)
            .display()
            .to_string();
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            qualified_name: e.qualified_name.clone(),
            kind: e.kind.display_name().to_string(),
            visibility: format!("{:?}", e.visibility).to_lowercase(),
            file_path,
            span: e.span,
            parent_id: e.parent_id.clone(),
            parameters: e
                .parameters
                .iter()
                .map(|p| JsonParam {
                    name: p.name.clone(),
                    type_name: p.type_name.clone(),
                    default_value: p.default_value.clone(),
                })
                .collect(),
            return_type: e.return_type.clone(),
            implements: e.implements.clone(),
            extends: e.extends.clone(),
            tags: e.tags.iter().cloned().collect(),
            attributes: e.attributes.clone(),
            metrics: JsonEntityMetrics {
                cyclomatic: e.metrics.cyclomatic,
                cognitive_complexity: e.metrics.cognitive_complexity,
                max_nesting: e.metrics.max_nesting,
                loc: e.metrics.loc,
                param_count: e.metrics.param_count,
                fan_in: e.metrics.fan_in,
                fan_out: e.metrics.fan_out,
                in_cycle: e.metrics.in_cycle,
                field_count: e.metrics.field_count,
                method_count: e.metrics.method_count,
                public_field_ratio: e.metrics.public_field_ratio,
                composite_score: e.metrics.composite_score,
                instability: e.metrics.instability,
                return_complexity: e.metrics.return_complexity,
                wmc: e.metrics.wmc,
                chain_depth: e.metrics.chain_depth,
                pagerank: e.metrics.pagerank,
                smells: e.metrics.smells.iter().map(|s| {
                    serde_json::to_value(s)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| format!("{:?}", s).to_lowercase())
                }).collect(),
            },
        }
    }
}

impl JsonRelationship {
    fn from_relationship(
        r: &crate::models::Relationship,
        source_language: crate::models::file_info::Language,
    ) -> Self {
        // Use serde to get the snake_case name matching the frontend's REL_KIND_MAP
        let kind = serde_json::to_value(&r.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{:?}", r.kind).to_lowercase());
        let label = r.kind.display_label_for(source_language).to_string();
        let incoming_label = r.kind.incoming_label_for(source_language).to_string();
        Self {
            source_id: r.source_id.clone(),
            target_id: r.target_id.clone(),
            kind,
            label,
            incoming_label,
            metadata: r.metadata.clone(),
            precision: r.precision.map(|p| p.marker().to_string()),
        }
    }
}

#[derive(Serialize)]
struct OutputMetadata {
    version: String,
    generated_at: String,
    root_path: String,
}

#[derive(Serialize)]
struct JsonMetrics {
    node_count: usize,
    edge_count: usize,
    average_degree: f64,
    most_connected: Vec<MostConnectedEntry>,
    cycle_count: usize,
}

#[derive(Serialize)]
struct MostConnectedEntry {
    entity_id: String,
    connections: usize,
}

impl From<crate::graph::GraphMetrics> for JsonMetrics {
    fn from(metrics: crate::graph::GraphMetrics) -> Self {
        Self {
            node_count: metrics.node_count,
            edge_count: metrics.edge_count,
            average_degree: metrics.average_degree,
            most_connected: metrics
                .most_connected
                .into_iter()
                .map(|(id, count)| MostConnectedEntry {
                    entity_id: id,
                    connections: count,
                })
                .collect(),
            cycle_count: metrics.cycle_count,
        }
    }
}

/// Detail data for a single entity (source_code, fields, impl_blocks).
/// Keyed by entity ID in the sidecar file.
#[derive(Serialize)]
struct EntityDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_code: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<JsonField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    impl_blocks: Vec<String>,
}

#[derive(Serialize)]
struct JsonField {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
}

/// A single node in the path index tree (folder or file).
#[derive(Serialize)]
struct IndexNode {
    /// Path relative to root. Empty string for the root node.
    path: String,
    /// 'folder' or 'file'
    #[serde(rename = "type")]
    node_type: &'static str,
    /// Aggregated entity count (recursive for folders)
    entity_count: usize,
    /// Aggregated relationship count where both endpoints are under this path
    relationship_count: usize,
    /// Distinct languages present at this scope (sorted, unique).
    /// For files this is a single language; for folders the union of descendants.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    languages: Vec<String>,
    /// Child paths (only populated for folders)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<String>,
}

#[derive(Serialize)]
struct IndexOutput {
    /// Root path string (matches `IndexNode.path == ""`)
    root: String,
    /// Flat map: path -> IndexNode (for efficient client-side lookup)
    nodes: std::collections::BTreeMap<String, IndexNode>,
    /// Total entity count across the whole codebase (parameters excluded)
    total_entities: usize,
    /// Total relationship count (TakesParam excluded)
    total_relationships: usize,
}

impl JsonRenderer {
    /// Render an index file describing the file/folder hierarchy with aggregated
    /// entity and relationship counts per scope. Used by the frontend to display
    /// a tree and gate visualization of large scopes.
    pub fn render_index(graph: &DependencyGraph, config: &Config) -> Result<String> {
        use crate::models::file_info::Language;
        use std::collections::{BTreeMap, HashMap, HashSet};

        // Per-file entity count (Parameters excluded)
        let mut entities_per_file: HashMap<String, usize> = HashMap::new();
        // Per-entity file path (for relationship scoping)
        let mut entity_file: HashMap<String, String> = HashMap::new();
        // Per-file language (derived from extension)
        let mut file_language: HashMap<String, String> = HashMap::new();

        for e in graph.entities() {
            if e.kind == EntityKind::Parameter
                || e.kind == EntityKind::Branch
                || e.kind == EntityKind::Loop
                || e.tags.contains("local_var")
            {
                continue;
            }
            // Skip ghost entities (no real file path)
            if e.tags.contains("ghost") {
                continue;
            }
            let rel_path = e
                .file_path
                .strip_prefix(&config.root_path)
                .unwrap_or(&e.file_path)
                .display()
                .to_string();
            *entities_per_file.entry(rel_path.clone()).or_insert(0) += 1;
            entity_file.insert(e.id.clone(), rel_path.clone());
            file_language.entry(rel_path).or_insert_with(|| {
                let ext = e
                    .file_path
                    .extension()
                    .and_then(|x| x.to_str())
                    .unwrap_or("");
                Language::from_extension(ext).display_name().to_string()
            });
        }

        // Per-file intra-file relationship count (both endpoints in same file).
        // We count cross-file relationships only at folder levels that contain both files.
        // To avoid O(folders × rels) during aggregation, we pre-compute:
        //   - intra_file_rels[file] += 1 if both endpoints are in same file
        //   - cross_rels: Vec<(file_a, file_b)> for cross-file rels
        let mut intra_file_rels: HashMap<String, usize> = HashMap::new();
        let mut cross_rels: Vec<(String, String)> = Vec::new();

        for r in graph.relationships() {
            if r.kind == RelationshipKind::TakesParam {
                continue;
            }
            let src_file = entity_file.get(&r.source_id);
            let tgt_file = entity_file.get(&r.target_id);
            match (src_file, tgt_file) {
                (Some(a), Some(b)) if a == b => {
                    *intra_file_rels.entry(a.clone()).or_insert(0) += 1;
                }
                (Some(a), Some(b)) => {
                    cross_rels.push((a.clone(), b.clone()));
                }
                _ => {}
            }
        }

        // Collect all paths (files + their ancestor folders)
        let mut all_paths: HashSet<String> = HashSet::new();
        all_paths.insert(String::new()); // root
        for file in entities_per_file.keys() {
            all_paths.insert(file.clone());
            let mut current = file.as_str();
            while let Some(pos) = current.rfind('/') {
                current = &current[..pos];
                all_paths.insert(current.to_string());
            }
        }

        // Determine parent/child relationships and node types.
        // A path is a 'file' if it's in entities_per_file, else 'folder'.
        let mut children_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in &all_paths {
            if path.is_empty() {
                continue;
            }
            let parent = match path.rfind('/') {
                Some(pos) => path[..pos].to_string(),
                None => String::new(),
            };
            children_map.entry(parent).or_default().push(path.clone());
        }
        // Sort children: folders first, then files, each group alphabetical.
        for children in children_map.values_mut() {
            children.sort_by(|a, b| {
                let a_is_file = entities_per_file.contains_key(a);
                let b_is_file = entities_per_file.contains_key(b);
                a_is_file.cmp(&b_is_file).then_with(|| a.cmp(b))
            });
        }

        // Direct rel count per path: intra-file for files, cross-file LCA for folders
        let mut direct_rel_count: HashMap<String, usize> = HashMap::new();
        for (file, _) in &entities_per_file {
            direct_rel_count.insert(file.clone(), intra_file_rels.get(file).copied().unwrap_or(0));
        }
        for (a, b) in &cross_rels {
            let lca = lowest_common_ancestor(a, b);
            *direct_rel_count.entry(lca).or_insert(0) += 1;
        }

        // Post-order DFS aggregation: each folder gets processed strictly after all
        // its descendants. This guarantees children have correct counts when we sum them.
        let mut entity_agg: HashMap<String, usize> = HashMap::new();
        let mut final_rel_agg: HashMap<String, usize> = HashMap::new();
        let mut lang_agg: HashMap<String, HashSet<String>> = HashMap::new();

        fn aggregate(
            path: &str,
            children_map: &BTreeMap<String, Vec<String>>,
            entities_per_file: &HashMap<String, usize>,
            direct_rel_count: &HashMap<String, usize>,
            file_language: &HashMap<String, String>,
            entity_agg: &mut HashMap<String, usize>,
            rel_agg: &mut HashMap<String, usize>,
            lang_agg: &mut HashMap<String, HashSet<String>>,
        ) {
            if let Some(&count) = entities_per_file.get(path) {
                // File (leaf)
                entity_agg.insert(path.to_string(), count);
                rel_agg.insert(
                    path.to_string(),
                    direct_rel_count.get(path).copied().unwrap_or(0),
                );
                let mut langs = HashSet::new();
                if let Some(l) = file_language.get(path) {
                    langs.insert(l.clone());
                }
                lang_agg.insert(path.to_string(), langs);
                return;
            }
            // Folder: recurse into children first
            let children = children_map.get(path).cloned().unwrap_or_default();
            for c in &children {
                aggregate(
                    c, children_map, entities_per_file, direct_rel_count, file_language,
                    entity_agg, rel_agg, lang_agg,
                );
            }
            let entity_sum: usize = children.iter().map(|c| entity_agg.get(c).copied().unwrap_or(0)).sum();
            let rel_sum: usize = children.iter().map(|c| rel_agg.get(c).copied().unwrap_or(0)).sum();
            let mut langs: HashSet<String> = HashSet::new();
            for c in &children {
                if let Some(child_langs) = lang_agg.get(c) {
                    langs.extend(child_langs.iter().cloned());
                }
            }
            entity_agg.insert(path.to_string(), entity_sum);
            rel_agg.insert(
                path.to_string(),
                rel_sum + direct_rel_count.get(path).copied().unwrap_or(0),
            );
            lang_agg.insert(path.to_string(), langs);
        }

        aggregate(
            "",
            &children_map,
            &entities_per_file,
            &direct_rel_count,
            &file_language,
            &mut entity_agg,
            &mut final_rel_agg,
            &mut lang_agg,
        );

        // Build the final IndexNode map
        let mut nodes: BTreeMap<String, IndexNode> = BTreeMap::new();
        for path in &all_paths {
            let is_file = entities_per_file.contains_key(path);
            let children = children_map.get(path).cloned().unwrap_or_default();
            let mut languages: Vec<String> = lang_agg
                .get(path)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            languages.sort();
            nodes.insert(
                path.clone(),
                IndexNode {
                    path: path.clone(),
                    node_type: if is_file { "file" } else { "folder" },
                    entity_count: entity_agg.get(path).copied().unwrap_or(0),
                    relationship_count: final_rel_agg.get(path).copied().unwrap_or(0),
                    languages,
                    children,
                },
            );
        }

        let total_entities = entity_agg.get("").copied().unwrap_or(0);
        let total_relationships = final_rel_agg.get("").copied().unwrap_or(0);

        let output = IndexOutput {
            root: config.root_path.display().to_string(),
            nodes,
            total_entities,
            total_relationships,
        };

        Ok(serde_json::to_string(&output)?)
    }

    /// Render a sidecar detail file containing source_code, fields, and impl_blocks
    /// keyed by entity ID. Only entities with at least one detail field are included.
    ///
    /// Also emits entries keyed by each file's project-root-relative path so
    /// collapsed "file" nodes in the graph can look up their full source. The
    /// file entries reuse the same `EntityDetail` shape (just `source_code`
    /// populated) so the UI needs no new lookup path — `loadDetail(original_id)`
    /// already resolves the file path for a collapsed node.
    pub fn render_details(graph: &DependencyGraph, config: &Config) -> Result<String> {
        let mut details = std::collections::HashMap::new();

        for e in graph.entities() {
            if e.kind == EntityKind::Parameter
                || e.kind == EntityKind::Branch
                || e.kind == EntityKind::Loop
                || e.tags.contains("local_var")
            {
                continue;
            }
            let has_detail = e.source_code.is_some() || !e.fields.is_empty() || !e.impl_blocks.is_empty() || e.documentation.is_some();
            if !has_detail {
                continue;
            }
            details.insert(
                e.id.clone(),
                EntityDetail {
                    documentation: e.documentation.clone(),
                    source_code: e.source_code.clone(),
                    fields: e
                        .fields
                        .iter()
                        .map(|f| JsonField {
                            name: f.name.clone(),
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                    impl_blocks: e.impl_blocks.clone(),
                },
            );
        }

        // Files: one entry per analysed file, keyed by the same project-root-
        // relative path that `file_metrics` uses. We try to read each file
        // from disk; if that fails (e.g. the file was deleted after analysis)
        // we skip it rather than failing the whole render. Large files are
        // truncated so a single generated / fixture file can't balloon the
        // details sidecar — rare in normal codebases, common in generated
        // bindings and test data.
        // Strip the project root so file details are keyed by the SAME
        // root-relative path that entity nodes carry in `file_path`.
        // Both sides are normalized (CurDir components collapsed) before
        // comparison — without this, `src/foo.rs` vs `./src` fails to match
        // component-by-component and `loadDetail` misses the entry.
        let normalize = |p: &std::path::Path| -> std::path::PathBuf {
            use std::path::Component;
            let mut buf = std::path::PathBuf::new();
            for c in p.components() {
                match c {
                    Component::CurDir => {}
                    _ => buf.push(c.as_os_str()),
                }
            }
            buf
        };
        let root_norm = normalize(&config.root_path);
        let strip_root = |raw: &str| -> String {
            let p = normalize(std::path::Path::new(raw));
            p.strip_prefix(&root_norm)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| p.display().to_string())
        };

        const MAX_FILE_BYTES: usize = 256 * 1024;
        for file in graph.file_metrics() {
            let abs = config.root_path.join(&file.path);
            let candidates = [abs, std::path::PathBuf::from(&file.path)];
            let content = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok());
            let documentation = graph
                .file_documentation(std::path::Path::new(&file.path))
                .map(str::to_string);
            let source = content.map(|mut source| {
                if source.len() > MAX_FILE_BYTES {
                    source.truncate(MAX_FILE_BYTES);
                    source.push_str("\n\n/* … truncated by nao (file exceeds 256 KB) */\n");
                }
                source
            });
            // An unreadable file (deleted since the analysis) still has a
            // description worth showing, so the entry turns on either half.
            if source.is_none() && documentation.is_none() {
                continue;
            }
            details.insert(
                strip_root(&file.path),
                EntityDetail {
                    documentation,
                    source_code: source,
                    fields: Vec::new(),
                    impl_blocks: Vec::new(),
                },
            );
        }

        Ok(serde_json::to_string(&details)?)
    }
}

/// Find the lowest common ancestor folder of two file paths.
/// Returns an empty string if they share no common prefix (root folder).
fn lowest_common_ancestor(a: &str, b: &str) -> String {
    let a_parts: Vec<&str> = a.split('/').collect();
    let b_parts: Vec<&str> = b.split('/').collect();
    let mut common = Vec::new();
    // Skip the last part of each (the file name) — we want the folder LCA.
    let a_folders = &a_parts[..a_parts.len().saturating_sub(1)];
    let b_folders = &b_parts[..b_parts.len().saturating_sub(1)];
    for (x, y) in a_folders.iter().zip(b_folders.iter()) {
        if x == y {
            common.push(*x);
        } else {
            break;
        }
    }
    common.join("/")
}

/// Simple timestamp without external chrono dependency
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    
    format!("{}", duration.as_secs())
}
