//! Centralized quality thresholds. Single source of truth for all scoring,
//! tier classification, and smell detection across both the Rust backend
//! and the frontend (serialized in the API response).

use serde::{Deserialize, Serialize};

/// Warn / bad threshold pair. For most metrics "higher is worse" so
/// `warn` < `bad`. Cohesion is inverted (lower is worse) — callers handle
/// the inversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarnBad {
    pub warn: f32,
    pub bad: f32,
}

/// All quality thresholds used by the analyzer. Serialized alongside
/// entities and scope metrics so the frontend reads them from the API
/// instead of maintaining its own copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    // --- Entity-level (callable) ---
    pub cc: WarnBad,
    pub cognitive: WarnBad,
    pub nest: WarnBad,
    pub loc_callable: WarnBad,
    pub loc_container: WarnBad,
    pub params: WarnBad,
    /// Distinct names one callable body puts in view at once — parameters,
    /// locals and explicitly-received fields. Miller's 7±2: seven is the
    /// point where a reader starts paging, twelve is past any of them.
    ///
    /// Unlike every other pair here this one also *is* the smell rule: a
    /// working set over `bad` raises `OverfullHead` directly, with no second
    /// cut-off in the smell block below. A separate knob could only ever
    /// disagree with the line the same file already draws.
    pub working_set: WarnBad,
    pub fan_out: WarnBad,
    pub fields: WarnBad,
    pub variants: WarnBad,
    pub method_count: WarnBad,
    pub public_field_ratio: WarnBad,

    // --- Scope-level (file) ---
    pub file_entity_count: WarnBad,
    pub file_loc: WarnBad,
    pub file_fan_out: WarnBad,

    // --- Scope-level (folder) ---
    pub folder_entity_count: WarnBad,
    pub folder_loc: WarnBad,
    pub folder_fan_out: WarnBad,

    // --- Cohesion (inverted: lower is worse) ---
    pub cohesion: WarnBad,

    // --- Smell detection ---
    pub god_class_fields: f32,
    pub god_class_methods: f32,
    pub god_class_fan_out: f32,
    pub dispatcher_cc: f32,
    pub dispatcher_fan_out: f32,
    pub dispatcher_loc_per_branch: f32,
    pub dispatcher_fan_out_cc_ratio: f32,
    pub feature_envy_ratio: f32,
    pub feature_envy_min_edges: f32,
    pub shotgun_fan_in: f32,
    /// Data bag: at least this many fields and no more than
    /// `data_bag_max_methods` methods. Marks "config bag" types that
    /// hold data without behaviour — not a god class, but often the
    /// grouping wants to be made explicit via nested sub-structs.
    pub data_bag_fields: f32,
    pub data_bag_max_methods: f32,

    // --- Scope scoring context ---
    pub min_entities_for_cohesion: f32,

    // --- Folder shape (inverted: higher is better) ---
    //
    // These gate the `ShapePattern` ladder. The first five are cut-offs on
    // measures that already run 0–1, so unlike the pairs above they are
    // single values, and a *higher* number is the good end. The sixth
    // counts children, where *lower* is the good end — it is a different
    // kind of bar and is documented as such.
    /// Below this share of level-stepping edges a folder reads as tangled
    /// rather than hierarchical.
    pub shape_layering: f32,
    /// Share of a folder's drawn edges that must be branching rather than
    /// merging for it to count as fractal — how close its picture is to a
    /// tree. Read beside `shape_layering`: same denominator, same scale,
    /// deliberately different question.
    pub shape_arborescence: f32,
    /// Entry concentration a folder must reach to count as fractal — how
    /// much of the traffic arriving from outside lands on one file.
    pub shape_entry: f32,
    /// Share of a folder's outgoing dependencies that must start at a leaf
    /// or at its door for it to count as fractal — whether the folder
    /// reaches outward from the bottom or leaks from its middle. The mirror
    /// of `shape_entry`: that one grades what arrives, this what leaves.
    pub shape_egress: f32,
    /// Mean compliance a folder's subfolders must reach for it to count as
    /// fractal. The recursive gate.
    pub shape_child: f32,
    /// How close a folder's breadth must stay to its widest subfolder's
    /// before the drawing is reported as changing scale between the two
    /// levels.
    ///
    /// The one bar here that gates nothing (ADR 0033). `uniformity` is
    /// reported against it and no tier moves, because the weights and
    /// cut-offs above were fitted against a distribution this term was not
    /// in, and re-ranking every repo's fractal badges on a number nobody
    /// has yet seen across a corpus is the thing ADR 0032 forbids. Set at
    /// `shape_layering`'s value rather than at one fitted here: four ratios
    /// on one scale with one bar is the only arrangement that needs no
    /// explaining, and a bar invented for this term alone would be invented
    /// blind.
    pub shape_uniformity: f32,
    /// Overall compliance a folder must reach to count as fractal.
    pub shape_compliance: f32,
    /// Most immediate children — files and subfolders together — a folder
    /// may hold and still count as fractal. A count rather than a ratio,
    /// which is why it is the one bar here that is not an `f32`.
    ///
    /// Unlike the five above this is a convention, not a measurement of the
    /// drawing, and it is the only gate that can be cleared without any
    /// edge changing. It earns its place because self-similarity is a claim
    /// about a picture a reader can hold at once, and a folder of forty
    /// files fails that however cleanly its edges step (ADR 0014). Like
    /// `shape_arborescence` it gates the tier and stays out of
    /// `compliance`, so a wide folder still reports an honest blend.
    pub shape_max_children: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cc: WarnBad {
                warn: 10.0,
                bad: 20.0,
            },
            cognitive: WarnBad {
                warn: 8.0,
                bad: 15.0,
            },
            nest: WarnBad {
                warn: 3.0,
                bad: 5.0,
            },
            loc_callable: WarnBad {
                warn: 30.0,
                bad: 60.0,
            },
            loc_container: WarnBad {
                warn: 100.0,
                bad: 200.0,
            },
            params: WarnBad {
                warn: 4.0,
                bad: 6.0,
            },
            working_set: WarnBad {
                warn: 7.0,
                bad: 12.0,
            },
            fan_out: WarnBad {
                warn: 7.0,
                bad: 15.0,
            },
            fields: WarnBad {
                warn: 8.0,
                bad: 15.0,
            },
            variants: WarnBad {
                warn: 6.0,
                bad: 12.0,
            },
            method_count: WarnBad {
                warn: 15.0,
                bad: 25.0,
            },
            public_field_ratio: WarnBad {
                warn: 0.5,
                bad: 0.8,
            },

            file_entity_count: WarnBad {
                warn: 15.0,
                bad: 30.0,
            },
            file_loc: WarnBad {
                warn: 400.0,
                bad: 800.0,
            },
            file_fan_out: WarnBad {
                warn: 10.0,
                bad: 20.0,
            },

            folder_entity_count: WarnBad {
                warn: 60.0,
                bad: 150.0,
            },
            folder_loc: WarnBad {
                warn: 2000.0,
                bad: 5000.0,
            },
            folder_fan_out: WarnBad {
                warn: 15.0,
                bad: 30.0,
            },

            cohesion: WarnBad {
                warn: 0.6,
                bad: 0.3,
            },

            god_class_fields: 10.0,
            god_class_methods: 15.0,
            god_class_fan_out: 10.0,
            dispatcher_cc: 20.0,
            dispatcher_fan_out: 10.0,
            dispatcher_loc_per_branch: 5.0,
            dispatcher_fan_out_cc_ratio: 0.4,
            feature_envy_ratio: 0.6,
            feature_envy_min_edges: 3.0,
            shotgun_fan_in: 15.0,
            data_bag_fields: 8.0,
            data_bag_max_methods: 3.0,

            min_entities_for_cohesion: 5.0,

            shape_layering: 0.7,
            // Deliberately the same cut-off as the line above. The two are
            // ratios over the same drawn edges, so an equal bar is the only
            // one that needs no explaining — a reader comparing "layered
            // 0.89, branching 0.67" is reading one scale. Measured on this
            // repo: 24 folders have edges to score, 15 clear it on layering
            // and 9 on branching, and the gate costs two folders their
            // fractal badge (src/parser/elevator, vscode-extension/src).
            shape_arborescence: 0.7,
            shape_entry: 0.6,
            shape_egress: 0.7,
            shape_child: 0.8,
            shape_uniformity: 0.7,
            shape_compliance: 0.85,
            // Eight. It began at seven, from the span of what a reader
            // holds at once, and that first note said plainly that the
            // repo could not yet validate it: every folder over the bar
            // was already cyclic or tangled, so the gate never fired.
            //
            // Two days of use supplied the missing evidence, and it argued
            // for one more. Once several folders had been taken up the
            // ladder, breadth became the most common blocker among the
            // ones whose graphs were already clean — and three of them sat
            // at exactly eight. `src/parser/rust/declarations` is a
            // perfect tree (branching 1.00, acyclic, properly layered) and
            // was held off `fractal` by a single file. A bar that stops a
            // spanning tree is measuring the wrong thing.
            //
            // Eight still refuses the folders the gate was written for —
            // the twelve-to-thirty-nine-child levels nobody takes in at a
            // glance — while letting a clean eight through. See ADR 0018.
            shape_max_children: 8,
        }
    }
}
