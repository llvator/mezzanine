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
    pub fan_out: WarnBad,
    pub fields: WarnBad,
    pub variants: WarnBad,
    pub method_count: WarnBad,
    pub public_field_ratio: WarnBad,

    // --- Scope-level (file) ---
    pub file_entity_count: WarnBad,
    pub file_loc: WarnBad,
    pub file_fan_out: WarnBad,

    // --- Scope-level (module) ---
    pub module_entity_count: WarnBad,
    pub module_loc: WarnBad,
    pub module_fan_out: WarnBad,

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
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cc: WarnBad { warn: 10.0, bad: 20.0 },
            cognitive: WarnBad { warn: 8.0, bad: 15.0 },
            nest: WarnBad { warn: 3.0, bad: 5.0 },
            loc_callable: WarnBad { warn: 30.0, bad: 60.0 },
            loc_container: WarnBad { warn: 100.0, bad: 200.0 },
            params: WarnBad { warn: 4.0, bad: 6.0 },
            fan_out: WarnBad { warn: 7.0, bad: 15.0 },
            fields: WarnBad { warn: 8.0, bad: 15.0 },
            variants: WarnBad { warn: 6.0, bad: 12.0 },
            method_count: WarnBad { warn: 15.0, bad: 25.0 },
            public_field_ratio: WarnBad { warn: 0.5, bad: 0.8 },

            file_entity_count: WarnBad { warn: 15.0, bad: 30.0 },
            file_loc: WarnBad { warn: 400.0, bad: 800.0 },
            file_fan_out: WarnBad { warn: 10.0, bad: 20.0 },

            module_entity_count: WarnBad { warn: 60.0, bad: 150.0 },
            module_loc: WarnBad { warn: 2000.0, bad: 5000.0 },
            module_fan_out: WarnBad { warn: 15.0, bad: 30.0 },

            cohesion: WarnBad { warn: 0.6, bad: 0.3 },

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
        }
    }
}
