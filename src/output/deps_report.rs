//! Text report behind `mezz deps <file>`.
//!
//! The unit is the *file*, not the declarations inside it. Most languages
//! never produce a File-kind entity — a parsed file is a handful of function
//! and type nodes and nothing that stands for the file itself — so the file's
//! dependencies are derived: every declaration in the file is a seed, and an
//! edge that lands back inside the same file is internal wiring, not a
//! dependency of the file on anything.
//!
//! Levels therefore count *external* hops. A call from one function in the
//! file to another costs nothing; the first thing outside the file is level 1.
//!
//! The graph handed here is the **whole repository's** (CLI-001). It has to
//! be: every dependent of a file lives outside it by definition, so a graph
//! built from the target alone answers "nothing depends on this file" for
//! every file in every repo — not because nothing does, but because no edge
//! could have arrived. Both directions here therefore *filter* a project-wide
//! graph down to the target rather than analysing the target in isolation.

use crate::check::{repo_relative, tally};
use crate::graph::DependencyGraph;
use crate::mcp::externals::Externals;
use crate::mcp::tools::{is_listed, lifted};
use crate::models::CodeEntity;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::{Component, Path, PathBuf};

/// Files named under one level before the rest are summarised. A file-level
/// answer over a whole repository fans out fast — level 2 of a well-connected
/// file is most of `src/` — and a report a reader scrolls past is a report
/// they don't read.
const MAX_FILES_PER_LEVEL: usize = 20;

/// Declarations named for one file before the rest are counted.
const MAX_NAMES_PER_FILE: usize = 8;

/// Render the `deps` report for `target`.
///
/// `graph` covers the repository rooted at `root`; `target` narrows it, and
/// every file named in the report is named relative to that root, so the
/// answer does not depend on which directory the reader was standing in
/// (CFG-013). `depth` bounds how many external hops the forward walk reports.
/// `reverse` swaps it for the one-hop "what depends on this file" listing.
pub fn render(
    graph: &DependencyGraph,
    root: &Path,
    target: &Path,
    depth: usize,
    reverse: bool,
) -> String {
    let naming = Naming::rooted_at(root);

    let mut output = String::new();
    let _ = writeln!(output, "Dependencies for: {}", naming.of(target));
    let _ = writeln!(output);

    let local = local_ids(graph, &naming, target);

    // Said rather than answered. With a repo-wide graph an empty seed set no
    // longer means "this file depends on nothing" — it means mezz holds
    // nothing from the file, and both questions are unanswerable. The
    // difference matters most in the case the reader is in: `--reverse`
    // before a delete, where "nothing depends on this" reads as permission.
    if local.is_empty() {
        let _ = writeln!(
            output,
            "No declarations from this file are in the graph — it was not parsed, \
             settings exclude it, or it declares nothing. Neither direction can be \
             answered for it."
        );
        return output;
    }

    if reverse {
        write_dependents(&mut output, graph, &naming, &local);
    } else {
        write_dependencies(&mut output, graph, &naming, &local, target, depth);
    }

    output
}

/// How a path is matched, and how it is printed.
///
/// Three spellings of the same file reach this module — the one the reader
/// typed (`src/a.rs`, `./src/a.rs`, or absolute), the one the walk recorded
/// (`./src/a.rs` when the analysed root was `.`), and the one the report
/// should show. Both questions are answered here so they cannot drift: a
/// mismatch in the first silently empties the report, and a mismatch in the
/// second prints which directory the reader was standing in (CFG-013).
struct Naming {
    /// The repo root, resolved.
    root: PathBuf,
    /// Resolved once: keying is asked of every entity in the graph.
    cwd: PathBuf,
}

impl Naming {
    fn rooted_at(root: &Path) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        Naming {
            root: key(root, &cwd),
            cwd,
        }
    }

    /// One comparable spelling of a path, for matching two of them.
    fn key(&self, path: &Path) -> PathBuf {
        key(path, &self.cwd)
    }

    /// The path as the report names it: relative to the repo root.
    fn of(&self, path: &Path) -> String {
        repo_relative(&self.root, &self.key(path))
    }
}

/// Absolute, with `./` segments dropped. Deliberately not `canonicalize` — it
/// touches the filesystem once per entity over graphs of tens of thousands,
/// and resolving symlinks would answer for a file nobody named.
fn key(path: &Path, cwd: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    absolute
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Every entity the graph holds for `target`.
fn local_ids<'g>(graph: &'g DependencyGraph, naming: &Naming, target: &Path) -> HashSet<&'g str> {
    let target = naming.key(target);
    graph
        .entities()
        // Ghosts carry an empty path, which would otherwise key to the
        // working directory and match a target spelled as one.
        .filter(|e| !e.file_path.as_os_str().is_empty())
        .filter(|e| naming.key(&e.file_path) == target)
        .map(|e| e.id.as_str())
        .collect()
}

/// What a walk out of the file found.
///
/// Targets outside the repo are counted, never listed. They were listed
/// once, and the report read `→ collect`, `→ push`, `→ len` — a file's
/// dependencies given as the methods it calls on its own locals. The count
/// keeps the signal (a file with forty external calls is coupled to
/// something) without the noise. What it could not do was tell a library
/// call from a parser miss, because the `ghost_stdlib`/`ghost_external`
/// tags cannot: `Externals` does that (MCP-039), and the listing that goes
/// with it is `mezz impact --path`'s, which has the rows to spend.
struct Walk<'g> {
    levels: Vec<Vec<String>>,
    /// Direct targets that are not code in this repo, placed.
    outside: Externals<'g>,
}

impl<'g> Walk<'g> {
    /// Decide what one edge target is worth: an id to walk on from, or
    /// nothing — tallied first if it is a direct non-repo target.
    ///
    /// `visited` doubles as the dedupe for the tally: one ghost node stands
    /// for every call to the same name, so inserting it here counts it once
    /// however many declarations of the file reach it.
    fn admit(
        &mut self,
        dep: &'g CodeEntity,
        first_hop: bool,
        visited: &mut HashSet<String>,
    ) -> Option<String> {
        if !visited.insert(dep.id.clone()) {
            return None;
        }
        if dep.tags.contains("ghost") {
            if first_hop {
                self.outside.add(dep);
            }
            return None;
        }
        is_listed(dep).then(|| dep.id.clone())
    }

    /// The counted-not-listed footer, or nothing when everything the file
    /// touches directly resolved to code in the repo.
    fn not_listed(&self) -> Option<String> {
        let (stdlib, third_party, unresolved) = self.outside.tallies();
        let mut parts = Vec::new();
        if stdlib > 0 {
            parts.push(format!("{stdlib} stdlib"));
        }
        if third_party > 0 {
            parts.push(format!("{third_party} third-party"));
        }
        if unresolved > 0 {
            parts.push(format!("{unresolved} unresolved"));
        }
        (!parts.is_empty()).then(|| {
            format!(
                "Called directly but not listed: {}. `mezz impact --path <file>` names them.",
                parts.join(", ")
            )
        })
    }
}

/// Breadth-first walk out of the file, one level per external hop. Seeded with
/// every declaration in the file and with those same ids pre-visited, so a
/// same-file edge is skipped rather than reported as a level.
fn walk<'g>(
    graph: &'g DependencyGraph,
    local: &HashSet<&str>,
    target: &Path,
    depth: usize,
) -> Walk<'g> {
    let mut visited: HashSet<String> = local.iter().map(|id| id.to_string()).collect();
    let mut frontier: Vec<String> = visited.iter().cloned().collect();
    frontier.sort();

    let mut walk = Walk {
        levels: Vec::new(),
        outside: Externals::of_file(graph, target),
    };
    for level in 0..depth {
        let mut next: Vec<String> = Vec::new();
        for id in &frontier {
            for (dep, _) in graph.dependencies(id) {
                if let Some(id) = walk.admit(dep, level == 0, &mut visited) {
                    next.push(id);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort();
        walk.levels.push(next.clone());
        frontier = next;
    }
    walk
}

fn write_dependencies(
    output: &mut String,
    graph: &DependencyGraph,
    naming: &Naming,
    local: &HashSet<&str>,
    target: &Path,
    depth: usize,
) {
    let walk = walk(graph, local, target, depth);

    if walk.levels.is_empty() {
        let _ = writeln!(output, "No dependencies on other files in this repository.");
    } else {
        let _ = writeln!(output, "Dependencies:");
        for (i, level) in walk.levels.iter().enumerate() {
            let _ = writeln!(output, "  Level {}:", i + 1);
            write_files(output, "→", by_file(graph, naming, level));
        }
    }

    if let Some(note) = walk.not_listed() {
        let _ = writeln!(output);
        let _ = writeln!(output, "{}", note);
    }
}

fn write_dependents(
    output: &mut String,
    graph: &DependencyGraph,
    naming: &Naming,
    local: &HashSet<&str>,
) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ids: Vec<&str> = local.iter().copied().collect();
    ids.sort();

    for id in ids {
        for (dep, rel) in graph.dependents(id) {
            // The call may have been written inside an `if` or a `for`, whose
            // synthetic scope node is the edge's source. Naming that would
            // print `if`; dropping it would lose the caller entirely.
            let Some(dep) = lifted(graph, dep) else {
                continue;
            };
            // A declaration calling its neighbour in the same file does not
            // make the file depend on itself.
            if local.contains(dep.id.as_str()) || !seen.insert(dep.id.clone()) {
                continue;
            }
            rows.entry(naming.of(&dep.file_path))
                .or_default()
                .push(format!("{} ({})", dep.name, rel.kind.display_label()));
        }
    }

    if rows.is_empty() {
        let _ = writeln!(output, "Nothing depends on this file.");
        return;
    }

    let _ = writeln!(
        output,
        "Dependents (what depends on this): {} in {}",
        tally(seen.len(), "declaration"),
        tally(rows.len(), "file")
    );
    for names in rows.values_mut() {
        names.sort();
    }
    write_files(output, "←", rows);
}

/// Declaration names grouped under the file that holds them, in path order.
fn by_file(
    graph: &DependencyGraph,
    naming: &Naming,
    ids: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in ids {
        let Some(e) = graph.get_entity(id) else {
            continue;
        };
        rows.entry(naming.of(&e.file_path))
            .or_default()
            .push(e.name.clone());
    }
    for names in rows.values_mut() {
        names.sort();
        names.dedup();
    }
    rows
}

/// One line per file, capped at both axes — the report is a shape, and a
/// reader who needs the full list has `mezz impact` for it.
fn write_files(output: &mut String, arrow: &str, rows: BTreeMap<String, Vec<String>>) {
    let total = rows.len();
    for (i, (file, names)) in rows.into_iter().enumerate() {
        if i == MAX_FILES_PER_LEVEL {
            let _ = writeln!(output, "    … and {} more", tally(total - i, "file"));
            return;
        }
        let _ = writeln!(output, "    {} {}: {}", arrow, file, capped(&names));
    }
}

fn capped(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(MAX_NAMES_PER_FILE)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().saturating_sub(MAX_NAMES_PER_FILE) {
        0 => shown,
        rest => format!("{} (+{} more)", shown, rest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Span};
    use std::path::PathBuf;

    fn entity(file: &str, line: usize, name: &str) -> CodeEntity {
        CodeEntity::new(
            name,
            EntityKind::Function,
            file,
            Span::from_positions(line, 0, line, 0),
        )
    }

    fn graph_of(entities: Vec<CodeEntity>, edges: &[(usize, usize)]) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|&(s, t)| {
                Relationship::new(&entities[s].id, &entities[t].id, RelationshipKind::Calls)
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

    /// Five declarations in one file, one edge leaving it. Before this
    /// module the caller looped over every entity in the file and printed a
    /// `Dependencies:` header per iteration — nine of ten of them empty.
    #[test]
    fn one_header_for_a_file_of_many_declarations() {
        let graph = graph_of(
            vec![
                entity("many.ts", 1, "a"),
                entity("many.ts", 2, "b"),
                entity("many.ts", 3, "c"),
                entity("many.ts", 4, "d"),
                entity("many.ts", 5, "e"),
                entity("helper.ts", 1, "helper"),
            ],
            &[(0, 5), (1, 0), (2, 1), (3, 2), (4, 3)],
        );

        let out = render(&graph, Path::new("."), &PathBuf::from("many.ts"), 2, false);

        assert_eq!(out.matches("Dependencies:").count(), 1);
        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper.ts: helper\n"
        );
    }

    /// The file's own wiring is not a dependency of the file, so a file that
    /// only calls itself reports nothing rather than a header over nothing.
    #[test]
    fn a_self_contained_file_says_it_has_no_dependencies() {
        let graph = graph_of(
            vec![entity("solo.ts", 1, "a"), entity("solo.ts", 2, "b")],
            &[(1, 0)],
        );

        let out = render(&graph, Path::new("."), &PathBuf::from("solo.ts"), 2, false);

        assert_eq!(
            out,
            "Dependencies for: solo.ts\n\nNo dependencies on other files in this repository.\n"
        );
    }

    /// CLI-001. A file mezz holds nothing from says so, in both directions.
    /// It used to answer "No dependencies" and "Nothing depends on this
    /// file" — two confident answers to a question that was never asked of
    /// the graph, and the second is the one a reader acts on before a delete.
    #[test]
    fn a_file_the_graph_does_not_hold_says_so_rather_than_answering() {
        let graph = graph_of(vec![entity("other.ts", 1, "a")], &[]);

        for reverse in [false, true] {
            let out = render(
                &graph,
                Path::new("."),
                &PathBuf::from("missing.ts"),
                2,
                reverse,
            );
            assert!(out.contains("No declarations from this file"), "{out}");
            assert!(!out.contains("Nothing depends on this file"), "{out}");
        }
    }

    /// Levels count hops *outside* the file: `b -> a` is free, so `helper`
    /// stays at level 1 and its own callee lands at level 2.
    #[test]
    fn levels_count_external_hops_only() {
        let graph = graph_of(
            vec![
                entity("many.ts", 1, "a"),
                entity("many.ts", 2, "b"),
                entity("helper.ts", 1, "helper"),
                entity("deep.ts", 1, "deep"),
            ],
            &[(1, 0), (0, 2), (2, 3)],
        );

        let out = render(&graph, Path::new("."), &PathBuf::from("many.ts"), 2, false);

        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper.ts: helper\n  \
             Level 2:\n    → deep.ts: deep\n"
        );
        assert_eq!(
            render(&graph, Path::new("."), &PathBuf::from("many.ts"), 1, false),
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper.ts: helper\n"
        );
    }

    /// The spelling the walk recorded and the spelling the reader typed are
    /// not the same string: analysing `.` gives every entity a `./` prefix.
    /// Matching on the string alone made every file look absent from its own
    /// repository's graph.
    #[test]
    fn a_target_matches_however_its_path_is_spelled() {
        let graph = graph_of(
            vec![entity("./src/a.ts", 1, "a"), entity("./src/b.ts", 1, "b")],
            &[(0, 1)],
        );

        for spelling in ["src/a.ts", "./src/a.ts", "./src/./a.ts"] {
            let out = render(&graph, Path::new("."), &PathBuf::from(spelling), 1, false);
            assert!(out.contains("→ src/b.ts: b"), "{spelling}: {out}");
        }
    }

    /// CLI-001. Third-party and unbindable targets are counted, not listed:
    /// the report used to answer "what does this file depend on" with
    /// `collect`, `push` and `len`. MCP-039 split the count, so a library
    /// call and a name mezz could not bind are no longer one number.
    #[test]
    fn stdlib_and_unresolved_targets_are_counted_rather_than_named() {
        let mut caller = entity("app.ts", 1, "caller");
        caller.id = "app::caller".to_string();
        let entities = vec![caller, entity("lib.ts", 1, "real")];
        let relationships = vec![
            Relationship::new("app::caller", &entities[1].id, RelationshipKind::Calls),
            Relationship::new(
                "app::caller",
                "std::fs::read_to_string",
                RelationshipKind::Calls,
            ),
            Relationship::new("app::caller", "who_knows", RelationshipKind::Calls),
        ];
        let graph = DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        });

        let out = render(&graph, Path::new("."), &PathBuf::from("app.ts"), 2, false);

        assert!(out.contains("→ lib.ts: real"), "{out}");
        assert!(!out.contains("read_to_string"), "{out}");
        assert!(!out.contains("who_knows"), "{out}");
        assert!(
            out.contains("Called directly but not listed: 1 stdlib, 1 unresolved."),
            "{out}"
        );
    }

    /// `--reverse` names the *files* that depend on this one, each dependent
    /// once, however many declarations of the file it reaches into.
    #[test]
    fn reverse_groups_dependents_under_the_files_they_live_in() {
        let graph = graph_of(
            vec![
                entity("many.ts", 1, "a"),
                entity("many.ts", 2, "b"),
                entity("caller.ts", 1, "caller"),
                entity("caller.ts", 2, "other"),
                entity("far.ts", 1, "far"),
            ],
            &[(2, 0), (2, 1), (3, 0), (4, 1), (1, 0)],
        );

        let out = render(&graph, Path::new("."), &PathBuf::from("many.ts"), 2, true);

        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependents (what depends on this): 3 declarations in \
             2 files\n    ← caller.ts: caller (calls), other (calls)\n    ← far.ts: far (calls)\n"
        );
    }

    /// A call written inside an `if` leaves from the branch node, not from
    /// the function holding it. Listing the branch would print `if`; dropping
    /// it would lose the caller — so it is lifted to the declaration a reader
    /// can open.
    #[test]
    fn a_dependent_calling_from_inside_a_branch_is_named_by_its_declaration() {
        let caller = entity("caller.ts", 1, "caller");
        let branch = CodeEntity::new(
            "if",
            EntityKind::Branch,
            "caller.ts",
            Span::from_positions(2, 0, 4, 0),
        )
        .with_parent(caller.id.clone());
        let graph = graph_of(vec![entity("many.ts", 1, "a"), caller, branch], &[(2, 0)]);

        let out = render(&graph, Path::new("."), &PathBuf::from("many.ts"), 2, true);

        assert!(out.contains("← caller.ts: caller (calls)"), "{out}");
        assert!(!out.contains("if ("), "{out}");
    }

    #[test]
    fn reverse_says_so_when_nothing_depends_on_the_file() {
        let graph = graph_of(
            vec![entity("leaf.ts", 1, "a"), entity("leaf.ts", 2, "b")],
            &[(1, 0)],
        );

        let out = render(&graph, Path::new("."), &PathBuf::from("leaf.ts"), 2, true);

        assert_eq!(
            out,
            "Dependencies for: leaf.ts\n\nNothing depends on this file.\n"
        );
    }
}
