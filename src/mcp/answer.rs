//! What a tool hands back, before either renderer has had it.
//!
//! Every tool used to return a `String`, so the only thing a caller could
//! do with an answer was read it (CLI-003). A converted tool now computes
//! one value and renders it twice: the prose it always printed, and the
//! JSON a script can parse. Both come off the same walk of the graph —
//! that is the whole point, and it is the same rule CLI-002 applied one
//! level up when it gave the tools a second front door.
//!
//! This module owns the parts that are the same for every tool: the
//! envelope, the scope digest, and the caveat about a holed graph. The
//! per-tool shapes live beside the tools that compute them.

use std::path::Path;

use serde_json::{json, Value};

use crate::models::CodeEntity;

use super::{tools, McpServer};

/// The revision of the shape below. An integer, because a consumer's only
/// sensible question is "is this the one I was written against".
///
/// Bumped when a field is renamed, removed or retyped — *not* when one is
/// added. See ADR 0035: that asymmetry is what makes the output safe to
/// parse from a CI job while still leaving room to say more.
pub(crate) const SCHEMA_VERSION: u64 = 1;

/// One tool's answer: the prose, and — once the tool is converted — the
/// value the prose was rendered from.
///
/// `data: None` is not "no data". It is "this tool has not been converted
/// yet", and it is what lets `--format json` fail by name rather than
/// print an empty object over a tool that in fact has plenty to say.
pub(crate) struct Answer {
    text: String,
    data: Option<Value>,
}

impl Answer {
    /// A tool that still builds its answer as lines of prose.
    pub(crate) fn prose(text: String) -> Self {
        Self { text, data: None }
    }

    /// A converted tool: the two renderings of one value.
    pub(crate) fn structured(text: String, data: Value) -> Self {
        Self {
            text,
            data: Some(data),
        }
    }

    /// The prose, as every caller has always received it.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The prose, for a caller that wants it and is done with the rest.
    pub(crate) fn into_text(self) -> String {
        self.text
    }

    /// The value, for the callers that can use one.
    pub(crate) fn data(&self) -> Option<&Value> {
        self.data.as_ref()
    }
}

/// What produced this answer, as a footer line (CFG-014).
///
/// Two facts, because the two ways a long-running server goes stale are
/// not detectable the same way. A scope change the server can see, so the
/// digest moves on its own. A replaced binary it cannot see at all — the
/// process goes on running the code it was started with — so the version
/// is printed for the *reader*, who can compare it against the one they
/// just installed. That asymmetry is why this is a footer rather than a
/// reload.
pub(crate) fn footer(server: &McpServer) -> String {
    format!(
        "\n\n_scope {} · mezz {}{}_",
        server.scope_id(),
        env!("CARGO_PKG_VERSION"),
        ImportCoverage::of(server).map_or_else(String::new, |c| c.note(&server.root)),
    )
}

/// The same two facts, plus the caveats, as fields (CLI-003).
///
/// The footer's honesty is load-bearing — the digest is how two answers
/// under different settings are told apart, and the coverage note is what
/// separates a clean graph from a holed one — so it cannot become a
/// trailing string a consumer has to regex. It becomes structure.
pub(crate) fn envelope(server: &McpServer, tool: &str, data: Value) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "tool": tool,
        "mezz_version": env!("CARGO_PKG_VERSION"),
        "scope": {
            "digest": server.scope_id(),
            "root": server.root.display().to_string(),
            "include_tests": server.include_tests,
            "languages": server.languages,
        },
        "caveats": ImportCoverage::of(server)
            .map(|c| vec![c.as_caveat(&server.root)])
            .unwrap_or_default(),
        "data": data,
    })
}

/// What the tool was pointed at, spelled the way the caller passed it:
/// relative to the root, and `"."` for the root itself.
///
/// The prose says "the project" there, which is the right thing to read
/// and the wrong thing to compare against a path.
pub(crate) fn scope_path(path: &Path, root: &Path) -> String {
    match tools::rel_path(path, root).as_str() {
        "" => ".".to_string(),
        p => p.to_string(),
    }
}

/// One entity, the way every structured answer spells it.
///
/// The machine-readable counterpart of [`tools::metric_suffix`], and
/// deliberately the same facts: a row reading `cx 14, out 9` in the prose
/// reads `"cyclomatic": 14, "fan_out": 9` here.
///
/// Two deliberate differences. A metric the parser never measured is
/// `null`, never `0` — zero is a measurement, and the suffix says the same
/// thing by omitting the part. And a metric that *is* zero is still
/// present, where the suffix drops it: a consumer filtering on `fan_in`
/// should not have to tell "no callers" apart from "field missing".
pub(crate) fn entity_json(e: &CodeEntity, base: &Path) -> Value {
    let m = &e.metrics;
    json!({
        "kind": e.kind.display_name(),
        "name": e.name,
        "file": tools::rel_path(&e.file_path, base),
        "line": e.span.start.line + 1,
        "metrics": {
            "loc": m.loc,
            "cyclomatic": m.cyclomatic,
            "cognitive": m.cognitive_complexity,
            "working_set": m.working_set,
            "methods": m.method_count,
            "fan_in": m.fan_in,
            "fan_out": m.fan_out,
            "in_cycle": m.in_cycle,
            "smells": m.smells.iter().map(|s| s.label()).collect::<Vec<_>>(),
        },
    })
}

/// How many folders the footer names before it stops, and why three.
///
/// The line is appended to *every* response, so it is charged against every
/// answer; three names fit on one wrapped line and are enough for a reader to
/// find their own folder or establish it is not there. The rest are counted,
/// never silently dropped — and in JSON, where no line has to wrap, they are
/// all carried.
pub(crate) const NAMED_HOLED_FOLDERS: usize = 3;

/// How much of what the parsers read reached the graph, and where the
/// holes are.
///
/// Every verdict these tools give is computed over the graph, so a hole in it
/// becomes a confident wrong answer: a folder is reported as a funnel because
/// the imports that would have shown otherwise are missing, not because
/// nothing leaves it. Silence is indistinguishable from cleanliness, and this
/// is what separates them.
///
/// A value rather than a formatted line, because it is now said twice — in
/// the footer and in `caveats` — and two spellings of one fact is exactly
/// the drift this ticket is about.
struct ImportCoverage {
    landed: usize,
    seen: usize,
    /// Folders the missing imports cross, worst first. Complete: the
    /// footer takes the first three, the JSON takes them all.
    folders: Vec<(String, usize)>,
}

impl ImportCoverage {
    /// The coverage of the warm-cached analysis the tool just used, and
    /// `None` when there is nothing to report — either the graph is whole,
    /// or it could not be obtained at all, in which case the tool's own
    /// error has already said so.
    ///
    /// Quiet when whole, so a sound graph costs no tokens and the line
    /// means something when it appears.
    fn of(server: &McpServer) -> Option<Self> {
        let graph = tools::analyze(server, &server.root).ok()?;
        let (landed, seen, folders) = graph.import_coverage_by_folder();
        (landed < seen).then_some(Self {
            landed,
            seen,
            folders,
        })
    }

    /// How many imports never landed. Never zero — see [`Self::of`].
    fn missing(&self) -> usize {
        self.seen.saturating_sub(self.landed)
    }

    /// The footer clause, named folders and all.
    fn note(&self, root: &Path) -> String {
        format!(
            " · {} of {} imports in the graph — {} missing, so any verdict \
             over the folders they cross is unsound{}",
            self.landed,
            self.seen,
            self.missing(),
            holed_folders(&self.folders, root),
        )
    }

    /// The same fact as one entry of the `caveats` array.
    ///
    /// `kind` leads so a consumer can switch on it: this is the only
    /// caveat today and will not be the only one for long.
    fn as_caveat(&self, root: &Path) -> Value {
        json!({
            "kind": "missing_imports",
            "landed": self.landed,
            "seen": self.seen,
            "missing": self.missing(),
            "folders": self.folders
                .iter()
                .map(|(folder, n)| json!({
                    "folder": super::reshape::rel(Path::new(folder), root),
                    "imports": n,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Which folders the missing imports cross, so the caveat can be checked
/// against the answer it qualifies.
///
/// A field report (2026-08-27) had three `fan_out` deltas and this footer in
/// the same response, and no way to intersect them: establishing that the
/// entities it was about to quote were sound took a separate `mezz deps` call
/// and a manual read, covering one file of the three. The footer was asking
/// every reader to do that, on every response, for every entity — one
/// unresolvable doubt turned into N manual checks.
///
/// Counts overlap by construction — an import is counted against both ends —
/// so they are not summed and not presented as a partition of the total.
fn holed_folders(ranked: &[(String, usize)], root: &Path) -> String {
    if ranked.is_empty() {
        return String::new();
    }
    let named: Vec<String> = ranked
        .iter()
        .take(NAMED_HOLED_FOLDERS)
        .map(|(folder, n)| format!("`{}` ({n})", super::reshape::rel(Path::new(folder), root)))
        .collect();
    let rest = ranked.len().saturating_sub(named.len());
    let tail = match rest {
        0 => String::new(),
        1 => ", and 1 more folder".to_string(),
        n => format!(", and {n} more folders"),
    };
    format!(": {}{}", named.join(", "), tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Field report, 2026-08-27: the footer's caveat was unlocalisable — "any
    /// verdict over the folders they cross is unsound" over folders it never
    /// named. The reporter quoted a `fan_out` delta anyway, because checking
    /// it took a `mezz deps` call per file.
    #[test]
    fn the_footer_names_the_folders_its_caveat_is_about() {
        let root = Path::new("/repo");
        let ranked = vec![
            ("/repo/frontend/src/lib/components".to_string(), 31),
            ("/repo/frontend/src/routes".to_string(), 22),
            ("/repo/backend/src/agent".to_string(), 9),
            ("/repo/backend/src/models".to_string(), 4),
            ("/repo/backend/src/routes".to_string(), 2),
        ];
        let note = holed_folders(&ranked, root);

        assert_eq!(
            note,
            ": `frontend/src/lib/components` (31), `frontend/src/routes` (22), \
             `backend/src/agent` (9), and 2 more folders"
        );
        assert!(
            !note.contains("/repo/"),
            "folders are named the way the reader spells them:\n{note}"
        );
    }

    /// Nothing to name is not a reason to print an empty clause, and a single
    /// leftover folder is not "1 more folders".
    #[test]
    fn the_footer_folder_clause_is_silent_when_it_has_nothing_to_add() {
        let root = Path::new("/repo");
        assert_eq!(holed_folders(&[], root), "");
        let four = vec![
            ("/repo/a".to_string(), 3),
            ("/repo/b".to_string(), 2),
            ("/repo/c".to_string(), 1),
            ("/repo/d".to_string(), 1),
        ];
        assert!(
            holed_folders(&four, root).ends_with(", and 1 more folder"),
            "{}",
            holed_folders(&four, root)
        );
    }

    /// The footer names three folders because a line has to wrap; JSON has
    /// no such budget, and a consumer intersecting the caveat against its
    /// own folder needs all of them (ADR 0035).
    #[test]
    fn the_caveat_carries_every_folder_the_footer_had_to_drop() {
        let coverage = ImportCoverage {
            landed: 812,
            seen: 900,
            folders: vec![
                ("/repo/a".to_string(), 31),
                ("/repo/b".to_string(), 22),
                ("/repo/c".to_string(), 9),
                ("/repo/d".to_string(), 4),
                ("/repo/e".to_string(), 2),
            ],
        };
        let root = Path::new("/repo");
        let caveat = coverage.as_caveat(root);

        assert_eq!(caveat["kind"], json!("missing_imports"));
        assert_eq!(caveat["missing"], json!(88));
        let folders = caveat["folders"].as_array().expect("folders is an array");
        assert_eq!(
            folders.len(),
            5,
            "the footer names {NAMED_HOLED_FOLDERS}; the caveat names them all"
        );
        assert_eq!(folders[0]["folder"], json!("a"));
        assert_eq!(folders[0]["imports"], json!(31));
        assert!(coverage.note(root).contains("and 2 more folders"));
    }
}
