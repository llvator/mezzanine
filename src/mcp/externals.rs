//! The half of a dependency list that leaves the repo (MCP-039).
//!
//! `impact` knew how many targets an entity reached that were not code in
//! this tree, and said only that: *plus 9 external/unresolved targets not
//! listed*. Nine is the whole answer when the resolved list is empty, and
//! it merges two populations that mean opposite things:
//!
//! - **External** — a real call into a library. It is a dependency of the
//!   code, and the thing a reader asking "what does this rely on" wants
//!   named.
//! - **Unresolved** — a call mezz could not bind to anything. It is a hole
//!   in the graph, and evidence that the *other* numbers in the same
//!   response are a floor rather than a total.
//!
//! A file with nine library calls and a file the parser missed nine times
//! are different files. This module tells them apart and names both.
//!
//! ## What can be placed, and what cannot
//!
//! Mezzanine does not carry external imports — `use tree_sitter::Node` is
//! not in the graph, only file-to-file import sites are — so a target is
//! placed from its own name and nothing else. Three questions, in order of
//! how much they prove:
//!
//! 1. Does the name claim a reserved namespace (`std::fs::write`)?
//! 2. Is the owner — the module or type the call hangs off — one the
//!    language ships (`Vec::push`, `fmt.Println`)?
//! 3. Does the owner read as a module or a type at all, rather than as a
//!    local variable (`Node::walk` yes, `body::push` no)?
//!
//! The third is what earns the "third-party" label, and it is deliberately
//! strict: a qualifier is proof only when it is capitalised or itself
//! qualified. `x.push(y)` reaches the graph as `body::push`, exactly the
//! shape `serde_json::json` has, and a lowercase single segment cannot
//! tell a crate from a local. Calling that one a dependency on a library
//! named `body` is how `mezz deps` came to report `collect`, `push` and
//! `len` as a file's dependencies (CLI-001), so it is not called one.
//!
//! An owner the repo itself declares is unresolved too, never third-party:
//! mezz holds that type and still failed to bind the call, which is a hole
//! and not a dependency. The same two proofs settle a bare capitalised
//! name — a type from a signature, `fn f(n: Node)` — which is third-party
//! when this repo declares nothing by that name, and a miss when it does.
//!
//! Unresolved targets keep whatever qualifier they came with —
//! `serde_json::json` is listed under that name, not as a bare `json` — so
//! a reader who recognises the library is not made to guess which of two
//! populations it fell into. What mezz will not do is claim to know.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::check::tally;
use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, Relationship, RelationshipKind};
use crate::parser::stdlib;

use super::tools::is_listed;

/// Modules or types named before the rest is counted.
const MAX_TARGETS: usize = 12;
/// Members named per target.
const MAX_MEMBERS: usize = 6;
/// Unresolved names listed before the rest is counted.
const MAX_NAMES: usize = 10;

/// What a target that is not code in this repo turned out to be.
///
/// Ordered so third-party sorts before stdlib: "which libraries does this
/// depend on" is the question being asked, and `Vec::push` is not one of
/// the answers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    /// A module or type this repo does not declare and the language does
    /// not ship: a dependency on something installed.
    ThirdParty,
    /// The language's own standard library or builtins.
    Stdlib,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::ThirdParty => "third-party",
            Kind::Stdlib => "stdlib",
        }
    }
}

/// Where one unbound target belongs.
enum Placed {
    /// A module or type that can be named, and the member reached on it.
    /// The owner is empty when the language's own name was recognised but
    /// nothing said what it hung off — a bare `println`, or a `clone` on a
    /// receiver mezz could not type.
    Named(Kind, String, String),
    /// A name mezz could not place at all.
    Nameless(String),
}

/// The unbound targets one subject — an entity, or a file — reaches, by
/// what they turned out to be.
pub(crate) struct Externals<'g> {
    language: Language,
    /// Every name the repo declares, so an owner mezz holds is reported as
    /// a miss rather than as a library.
    declared: HashSet<&'g str>,
    /// (kind, owner) → member → call sites.
    named: BTreeMap<(Kind, String), BTreeMap<String, usize>>,
    /// Unplaceable name → call sites.
    nameless: BTreeMap<String, usize>,
}

impl<'g> Externals<'g> {
    /// An accumulator for a subject written in `language`.
    pub(crate) fn new(graph: &'g DependencyGraph, language: Language) -> Self {
        Externals {
            language,
            declared: graph
                .entities()
                .filter(|e| is_listed(e))
                .map(|e| e.name.as_str())
                .collect(),
            named: BTreeMap::new(),
            nameless: BTreeMap::new(),
        }
    }

    /// The same, for a subject that lives in one file — the language is
    /// the file's.
    pub(crate) fn of_file(graph: &'g DependencyGraph, file: &Path) -> Self {
        Self::new(graph, Language::from_path(file))
    }

    /// Count the targets of `edges` that are not code in this repo.
    ///
    /// Which edges those are is decided here rather than by the caller: a
    /// section that had to remember "ghosts, and not `Contains`" before it
    /// could ask is a section that can get it wrong. Callers doing their
    /// own partitioning for other reasons — the file view, which sorts
    /// every edge into three buckets — reach for [`Self::add`] directly.
    pub(crate) fn gather(&mut self, edges: &[(&CodeEntity, &Relationship)]) {
        for (target, rel) in edges {
            if target.tags.contains("ghost") && rel.kind != RelationshipKind::Contains {
                self.add(target);
            }
        }
    }

    /// Count one edge whose target the graph could not bind to repo code.
    /// Called once per edge, so the counts are call sites rather than
    /// distinct names.
    pub(crate) fn add(&mut self, ghost: &CodeEntity) {
        match self.place(ghost) {
            Placed::Named(kind, owner, member) => {
                *self
                    .named
                    .entry((kind, owner))
                    .or_default()
                    .entry(member)
                    .or_default() += 1;
            }
            Placed::Nameless(name) => *self.nameless.entry(name).or_default() += 1,
        }
    }

    /// Which of the three answers this target gets. See the module header
    /// for why the order of the questions is the order of what they prove.
    fn place(&self, ghost: &CodeEntity) -> Placed {
        let qualified = match ghost.qualified_name.is_empty() {
            true => ghost.name.as_str(),
            false => ghost.qualified_name.as_str(),
        };
        let (raw_owner, member) = split(self.language, qualified);
        // An owner that is not a path of plain identifiers is an
        // expression — `entity.id`, `node_text(src)` — and places nothing,
        // nor is it worth printing.
        let owner = raw_owner.filter(|o| is_name_path(self.language, o));

        if self.is_stdlib(ghost, qualified, owner, member) {
            // `b::len` is a standard-library name on a receiver called
            // `b`. The name is worth reporting; the receiver is not.
            let owner = owner.filter(|o| {
                stdlib::is_stdlib_owner(self.language, o) || reads_as_a_module(self.language, o)
            });
            return Placed::Named(Kind::Stdlib, owner.unwrap_or_default().into(), member.into());
        }
        match self.third_party(raw_owner, owner, member) {
            Some(placed) => placed,
            // Nothing this repo can be told about: an unqualified name, or
            // a qualifier that is some local's source text. It keeps
            // whatever it arrived with — `serde_json::json` is listed
            // under that name — because the reader may place what mezz
            // cannot.
            None => Placed::Nameless(match owner {
                Some(owner) => format!("{owner}{}{member}", separator(self.language)),
                None => member.to_string(),
            }),
        }
    }

    /// Whether the language itself owns this name — by namespace, by the
    /// graph's own ghost category, or by the type it hangs off.
    fn is_stdlib(
        &self,
        ghost: &CodeEntity,
        qualified: &str,
        owner: Option<&str>,
        member: &str,
    ) -> bool {
        stdlib::is_stdlib_path(self.language, qualified)
            || ghost.tags.contains("ghost_stdlib")
            || stdlib::is_stdlib_owner(self.language, owner.unwrap_or(member))
            // No owner to place, so the parser's own table is the last
            // thing that can recognise the name.
            || (owner.is_none_or(|o| !reads_as_a_module(self.language, o))
                && stdlib::is_stdlib_member(self.language, member))
    }

    /// The target as a dependency on something installed, or `None` when
    /// nothing about the name proves one.
    fn third_party(
        &self,
        raw_owner: Option<&str>,
        owner: Option<&str>,
        member: &str,
    ) -> Option<Placed> {
        let declared = |name: &str| self.declared.contains(root_segment(self.language, name));
        match owner {
            // The repo declares this owner, so the call is a miss on code
            // mezz holds, not a reach outside it.
            Some(owner) if declared(owner) => None,
            Some(owner) if reads_as_a_module(self.language, owner) => Some(Placed::Named(
                Kind::ThirdParty,
                owner.to_string(),
                member.to_string(),
            )),
            Some(_) => None,
            // A capitalised bare name is a type — the same proof a
            // capitalised owner carries — and one this repo does not
            // declare is a type from somewhere installed. It arrives
            // unqualified because a signature named it: `fn f(n: Node)`.
            None if raw_owner.is_none()
                && reads_as_a_module(self.language, member)
                && !declared(member) =>
            {
                Some(Placed::Named(
                    Kind::ThirdParty,
                    String::new(),
                    member.to_string(),
                ))
            }
            None => None,
        }
    }

    /// Distinct targets, as `(stdlib, third-party, unresolved)` — the
    /// three numbers a one-line summary needs.
    ///
    /// Targets, not owners: `Node::walk` and `Node::kind` are two things
    /// this code depends on, printed as one row.
    pub(crate) fn tallies(&self) -> (usize, usize, usize) {
        let of = |want: Kind| {
            self.named
                .iter()
                .filter(|((kind, _), _)| *kind == want)
                .map(|(_, members)| members.len())
                .sum()
        };
        (of(Kind::Stdlib), of(Kind::ThirdParty), self.nameless.len())
    }

    /// The two sections, ready to append to a report body. `subject` names
    /// what the counts are about — "this entity", "this file".
    pub(crate) fn sections(&self, subject: &str) -> Vec<String> {
        let mut body = vec![String::new()];
        body.extend(self.external_section(subject));
        body.push(String::new());
        body.extend(self.unresolved_section());
        body
    }

    /// What was reached outside the repo, grouped by the module or type
    /// the name hangs off.
    fn external_section(&self, subject: &str) -> Vec<String> {
        let rows = self.rows();
        let calls: usize = rows.iter().map(|row| row.calls).sum();
        let mut body = vec![format!(
            "## External ({}) — library code {} relies on, which the list above cannot name",
            match rows.is_empty() {
                true => "0 calls".to_string(),
                false => format!("{} to {}", tally(calls, "call"), tally(rows.len(), "target")),
            },
            subject,
        )];
        if rows.is_empty() {
            return body;
        }
        body.push(
            "Grouped by the module or type the call hangs off, counted rather than listed per \
             call site. `stdlib` is the language's own; `third-party` is an owner neither the \
             language nor this repo declares."
                .to_string(),
        );
        body.extend(rows.iter().take(MAX_TARGETS).map(Row::line));
        if rows.len() > MAX_TARGETS {
            body.push(format!(
                "… and {}. The cap is {MAX_TARGETS}.",
                tally(rows.len() - MAX_TARGETS, "more target")
            ));
        }
        body
    }

    /// What could not be placed at all — and the caveat it puts on every
    /// other number in the response.
    fn unresolved_section(&self) -> Vec<String> {
        let calls: usize = self.nameless.values().sum();
        if self.nameless.is_empty() {
            return vec![
                "## Unresolved (0 calls) — every direct target was placed, so the counts above \
                 are totals rather than floors"
                    .to_string(),
            ];
        }
        let (listed, dropped) = counted_names(&self.nameless, MAX_NAMES);
        let rest = match dropped {
            0 => String::new(),
            n => format!(", … and {}", tally(n, "more name")),
        };
        vec![
            format!(
                "## Unresolved ({} to {}) — mezz could not bind these to anything, so every \
                 count above is a floor, not a total",
                tally(calls, "call"),
                tally(self.nameless.len(), "name"),
            ),
            "A parser miss, a call form the language's extractor does not reach, or a receiver \
             whose type nothing declared. Qualified where the name arrived qualified, because \
             the half that says what it was called on is the half mezz could not place."
                .to_string(),
            format!("- {listed}{rest}"),
        ]
    }

    /// The external rows in reading order: third-party before stdlib, then
    /// most-reached first, then by name so two runs over one graph print
    /// identically (AN-002).
    fn rows(&self) -> Vec<Row<'_>> {
        let mut rows: Vec<Row> = self
            .named
            .iter()
            .map(|((kind, owner), members)| Row {
                kind: *kind,
                owner,
                members,
                calls: members.values().sum(),
            })
            .collect();
        rows.sort_by_key(|row| (row.kind, std::cmp::Reverse(row.calls), row.owner));
        rows
    }
}

/// One module or type, and the members of it this subject reached.
struct Row<'a> {
    kind: Kind,
    /// Empty when the name was recognised but nothing said what it hung
    /// off — a bare `println`, or a `clone` on an untyped receiver.
    owner: &'a str,
    members: &'a BTreeMap<String, usize>,
    calls: usize,
}

impl Row<'_> {
    /// `- third-party `Node` — `walk` (3×), `kind` (1×)`, or, with nothing
    /// to hang it off, `- stdlib — `println` (2×)`.
    fn line(&self) -> String {
        let (listed, dropped) = counted_names(self.members, MAX_MEMBERS);
        let rest = match dropped {
            0 => String::new(),
            n => format!(", +{n} more"),
        };
        let owner = match self.owner.is_empty() {
            true => String::new(),
            false => format!(" `{}`", self.owner),
        };
        format!("- {}{} — {}{}", self.kind.label(), owner, listed, rest)
    }
}

/// `` `walk` (3×), `kind` (1×) `` — most-reached first, then by name so the
/// order is the graph's and not the map's — with however many were left
/// over the cap.
fn counted_names(counts: &BTreeMap<String, usize>, cap: usize) -> (String, usize) {
    let mut names: Vec<(&String, &usize)> = counts.iter().collect();
    names.sort_by_key(|(name, calls)| (std::cmp::Reverse(**calls), *name));
    let listed: Vec<String> = names
        .iter()
        .take(cap)
        .map(|(name, calls)| format!("`{name}` ({calls}×)"))
        .collect();
    (listed.join(", "), names.len().saturating_sub(cap))
}

/// The owner and the member of a target name: `Vec::push` splits,
/// `println` does not.
///
/// Split on the language's own qualifier, so a `.` inside a Rust owner
/// stays where it belongs — in `entity.id::clone` the owner is the
/// expression `entity.id`, and calling it a two-segment module path would
/// place a local variable as a library.
pub(super) fn split(language: Language, qualified: &str) -> (Option<&str>, &str) {
    let (owner, member) = match qualified.rsplit_once(separator(language)) {
        Some((owner, member)) if !owner.is_empty() && !member.is_empty() => (Some(owner), member),
        _ => (None, qualified),
    };
    // A parser that spells its qualifier the other way round still yields
    // a member name rather than a whole path.
    (owner, member.rsplit(['.', ':']).next().unwrap_or(member))
}

fn separator(language: Language) -> &'static str {
    match language {
        Language::Rust | Language::Cpp | Language::C | Language::Ruby | Language::PHP => "::",
        _ => ".",
    }
}

/// True when every segment of `owner` is a plain identifier — the one test
/// that separates a module path from a receiver expression.
fn is_name_path(language: Language, owner: &str) -> bool {
    !owner.is_empty()
        && owner.split(separator(language)).all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
}

/// True when a name path reads as a module or a type rather than as a
/// local variable: it is capitalised, or — in a `::` language only — it is
/// itself qualified.
///
/// The lowercase single segment is the ambiguous one — `serde_json::json`
/// and `body::push` are the same shape — and it is deliberately excluded.
/// A capitalised qualifier is proof of a type in every language whose
/// locals are conventionally lowercase.
///
/// Qualification is proof only where field access is spelled differently
/// from a module path. In Rust `a::b::c` cannot be a field chain, because
/// one of those is written `a.b.c` and would not be a name path at all. In
/// a language that spells both with a dot, `options.config.get` and
/// `com.acme.Widget.of` are the same shape, and treating the first as a
/// library called `options.config` is the failure CLI-001 removed.
///
/// The keywords are the receivers that *look* like types: `Self::helper`
/// and `crate::models::EntityKind` name this repo, not a library.
fn reads_as_a_module(language: Language, owner: &str) -> bool {
    const RECEIVERS: &[&str] = &["self", "Self", "this", "cls", "super", "crate", "it", "me"];
    let separator = separator(language);
    let root = root_segment(language, owner);
    !RECEIVERS.contains(&root)
        && ((separator == "::" && owner.contains(separator))
            || root.chars().next().is_some_and(char::is_uppercase))
}

/// The first segment of an owner — the name a repo would have declared.
pub(super) fn root_segment(language: Language, owner: &str) -> &str {
    owner.split(separator(language)).next().unwrap_or(owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityKind, Span};

    fn ghost(qualified: &str, stdlib: bool) -> CodeEntity {
        let mut e = CodeEntity::new(
            qualified.rsplit("::").next().unwrap_or(qualified),
            EntityKind::Function,
            "",
            Span::default(),
        );
        e.qualified_name = qualified.to_string();
        e.tags.insert("ghost".to_string());
        if stdlib {
            e.tags.insert("ghost_stdlib".to_string());
        }
        e
    }

    /// A graph holding one declaration, so `declared` has something in it.
    fn graph_with(name: &str) -> DependencyGraph {
        DependencyGraph::from_analysis(&AnalysisResult {
            entities: vec![CodeEntity::new(
                name,
                EntityKind::Function,
                "a.rs",
                Span::default(),
            )],
            relationships: Vec::new(),
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn placed(graph: &DependencyGraph, language: Language, qualified: &str) -> String {
        let mut acc = Externals::new(graph, language);
        acc.add(&ghost(qualified, false));
        acc.sections("this entity").join("\n")
    }

    /// The distinction the ticket exists for: a library call is named as
    /// one, and a call the analyser could not bind is named as a hole.
    #[test]
    fn a_library_call_and_a_parser_miss_end_up_in_different_sections() {
        let graph = graph_with("render_row");
        let mut acc = Externals::new(&graph, Language::Rust);
        acc.add(&ghost("Node::walk", false));
        acc.add(&ghost("Node::walk", false));
        acc.add(&ghost("Node::kind", false));
        acc.add(&ghost("render_row", false));
        let out = acc.sections("this entity").join("\n");

        assert!(
            out.contains("- third-party `Node` — `walk` (2×), `kind` (1×)"),
            "the library calls were not named:\n{out}"
        );
        assert!(
            out.contains("## External (3 calls to 1 target)"),
            "the external calls were not counted:\n{out}"
        );
        assert!(
            out.contains("## Unresolved (1 call to 1 name)") && out.contains("`render_row` (1×)"),
            "the unbound call was not reported as one:\n{out}"
        );
        assert!(
            out.contains("floor, not a total"),
            "the unresolved section did not caveat the other counts:\n{out}"
        );
    }

    /// `serde_json::json` and `body::push` reach the graph as the same
    /// shape, so neither is claimed as a library — but the qualifier is
    /// kept, so a reader who recognises the crate is not made to guess
    /// which of the two an unadorned `json` was.
    #[test]
    fn an_unplaceable_qualifier_is_kept_rather_than_claimed_or_dropped() {
        let graph = graph_with("unrelated");
        let out = placed(&graph, Language::Rust, "serde_json::json");

        assert!(!out.contains("- third-party"), "a guess was printed:\n{out}");
        assert!(
            out.contains("`serde_json::json` (1×)"),
            "the qualifier was dropped:\n{out}"
        );
    }

    /// The parser's own stdlib table separates library noise from a
    /// third-party dependency — but only where nothing better placed the
    /// name: `Node::to_string` is a call on a third-party type, not a
    /// standard-library call, for all that `to_string` is Rust's.
    #[test]
    fn the_stdlib_tables_classify_but_do_not_outrank_an_owner() {
        let graph = graph_with("unrelated");
        assert!(placed(&graph, Language::Rust, "Vec::push").contains("- stdlib `Vec`"));
        assert!(placed(&graph, Language::Rust, "collect").contains("- stdlib — `collect` (1×)"));
        assert!(placed(&graph, Language::Rust, "std::fs::write").contains("- stdlib `std::fs`"));
        assert!(
            placed(&graph, Language::Rust, "Node::to_string").contains("- third-party `Node`"),
            "a stdlib member name outranked a third-party owner"
        );
    }

    /// `body.push(x)` reaches the graph as `body::push`. Reporting a local
    /// variable as a library is the failure CLI-001 removed; it stays
    /// removed.
    #[test]
    fn a_receiver_mezz_could_not_type_is_never_reported_as_a_library() {
        let graph = graph_with("unrelated");
        for target in ["body::push", "entity.id::clone", "node_text(src)::to_string"] {
            let out = placed(&graph, Language::Rust, target);
            assert!(
                !out.contains("- third-party"),
                "{target} was named as a library:\n{out}"
            );
        }
        // `clone` and `to_string` are Rust's own, so they are still placed
        // — just with nothing to hang them off.
        assert!(placed(&graph, Language::Rust, "entity.id::clone").contains("- stdlib — `clone`"));
        assert!(placed(&graph, Language::Rust, "body::push").contains("`body::push` (1×)"));
    }

    /// An owner the repo declares is a miss on code mezz holds, which is
    /// the opposite of a dependency on something installed.
    #[test]
    fn an_owner_this_repo_declares_is_a_hole_not_a_dependency() {
        let graph = graph_with("Walk");
        let out = placed(&graph, Language::Rust, "Walk::admit");

        assert!(!out.contains("third-party `Walk`"), "{out}");
        assert!(out.contains("## Unresolved (1 call to 1 name)"), "{out}");
    }

    /// A type reaches the graph unqualified when a signature named it. The
    /// declared-name check is what makes that placeable: `Node` is a
    /// library type here, and a hole in a repo that declares one.
    #[test]
    fn a_bare_type_is_placed_by_whether_this_repo_declares_it() {
        assert!(
            placed(&graph_with("unrelated"), Language::Rust, "Node")
                .contains("- third-party — `Node` (1×)"),
            "a library type was reported as a hole"
        );
        assert!(
            placed(&graph_with("Node"), Language::Rust, "Node")
                .contains("## Unresolved (1 call to 1 name)"),
            "a type this repo declares was reported as a library"
        );
    }

    /// Go writes every out-of-package call as `pkg.Name`, so the package
    /// table is the whole classification — and a `::` split would find
    /// nothing to split on.
    #[test]
    fn a_dot_language_splits_on_its_own_qualifier() {
        let graph = graph_with("unrelated");
        assert!(placed(&graph, Language::Go, "fmt.Println").contains("- stdlib `fmt`"));
        // A Go package outside the standard library is spelled exactly
        // like a local receiver, so it is listed, not claimed.
        assert!(placed(&graph, Language::Go, "chi.NewRouter").contains("`chi.NewRouter` (1×)"));
        assert!(placed(&graph, Language::Java, "Pattern.compile").contains("- stdlib `Pattern`"));
    }

    /// `options.config.get()` and `com.acme.Widget.of()` are the same
    /// shape where field access is spelled with a dot, so being qualified
    /// proves nothing there — only capitalisation does. In Rust it proves
    /// everything, because a field chain is not a `::` path at all.
    #[test]
    fn a_qualified_owner_is_proof_only_where_field_access_is_spelled_differently() {
        let graph = graph_with("unrelated");
        assert!(
            placed(&graph, Language::TypeScript, "options.config.get")
                .contains("`options.config.get` (1×)"),
            "a field chain was reported as a library"
        );
        assert!(
            placed(&graph, Language::Rust, "tree_sitter::Node::walk")
                .contains("- third-party `tree_sitter::Node`"),
            "a crate path was not reported as a library"
        );
    }

    /// Both sections are printed even when empty: "nothing left this repo"
    /// and "nothing was missed" are answers, and the second one is what
    /// makes the other counts trustworthy.
    #[test]
    fn a_subject_that_reaches_nothing_outside_says_so() {
        let graph = graph_with("a");
        let out = Externals::new(&graph, Language::Rust)
            .sections("this entity")
            .join("\n");

        assert!(out.contains("## External (0 calls)"), "{out}");
        assert!(out.contains("## Unresolved (0 calls)"), "{out}");
    }

    /// Long tails are capped like every other sub-list, and say so.
    #[test]
    fn both_sections_cap_and_report_what_they_dropped() {
        let graph = graph_with("unrelated");
        let mut acc = Externals::new(&graph, Language::Rust);
        for i in 0..MAX_TARGETS + 3 {
            acc.add(&ghost(&format!("Lib{i}::call"), false));
        }
        for i in 0..MAX_NAMES + 2 {
            acc.add(&ghost(&format!("miss_{i}"), false));
        }
        let out = acc.sections("this entity").join("\n");

        assert!(out.contains("… and 3 more targets. The cap is 12."), "{out}");
        assert!(out.contains("… and 2 more names"), "{out}");
    }
}
