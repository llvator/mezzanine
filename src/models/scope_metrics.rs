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

    /// How legible a picture this folder draws — see [`FolderShape`].
    ///
    /// Always `None` for a file: the measure is about the graph a scope's
    /// *children* form, and a file has none. Like `ref_fan_in` above it
    /// feeds no ratio and never enters `composite_score`; organisation and
    /// code quality are different questions and a folder can score well on
    /// one while failing the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<FolderShape>,
}

/// The four organisation patterns a folder's collapsed child graph can
/// fall into, worst first. Ordered as a ladder: each tier is the one below
/// it plus one more property.
/// Declaration order is the ladder, so `Cyclic < Tangled < Hierarchical
/// < Fractal` and a parent can ask whether its worst child clears a bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapePattern {
    /// Children depend on each other in a loop. No drawing of a cycle has
    /// a reading order, so this disqualifies every tier above.
    Cyclic,
    /// Acyclic, but edges jump levels rather than stepping down one at a
    /// time. The drawing has no layers to follow.
    Tangled,
    /// A clean layered DAG. Readable, but says nothing about whether the
    /// folders inside it are.
    Hierarchical,
    /// Hierarchical, few enough children to take in at one glance, reached
    /// from outside through few doors, and made of children that are
    /// themselves at least hierarchical. The shape holds at more than one
    /// zoom level, which is what makes it self-similar.
    Fractal,
}

impl ShapePattern {
    /// Lower-case tag used in JSON, the MCP output and the UI.
    pub fn label(&self) -> &'static str {
        match self {
            ShapePattern::Cyclic => "cyclic",
            ShapePattern::Tangled => "tangled",
            ShapePattern::Hierarchical => "hierarchical",
            ShapePattern::Fractal => "fractal",
        }
    }

    /// What to do about it, when there is something to do.
    pub fn hint(&self) -> &'static str {
        match self {
            ShapePattern::Cyclic => {
                "Break the loop between these children — usually by moving the shared \
                 piece down into a child both can depend on."
            }
            ShapePattern::Tangled => {
                "Edges skip levels here. Either the intermediate layer is not carrying \
                 the traffic it should, or the shortcuts around it are the real design."
            }
            ShapePattern::Hierarchical => {
                "Readable at this level. It falls short of fractal because it holds more \
                 children than a reader takes in at once, because its children share \
                 dependencies rather than branching, because outsiders reach into it at \
                 many points, or because a folder inside it does not hold the same shape."
            }
            ShapePattern::Fractal => "Nothing to do — the shape holds at every level.",
        }
    }
}

/// The one gate that stopped a folder reaching the tier above it, with
/// the measurement that failed it. `None` for a `Fractal` folder, which
/// nothing is holding back.
///
/// `compliance` cannot carry this: it is a weighted blend, so two folders
/// at 0.72 can need opposite fixes, and a reader given the blend has to
/// re-derive which of four gates it was. Naming the binding constraint
/// says *why* instead of *how much*, which is the difference between a
/// number and an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "gate", content = "value", rename_all = "snake_case")]
pub enum ShapeBlocker {
    /// Children sit in a dependency loop. Carries `acyclicity`.
    Cycles(f32),
    /// Edges skip levels instead of stepping down one. Carries `layering`.
    Layering(f32),
    /// The drawing is not a tree: children leaning on the same sibling so
    /// it merges rather than branches, children no edge reaches at all, or
    /// both. Carries `arborescence`.
    Merges(f32),
    /// Outsiders reach in at many points rather than through one door.
    /// Carries `entry_concentration`.
    Entry(f32),
    /// A subfolder is itself below hierarchical — the recursion breaks one
    /// level down. Carries that child's pattern.
    ChildPattern(ShapePattern),
    /// The subfolders are broadly out of order without any single one
    /// being unreadable. Carries `child_compliance`.
    ChildCompliance(f32),
    /// Children with no edges between them: a legible row of dots, but no
    /// structure for the recursion to be self-similar *to*.
    Unstructured,
    /// More immediate children than a reader holds at once. Carries the
    /// count. The one gate that is a convention rather than a reading of
    /// the drawing, and the only one whose fix moves files instead of
    /// edges.
    Breadth(u32),
    /// Every gate passed and the blended score still fell short — several
    /// terms a little low rather than one clearly wrong. Carries
    /// `compliance`.
    Compliance(f32),
}

impl ShapeBlocker {
    /// The gate and its failing measurement in one phrase, for a line that
    /// already names the folder.
    ///
    /// Split in two along the line the ladder itself is built on — what
    /// this folder's own drawing shows, against what only its children or
    /// the blend can answer. The same split orders the gates in
    /// `analyzer::folder_shape::short_of_fractal`.
    pub fn summary(&self) -> String {
        self.own_drawing_summary()
            .unwrap_or_else(|| self.deeper_summary())
    }

    /// Whether this gate can be settled from the folder's own drawing, or
    /// only by changing something outside it — a subfolder, a caller, or
    /// enough of the blend to move it.
    ///
    /// The same line `summary` splits on, exposed because a reader
    /// planning an order of work needs it *before* they need the wording.
    /// A folder held back by a tangled subfolder is not one to send
    /// anybody at yet: the ladder is recursive, so that work is done one
    /// level down and the parent re-measured afterwards. Derived from
    /// `own_drawing_summary` rather than re-matching the variants, so the
    /// two readings of the line cannot drift apart.
    pub fn is_own_drawing(&self) -> bool {
        self.own_drawing_summary().is_some()
    }

    /// Gates settled by the picture in front of the reader. `None` for the
    /// ones that are not.
    fn own_drawing_summary(&self) -> Option<String> {
        Some(match self {
            ShapeBlocker::Cycles(v) => format!("a loop among its children (acyclic {v:.2})"),
            ShapeBlocker::Layering(v) => format!("edges skipping levels (layered {v:.2})"),
            ShapeBlocker::Merges(v) => {
                format!("children sharing a parent or having none (one-parent {v:.2})")
            }
            ShapeBlocker::Breadth(n) => {
                format!("more children than a reader holds at once ({n} of them)")
            }
            ShapeBlocker::Unstructured => "no edges between its children".to_string(),
            _ => return None,
        })
    }

    /// Gates about what is inside the children, who reaches in from
    /// outside, or the blend over all of it.
    fn deeper_summary(&self) -> String {
        match self {
            ShapeBlocker::Entry(v) => format!("too many doors in (one-door-in {v:.2})"),
            ShapeBlocker::ChildPattern(p) => format!("a {} folder inside it", p.label()),
            ShapeBlocker::ChildCompliance(v) => {
                format!("subfolders broadly out of order (children {v:.2})")
            }
            ShapeBlocker::Compliance(v) => format!("an overall score of {v:.2}"),
            // Every remaining variant is answered by `own_drawing_summary`
            // and never reaches here. Phrased rather than panicked: a
            // blocker that somehow arrived unlabelled should degrade a
            // sentence, not take down the tool reporting it.
            _ => "a gate this build has no wording for".to_string(),
        }
    }
}

/// How legible the graph a folder draws is.
///
/// Scored over exactly what the canvas renders when collapsed to this
/// folder: its immediate children, each subfolder standing as one node.
/// That is deliberate — the number is a claim about a picture someone
/// actually looks at, not about an abstract graph nobody draws.
///
/// Every sub-score runs 0–1 with **higher meaning better**, which is the
/// opposite of `composite_score` beside it. The two answer different
/// questions and are not comparable; the inversion is the reminder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderShape {
    /// Which tier the folder lands in.
    pub pattern: ShapePattern,
    /// Weighted mean of the sub-scores below, over whichever are defined.
    pub compliance: f32,
    /// Share of children that sit outside any dependency loop. `1.0` means
    /// no cycles at all; anything less forces the `Cyclic` pattern.
    pub acyclicity: f32,
    /// Share of edges that step exactly one level down, where a level is
    /// the longest path from a source. The rest jump levels, which is what
    /// makes a drawing hard to follow. `None` when the children have no
    /// edges between them — an unmeasured shape, not a perfect one.
    ///
    /// Measured over the cycle-condensed graph so a folder with a loop
    /// still reports something meaningful beside its `acyclicity`.
    pub layering: Option<f32>,
    /// Share of the drawn edges that would survive in a spanning forest —
    /// one arriving at each child. The rest are merges: a second and third
    /// sibling leaning on the same file. `None` on the same condition
    /// `layering` is, the two being ratios over the same edges.
    ///
    /// This is the property `layering` deliberately refuses to measure, and
    /// the two disagree on purpose. Two files leaning on one helper is
    /// healthy reuse and steps one level cleanly, so `layering` scores it
    /// 1.0; it is also a merge point a reader has to hold in their head,
    /// so `arborescence` scores it 0.5. Different questions, different
    /// numbers (ADR 0013).
    ///
    /// Gates `Fractal` and stays out of `compliance`, for the reason shape
    /// itself stays out of `composite_score`: a blend that moves for two
    /// unrelated reasons can be acted on for neither.
    ///
    /// Measured over the cycle-condensed graph, like `layering` and for the
    /// same reason — a loop is already charged to `acyclicity`.
    pub arborescence: Option<f32>,
    /// Of the dependencies arriving from outside this folder, the share
    /// landing on its single most-depended-on file. High means outsiders
    /// come through one door and the folder is an honest single node when
    /// collapsed; low means they pierce it at many points and the collapsed
    /// drawing is hiding traffic. `None` for a folder nothing outside it
    /// depends on.
    pub entry_concentration: Option<f32>,
    /// Mean `compliance` of the subfolders directly inside this one. The
    /// recursive term — it is what makes the measure about self-similarity
    /// rather than about one level in isolation. `None` for a folder with
    /// no subfolders.
    pub child_compliance: Option<f32>,
    /// Immediate children, files and subfolders together.
    ///
    /// Gates `Fractal` against `Thresholds::shape_max_children` and stays
    /// out of `compliance`, exactly as `arborescence` does (ADR 0014). It
    /// was reported and unscored until then, on the grounds that a legible
    /// shape is a property of the graph while "too many children" is a
    /// convention. Both halves of that are still true — the convention is
    /// now enforced anyway, because self-similarity is a claim about a
    /// picture someone can take in at one glance, and a folder of forty
    /// cleanly-stepping files is not one. Keeping it out of the blend is
    /// what stops a wide folder's compliance from lying about its edges.
    pub child_count: u32,
    /// Why this folder is not one tier higher — see [`ShapeBlocker`].
    /// `None` only for `Fractal`, where there is no tier above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<ShapeBlocker>,
}

/// File names that are wiring / entry-point by nature.
const WIRING_FILES: &[&str] = &[
    "mod.rs",
    "main.rs",
    "lib.rs",
    "index.ts",
    "index.js",
    "mod.ts",
    "__init__.py",
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
        let is_root_module =
            is_module && self.fan_in == 0 && self.fan_out == 0 && self.external_edges == 0;

        let entity_red = if is_module {
            t.module_entity_count.bad
        } else {
            t.file_entity_count.bad
        };
        let loc_red = if is_module {
            t.module_loc.bad
        } else {
            t.file_loc.bad
        };
        let fo_red = if is_module {
            t.module_fan_out.bad
        } else {
            t.file_fan_out.bad
        };

        let entity = if is_root_module {
            0.0
        } else {
            (self.entity_count as f32 / entity_red).min(2.0)
        };
        let mut loc = if is_root_module {
            0.0
        } else {
            (self.loc as f32 / loc_red).min(2.0)
        };
        let fo = (self.fan_out as f32 / fo_red).min(2.0);

        let min_ent = t.min_entities_for_cohesion as u32;
        let has_meaningful_cohesion = self.cohesion.is_some() && self.entity_count >= min_ent;
        let is_stable_data_model = self.instability.unwrap_or(1.0) <= 0.15
            && self.fan_in >= 3
            && self.container_count >= 2
            && self.container_count as f32 >= self.callable_count as f32 * 0.3;
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let is_wiring = WIRING_FILES.contains(&file_name);

        let (cohesion_penalty, cohesion_weight) =
            if !has_meaningful_cohesion || is_stable_data_model {
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
        let norm = if base_weight > 0.0 {
            1.0 / base_weight
        } else {
            1.0
        };
        self.composite_score =
            (0.25 * entity + 0.2 * loc + 0.25 * fo + cohesion_weight * cohesion_penalty) * norm
                + cycle;
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

#[cfg(test)]
mod shape_blocker_tests {
    use super::*;

    fn every_variant() -> Vec<ShapeBlocker> {
        vec![
            ShapeBlocker::Cycles(0.5),
            ShapeBlocker::Layering(0.5),
            ShapeBlocker::Merges(0.5),
            ShapeBlocker::Entry(0.5),
            ShapeBlocker::ChildPattern(ShapePattern::Tangled),
            ShapeBlocker::ChildCompliance(0.5),
            ShapeBlocker::Unstructured,
            ShapeBlocker::Breadth(9),
            ShapeBlocker::Compliance(0.5),
        ]
    }

    /// The predicate and the wording read the same line or a reader is
    /// told a folder is actionable and then handed a sentence about its
    /// children. Adding a variant fails here until it is placed on one
    /// side, which is the point: an unplaced gate silently falls to
    /// "answered elsewhere" and never reaches a work list.
    #[test]
    fn the_predicate_agrees_with_the_wording() {
        for blocker in every_variant() {
            assert_eq!(
                blocker.is_own_drawing(),
                blocker.own_drawing_summary().is_some(),
                "{blocker:?} splits one way for the predicate and another for the text",
            );
        }
    }

    /// No variant may fall through to the unlabelled arm of
    /// `deeper_summary` — that string is a degradation, not a wording.
    #[test]
    fn every_variant_has_a_wording() {
        for blocker in every_variant() {
            let summary = blocker.summary();
            assert!(
                !summary.contains("no wording for"),
                "{blocker:?} reached the fallback arm",
            );
        }
    }

    /// The gates settled by looking at the picture, named rather than
    /// derived, so a change of mind about where the line sits has to be
    /// made deliberately here as well as in the code.
    #[test]
    fn the_line_sits_where_the_ladder_puts_it() {
        let own: Vec<bool> = every_variant()
            .iter()
            .map(ShapeBlocker::is_own_drawing)
            .collect();
        assert_eq!(
            own,
            vec![true, true, true, false, false, false, true, true, false],
            "Cycles/Layering/Merges/Unstructured/Breadth are the folder's own \
             drawing; Entry/ChildPattern/ChildCompliance/Compliance are not",
        );
    }
}
