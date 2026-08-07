//! Per-file and per-module (directory) code-quality metrics.
//!
//! Rolls entity- and relationship-level data up to coarser scopes so the
//! UI can answer "which files are bloated?" and "which directories are
//! tightly coupled?" — complementary to the per-entity view.

use serde::{Deserialize, Serialize};

/// Aggregate quality metrics for a single file OR a module (directory).
/// The shape is shared because the signals are the same at both scopes;
/// callers distinguish them via the surrounding `FileMetrics` /
/// `ModuleMetrics` wrapper.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScopeMetrics {
    /// Total non-parameter entities living in this scope.
    pub entity_count: u32,
    /// Subset of `entity_count` that are callables (functions / methods).
    pub callable_count: u32,
    /// Subset that are containers (structs / enums / traits / modules).
    pub container_count: u32,
    /// Lines of code summed across the scope. For files this matches the
    /// raw line count; for modules it's the sum of contained files.
    pub loc: u32,
    /// Dependency edges whose source and target are both in this scope.
    pub internal_edges: u32,
    /// Dependency edges that cross this scope's boundary (either direction).
    pub external_edges: u32,
    /// Cohesion ratio `internal / (internal + external)`, in [0, 1]. `None`
    /// when the scope has no dependency edges at all (undefined).
    pub cohesion: Option<f32>,
    /// Distinct other scopes depending on this one (incoming).
    pub fan_in: u32,
    /// Distinct other scopes this one depends on (outgoing).
    pub fan_out: u32,
    /// True if this scope participates in a scope-level dependency cycle.
    pub in_cycle: bool,
    /// Instability index: `fan_out / (fan_in + fan_out)`. In [0, 1].
    /// `None` when both are 0.
    pub instability: Option<f32>,

    // --- Reference edges, counted apart from coupling (UI-091) ---
    //
    // Every field above is computed over dependency edges only, so a scope
    // whose relationships are all `References` — a Markdown link, an
    // Elevator or Impex reference, a folded SQL foreign key — scores a
    // fan-out of `0` while the canvas plainly draws arrows leaving it. Zero
    // is the *good* end of that scale, so an unmeasured scope reads as a
    // perfectly decoupled one.
    //
    // These two say how much was passed over. They deliberately feed no
    // ratio, no cycle detection and no composite score: this is a claim
    // about what the numbers above cover, not a second opinion on coupling.
    /// Distinct other scopes referencing this one, over `References` edges.
    #[serde(default)]
    pub ref_fan_in: u32,
    /// Distinct other scopes this one references, over `References` edges.
    #[serde(default)]
    pub ref_fan_out: u32,

    // --- Aggregated entity-quality rollup ---
    /// Mean composite score of all entities in this scope.
    pub avg_quality: f32,
    /// Highest (worst) composite score in this scope.
    pub max_quality: f32,
    /// Entities whose composite score is ≤ 0.5 (healthy).
    pub quality_ok: u32,
    /// Entities whose composite score is in (0.5, 1.0] (amber).
    pub quality_warn: u32,
    /// Entities whose composite score is > 1.0 (red).
    pub quality_bad: u32,

    /// Composite "refactor pressure" score for this scope. Context-aware:
    /// suppresses cohesion penalty for stable data models and wiring files,
    /// halves LOC penalty when cohesion is high.
    pub composite_score: f32,
}

/// File names that are wiring / entry-point by nature.
const WIRING_FILES: &[&str] = &[
    "mod.rs", "main.rs", "lib.rs", "index.ts", "index.js",
    "mod.ts", "__init__.py",
];

impl ScopeMetrics {
    /// Compute the composite scope score. `path` is the file/module path
    /// (used for wiring-file detection). `is_module` toggles file vs.
    /// module thresholds.
    pub fn compute_composite_score(&mut self, path: &str, is_module: bool) {
        let t = super::thresholds::Thresholds::default();

        // The root module encompasses the entire project — its entity count
        // and LOC will always be at maximum. Penalizing it is not actionable.
        // Detected by: no external edges at all (nothing outside it exists).
        let is_root_module = is_module && self.fan_in == 0 && self.fan_out == 0 && self.external_edges == 0;

        let entity_red = if is_module { t.module_entity_count.bad } else { t.file_entity_count.bad };
        let loc_red = if is_module { t.module_loc.bad } else { t.file_loc.bad };
        let fo_red = if is_module { t.module_fan_out.bad } else { t.file_fan_out.bad };

        let entity = if is_root_module { 0.0 } else { (self.entity_count as f32 / entity_red).min(2.0) };
        let mut loc = if is_root_module { 0.0 } else { (self.loc as f32 / loc_red).min(2.0) };
        let fo = (self.fan_out as f32 / fo_red).min(2.0);

        let min_ent = t.min_entities_for_cohesion as u32;
        let has_meaningful_cohesion = self.cohesion.is_some()
            && self.entity_count >= min_ent;
        let is_stable_data_model = self.instability.unwrap_or(1.0) <= 0.15
            && self.fan_in >= 3
            && self.container_count >= 2
            && self.container_count as f32 >= self.callable_count as f32 * 0.3;
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let is_wiring = WIRING_FILES.contains(&file_name);

        let (cohesion_penalty, cohesion_weight) = if !has_meaningful_cohesion || is_stable_data_model {
            (0.0, 0.0)
        } else {
            let coh_val = self.cohesion.unwrap_or(1.0);
            let penalty = if coh_val < t.cohesion.bad {
                ((t.cohesion.bad - coh_val) / t.cohesion.bad).min(1.0) * 2.0
            } else if coh_val < t.cohesion.warn {
                0.5
            } else {
                0.0
            };
            let weight = if is_wiring { 0.12 } else { 0.25 };
            (penalty, weight)
        };

        // LOC penalty halved when cohesion is high.
        if has_meaningful_cohesion && self.cohesion.unwrap_or(0.0) >= 0.8 {
            loc *= 0.5;
        }

        let cycle = if self.in_cycle { 0.5 } else { 0.0 };
        let base_weight = 0.25 + 0.2 + 0.25 + cohesion_weight;
        let norm = if base_weight > 0.0 { 1.0 / base_weight } else { 1.0 };
        self.composite_score = (0.25 * entity + 0.2 * loc + 0.25 * fo
            + cohesion_weight * cohesion_penalty) * norm + cycle;
    }
}

/// Per-file rollup. `path` is relative to the analysis root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetrics {
    pub path: String,
    pub metrics: ScopeMetrics,
}

/// Per-directory rollup. `path` is relative to the analysis root; the
/// root directory itself is `""`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModuleMetrics {
    pub path: String,
    pub metrics: ScopeMetrics,
}
