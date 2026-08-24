//! What `reshape` last told an agent about a folder, and where the server
//! keeps it.
//!
//! Its own file because two modules need it for unrelated reasons.
//! `reshape` builds a reading and compares it with the one before;
//! [`McpServer`](super::McpServer) merely has to hold the readings between
//! calls. With the type living in `reshape`, the server depended on the
//! whole tool to declare one field, and the folder's children closed a
//! loop — `mod.rs → reshape.rs → mod.rs` — that left the drawing with no
//! reading order at all.
//!
//! The store travels with the type for the same reason. A `Mutex<HashMap<…>>`
//! spelled out in the server struct is the map's shape leaking into a file
//! that never touches it; `swap` is the only thing anyone does to it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use crate::models::{FolderPicture, FolderShape, ShapePattern};

/// What `reshape` told an agent about a folder last time it asked.
///
/// Structure is stored beside the scores because the two answer different
/// halves of the closing instruction. The scores say whether the verdict
/// moved; the edge and boundary sets say whether anything about
/// who-depends-on-what moved with it. A tier that rose while both sets
/// stayed identical is the cosmetic change this tool spends its whole
/// forbidden list warning against, and it is only detectable by having
/// kept the earlier drawing.
#[derive(Clone, PartialEq)]
pub(crate) struct Baseline {
    /// The scope the reading was taken under — see
    /// [`scope_id`](super::scope_id).
    ///
    /// Recorded beside the numbers because a difference here is the one
    /// explanation for an unchanged drawing that the agent cannot check
    /// for itself: the settings file it would have to read is the
    /// server's, and the server may have been started somewhere else
    /// entirely (CFG-014).
    pub(super) scope: String,
    pub(super) pattern: ShapePattern,
    pub(super) compliance: f32,
    pub(super) layering: Option<f32>,
    pub(super) arborescence: Option<f32>,
    pub(super) entry_concentration: Option<f32>,
    pub(super) child_count: u32,
    /// Every drawn edge, as `from → to`.
    pub(super) edges: BTreeSet<(String, String)>,
    /// Every dependency crossing the boundary, so a change in entry
    /// concentration can be attributed to something rather than appearing
    /// out of nowhere.
    pub(super) outside: BTreeSet<(String, String)>,
}

impl Baseline {
    pub(super) fn of(shape: &FolderShape, p: &FolderPicture, scope: String) -> Self {
        Self {
            scope,
            pattern: shape.pattern,
            compliance: shape.compliance,
            layering: shape.layering,
            arborescence: shape.arborescence,
            entry_concentration: shape.entry_concentration,
            child_count: shape.child_count,
            edges: p
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect(),
            outside: p
                .outside
                .iter()
                .map(|o| (o.outside.clone(), o.inside.clone()))
                .collect(),
        }
    }

    /// Whether every measured value is identical — the test behind
    /// "Nothing has changed".
    ///
    /// Not `==`, because [`Self::scope`] travels on the same record
    /// without being part of the reading. A configuration change that
    /// moved no number still has to reach that sentence rather than the
    /// field-by-field diff below it, which would otherwise print
    /// `verdict tangled → tangled` and call it progress. Written as a
    /// comparison against a rescoped copy rather than a list of fields,
    /// so a field added later counts as part of the reading by default.
    pub(super) fn same_reading(&self, other: &Self) -> bool {
        let mut rescoped = self.clone();
        rescoped.scope.clone_from(&other.scope);
        rescoped == *other
    }

    /// Whether anything about who-depends-on-what differs. The test the
    /// forbidden list already states in prose, made mechanical.
    pub(super) fn same_structure(&self, other: &Self) -> bool {
        self.edges == other.edges
            && self.outside == other.outside
            && self.child_count == other.child_count
    }
}

/// The last drawing reported per folder, keyed by its relative path.
///
/// Held by the server rather than asked of the caller on purpose. The
/// tool's closing instruction is to re-run and check the tier moved, and
/// an agent that reports its own before-and-after is grading its own work
/// from memory. Keeping the baseline here means the second call answers
/// the question instead of the agent.
#[derive(Default)]
pub(crate) struct ShapeBaselines(Mutex<HashMap<String, Baseline>>);

impl ShapeBaselines {
    /// Store this call's drawing and hand back the one it replaced.
    ///
    /// Swapped rather than read-then-written so the record is always the
    /// last thing the agent was actually shown: a call that reports
    /// against the previous drawing is also the call the *next* one will
    /// be graded on. A poisoned lock loses the baseline and reports
    /// nothing, which is the right failure — a comparison against an
    /// unknown drawing would be worse than none.
    pub(super) fn swap(&self, folder: &str, current: &Baseline) -> Option<Baseline> {
        let mut seen = self.0.lock().ok()?;
        seen.insert(folder.to_string(), current.clone())
    }
}
