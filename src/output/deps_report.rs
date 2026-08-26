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

use crate::graph::DependencyGraph;
use crate::models::CodeEntity;
use std::collections::HashSet;
use std::fmt::Write;
use std::path::Path;

/// Render the `deps` report for `target`.
///
/// `depth` bounds how many external hops the forward walk reports.
/// `reverse` swaps it for the one-hop "what depends on this file" listing.
pub fn render(graph: &DependencyGraph, target: &Path, depth: usize, reverse: bool) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Dependencies for: {}", target.display());
    let _ = writeln!(output);

    let local: HashSet<&str> = graph
        .entities()
        .filter(|e| e.file_path == target)
        .map(|e| e.id.as_str())
        .collect();

    if reverse {
        write_dependents(&mut output, graph, &local);
    } else {
        write_dependencies(&mut output, graph, &local, depth);
    }

    output
}

/// Breadth-first walk out of the file, one level per external hop. Seeded with
/// every declaration in the file and with those same ids pre-visited, so a
/// same-file edge is skipped rather than reported as a level.
fn levels(graph: &DependencyGraph, local: &HashSet<&str>, depth: usize) -> Vec<Vec<String>> {
    let mut visited: HashSet<String> = local.iter().map(|id| id.to_string()).collect();
    let mut frontier: Vec<String> = visited.iter().cloned().collect();
    frontier.sort();

    let mut levels = Vec::new();
    for _ in 0..depth {
        let mut next: Vec<String> = Vec::new();
        for id in &frontier {
            for (dep, _) in graph.dependencies(id) {
                if visited.insert(dep.id.clone()) {
                    next.push(dep.id.clone());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort();
        levels.push(next.clone());
        frontier = next;
    }
    levels
}

fn write_dependencies(
    output: &mut String,
    graph: &DependencyGraph,
    local: &HashSet<&str>,
    depth: usize,
) {
    let levels = levels(graph, local, depth);
    if levels.is_empty() {
        let _ = writeln!(output, "No dependencies.");
        return;
    }

    let _ = writeln!(output, "Dependencies:");
    for (i, level) in levels.iter().enumerate() {
        let _ = writeln!(output, "  Level {}:", i + 1);
        // The walk orders by id so the traversal is reproducible; the reader
        // wants names, so the printing re-sorts by the label it shows.
        let mut names: Vec<String> = level
            .iter()
            .map(|id| match graph.get_entity(id) {
                Some(dep) => dep.name.clone(),
                None => format!("{} (external)", id),
            })
            .collect();
        names.sort();
        for name in names {
            let _ = writeln!(output, "    → {}", name);
        }
    }
}

fn write_dependents(output: &mut String, graph: &DependencyGraph, local: &HashSet<&str>) {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut rows: Vec<(&CodeEntity, &'static str)> = Vec::new();
    let mut ids: Vec<&str> = local.iter().copied().collect();
    ids.sort();

    for id in ids {
        for (dep, rel) in graph.dependents(id) {
            // A declaration calling its neighbour in the same file does not
            // make the file depend on itself.
            if local.contains(dep.id.as_str()) || !seen.insert(dep.id.as_str()) {
                continue;
            }
            rows.push((dep, rel.kind.display_label()));
        }
    }

    if rows.is_empty() {
        let _ = writeln!(output, "Nothing depends on this file.");
        return;
    }

    rows.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let _ = writeln!(output, "Dependents (what depends on this):");
    for (dep, label) in rows {
        let _ = writeln!(output, "  ← {} ({})", dep.name, label);
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

        let out = render(&graph, &PathBuf::from("many.ts"), 2, false);

        assert_eq!(out.matches("Dependencies:").count(), 1);
        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper\n"
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

        let out = render(&graph, &PathBuf::from("solo.ts"), 2, false);

        assert_eq!(out, "Dependencies for: solo.ts\n\nNo dependencies.\n");
    }

    /// A file mezz could not parse — or one that isn't in the graph at all —
    /// has no seeds, so the walk is empty rather than unbounded.
    #[test]
    fn an_unknown_file_says_it_has_no_dependencies() {
        let graph = graph_of(vec![entity("other.ts", 1, "a")], &[]);

        let out = render(&graph, &PathBuf::from("missing.ts"), 2, false);

        assert_eq!(out, "Dependencies for: missing.ts\n\nNo dependencies.\n");
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

        let out = render(&graph, &PathBuf::from("many.ts"), 2, false);

        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper\n  Level 2:\n    → deep\n"
        );
        assert_eq!(
            render(&graph, &PathBuf::from("many.ts"), 1, false),
            "Dependencies for: many.ts\n\nDependencies:\n  Level 1:\n    → helper\n"
        );
    }

    /// `--reverse` prints its header once too, and lists each dependent once
    /// even when it reaches into several declarations of the file.
    #[test]
    fn reverse_lists_each_dependent_once_under_one_header() {
        let graph = graph_of(
            vec![
                entity("many.ts", 1, "a"),
                entity("many.ts", 2, "b"),
                entity("caller.ts", 1, "caller"),
            ],
            &[(2, 0), (2, 1), (1, 0)],
        );

        let out = render(&graph, &PathBuf::from("many.ts"), 2, true);

        assert_eq!(
            out,
            "Dependencies for: many.ts\n\nDependents (what depends on this):\n  ← caller (calls)\n"
        );
    }

    #[test]
    fn reverse_says_so_when_nothing_depends_on_the_file() {
        let graph = graph_of(
            vec![entity("leaf.ts", 1, "a"), entity("leaf.ts", 2, "b")],
            &[(1, 0)],
        );

        let out = render(&graph, &PathBuf::from("leaf.ts"), 2, true);

        assert_eq!(
            out,
            "Dependencies for: leaf.ts\n\nNothing depends on this file.\n"
        );
    }
}
