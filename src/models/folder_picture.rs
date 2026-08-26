//! The graph a folder draws, with a verdict on every part of it.
//!
//! [`super::FolderShape`] is the verdict — four numbers and the gate that
//! capped the tier. This is the evidence behind it: the same collapsed
//! child graph the score is computed over, kept rather than discarded, with
//! each node's level and each edge's reading marked on it.
//!
//! It exists because a score you cannot look at is not actionable. A reader
//! told "this folder is tangled, layered 0.61" has no way to find the six
//! edges that did it, and neither has an agent asked to fix them. Both need
//! the picture, and both must be handed the *same* picture the number came
//! from — which is why this is produced by the pass that does the scoring
//! rather than reconstructed alongside it.
//!
//! Deliberately not on `ScopeMetrics`. Every folder gets a `FolderShape`
//! because four floats per folder is free; a picture per folder is the
//! whole edge list again, so it is computed for the one folder somebody
//! asked about.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Strip an analysis root off one path, leaving anything that does not
/// start with it alone — the same rule the JSON renderer applies to file
/// and module rollups, so a picture and the metrics beside it are keyed
/// alike.
fn relative(raw: &str, root: &Path) -> String {
    match Path::new(raw).strip_prefix(root) {
        Ok(rest) => rest.display().to_string(),
        Err(_) => raw.to_string(),
    }
}

/// One folder's drawn graph: its immediate children, the edges between
/// them, and the one-hop traffic across its boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderPicture {
    /// The folder this is a picture of.
    pub folder: String,
    /// Immediate children — files directly inside, and each subfolder as a
    /// single node. Exactly what the canvas draws when collapsed here, and
    /// exactly what [`super::FolderShape`] is scored over (ADR 0012).
    pub children: Vec<PictureChild>,
    /// Edges between those children, each carrying how it reads. Exactly
    /// the set the scores were computed over — `erased` is drawn beside
    /// it and deliberately not in it.
    pub edges: Vec<PictureEdge>,
    /// The arrows the build erases, discounted from the scores (ADR 0026).
    ///
    /// Beside `edges` rather than inside it, because the contract of this
    /// struct is that the picture is the graph the numbers came from. An
    /// arrow that is not counted must not be drawn as though it were, and
    /// an arrow that is not drawn at all leaves a reader wondering where
    /// their `import type` went. Listed apart answers both.
    #[serde(default)]
    pub erased: Vec<ErasedEdge>,
    /// The files outside the folder that touch it, one hop and never
    /// transitively. Empty for a folder nothing outside it uses.
    pub outside: Vec<OutsideEdge>,
    /// The files inside taking the most dependencies from outside — the
    /// folder's front doors, and the numerator of `entry_concentration`.
    ///
    /// Every file tied at the maximum is listed. Picking one arbitrarily
    /// would paint an honest tie as a breach, which is a defect the reader
    /// would then go looking for and not find.
    pub doors: Vec<String>,
}

impl FolderPicture {
    /// The same picture with every path made relative to the analysis root.
    ///
    /// The graph stores absolute paths; everything that leaves the process
    /// — the JSON renderer, this endpoint, the MCP tools — reports them
    /// root-relative so the answer does not change with the build machine.
    /// Applied to *every* path a picture carries, since a drawing whose
    /// nodes and edges were keyed differently could not be joined up.
    pub fn relative_to(mut self, root: &Path) -> Self {
        self.folder = relative(&self.folder, root);
        for child in &mut self.children {
            child.path = relative(&child.path, root);
        }
        self.rebase_arrows(root);
        for edge in &mut self.outside {
            edge.outside = relative(&edge.outside, root);
            edge.inside = relative(&edge.inside, root);
            edge.child = relative(&edge.child, root);
        }
        self.doors = self.doors.iter().map(|d| relative(d, root)).collect();
        self
    }

    /// The two arrow lists, rebased together — the scored edges and the
    /// erased ones, which are keyed alike and have to stay that way.
    fn rebase_arrows(&mut self, root: &Path) {
        for edge in &mut self.edges {
            edge.rebase(root);
        }
        for edge in &mut self.erased {
            edge.rebase(root);
        }
    }
}

/// Whether a node in the picture is a file or a whole subfolder standing
/// as one circle. The distinction matters to a reader deciding where to
/// go next: a subfolder can be opened, and has a shape of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildKind {
    File,
    Folder,
}

/// One circle in the drawing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PictureChild {
    pub path: String,
    pub kind: ChildKind,
    /// Which row it draws on: the longest path reaching it from a source,
    /// the same levels `layering` is measured against. Every member of a
    /// dependency loop shares one level, the loop having no internal order
    /// to lay out.
    pub level: u32,
    /// Dependencies arriving from outside the folder and landing inside
    /// this child.
    pub inbound: u32,
    /// Whether this child holds one of the folder's doors.
    pub is_door: bool,
}

/// How an edge between two children reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeVerdict {
    /// Steps exactly one level down — the shape a reader can follow.
    Step,
    /// Jumps past a level. What `layering` charges for: the edge you have
    /// to hold in your head while following the rest.
    Skip,
    /// Runs inside a dependency loop, so it has no direction to read. What
    /// `acyclicity` charges for.
    Back,
}

/// One edge in the drawing, and what it costs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PictureEdge {
    pub from: String,
    pub to: String,
    pub verdict: EdgeVerdict,
}

impl PictureEdge {
    /// Both endpoints relative to the analysis root — one arrow's share of
    /// [`FolderPicture::relative_to`].
    fn rebase(&mut self, root: &Path) {
        self.from = relative(&self.from, root);
        self.to = relative(&self.to, root);
    }
}

/// One arrow every import behind it is erased at build (AN-025) — drawn,
/// and left out of every score.
///
/// No verdict, unlike [`PictureEdge`]. `Step`, `Skip` and `Back` are
/// readings of the levels, and the levels are assigned over the scored
/// graph this edge is not in; giving it one would be inventing a place
/// for it in a drawing it was taken out of.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ErasedEdge {
    pub from: String,
    pub to: String,
}

impl ErasedEdge {
    /// As [`PictureEdge::rebase`]. Written twice rather than shared behind
    /// a trait: two fields and two lines, where the trait would be the
    /// larger thing to read.
    fn rebase(&mut self, root: &Path) {
        self.from = relative(&self.from, root);
        self.to = relative(&self.to, root);
    }
}

/// How a dependency crossing the folder's boundary reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutsideVerdict {
    /// Arrives at a door. This is what a folder with a facade looks like.
    Entry,
    /// Arrives somewhere else — an outsider reaching past the front door
    /// into the interior. What `entry_concentration` charges for, and the
    /// thing to fix when a folder is held back by its doors.
    Breach,
    /// Leaves the folder. Where it *lands* is never a defect and is its
    /// target's business — depending outward is what a folder is for. Where
    /// it *starts* is this folder's own shape, and is graded: an exit from a
    /// middle-layer child is what `egress` charges for (AN-028, ADR 0031).
    Exit,
}

/// One dependency crossing the boundary, one hop out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutsideEdge {
    /// The file on the far side, wherever in the repo it lives.
    pub outside: String,
    /// The file inside the folder at this end. A file and not a child, so
    /// a breach names the thing to go and change rather than the subfolder
    /// containing it.
    pub inside: String,
    /// The immediate child of the folder that `inside` sits in — which
    /// circle in the drawing the line attaches to.
    pub child: String,
    pub verdict: OutsideVerdict,
}
