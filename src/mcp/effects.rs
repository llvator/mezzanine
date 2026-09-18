//! What the code does to the world, and how far away it does it (MCP-043).
//!
//! `impact` answers "if I change this, what breaks" with the call graph and
//! the metrics over it. Neither says that the function writes to disk, that
//! its caller holds a lock while it does, or that the pure-looking helper
//! three hops down issues a network call — and those are where regressions
//! concentrate. The classes themselves, and what is deliberately excluded
//! from them, are [`crate::parser::effects`]; this module is the walk that
//! finds them and the section that prints them.
//!
//! ## Its own plus its callees'
//!
//! An entity's effect surface is the effects of the calls in its own body,
//! plus the effects of everything it can reach to `depth` hops. That is the
//! part a reader cannot get by looking: `write_report` looks pure, and the
//! `fs` under it belongs to a helper two files away. Each effect is
//! attributed to the entity whose body actually holds the call, so the row
//! is a place to go and read rather than a warning to take on trust.
//!
//! A class is listed once, under the shallowest call that introduces it. An
//! entity that writes files four ways has one `fs` row with four targets on
//! it, not four rows.
//!
//! ## Where it takes its input
//!
//! The same ghost targets [`super::externals`] places (MCP-039) — the calls
//! that left this repo. What it does *not* do is inherit that module's
//! third-party/unresolved split, because the two ask different questions of
//! the same name. That split is a test of *shape*: `serde_json::json` and
//! `body::push` look alike, so neither is claimed as a library. An effect
//! table is a test of *identity*: a rule matches `reqwest`, not "a lowercase
//! qualifier", and matching one is proof in a way that shape never is. So a
//! call can be named here while the section above could only list it as
//! unresolved, and that is the tables doing their job rather than the two
//! sections disagreeing.
//!
//! The one thing carried across unchanged is MCP-039's rule that an owner
//! this repo declares is a hole and not a dependency: a repo with its own
//! `Command` gets silence rather than a `proc` it never had.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::graph::DependencyGraph;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, RelationshipKind};
use crate::parser::effects::{self, Effect, COVERED};

use super::externals::{root_segment, split};
use super::tools::{body_scope_ids, is_listed, lifted, rel_path};

/// Targets named per class before the rest is counted. Four classes, so the
/// whole section stays inside a screen even at the cap.
const MAX_TARGETS: usize = 6;

/// The body one call was found in: what to attribute it to, how far out it
/// is, and whose tables classify it.
///
/// The language is the *holder's*, not the subject's. A Rust function whose
/// callee is TypeScript reaches `fs.readFile` through a body where `.` is
/// the qualifier and `fetch` means the network — classifying that with the
/// subject's table would split the name on the wrong separator and consult
/// the wrong list.
#[derive(Clone, Copy)]
struct Holder<'g> {
    via: Option<&'g CodeEntity>,
    depth: usize,
    language: Language,
}

/// One classified call, in the one body that holds it.
///
/// Keyed per body rather than per target, so the count beside a name is
/// what *that* body does. Summing two callers' writes and then naming one
/// of them reads as a claim about that caller, and is wrong about it: the
/// row would say `fs::create_dir_all (2×) via write` where `write` does it
/// once.
struct Site<'g> {
    /// The entity whose body holds the call — `None` when it is the
    /// subject's own body.
    via: Option<&'g CodeEntity>,
    /// Hops from the subject. `0` is the subject itself.
    depth: usize,
    /// Call sites in this body.
    calls: usize,
}

/// The effect classes reachable from one subject — an entity to some depth,
/// or a file at one hop.
pub(crate) struct EffectSurface<'g> {
    language: Language,
    /// Every name this repo declares, so an owner mezz holds is left
    /// unclassified rather than reported as a library effect (MCP-039).
    declared: HashSet<&'g str>,
    /// (class, target name, id of the body holding it) → the call. The
    /// empty id is the subject's own body.
    found: BTreeMap<(Effect, String, String), Site<'g>>,
}

impl<'g> EffectSurface<'g> {
    fn new(graph: &'g DependencyGraph, language: Language) -> Self {
        EffectSurface {
            language,
            declared: graph
                .entities()
                .filter(|e| is_listed(e))
                .map(|e| e.name.as_str())
                .collect(),
            found: BTreeMap::new(),
        }
    }

    /// The surface of one entity: its own calls, then its callees' to
    /// `max_depth` hops.
    ///
    /// Breadth-first, so the first body to reach a target is the nearest
    /// one, and `seen` is what keeps a cycle — the commonest shape in any
    /// call graph — from spinning.
    pub(crate) fn of_entity(
        graph: &'g DependencyGraph,
        target: &'g CodeEntity,
        max_depth: usize,
    ) -> Self {
        let mut surface = Self::new(graph, Language::from_path(&target.file_path));
        let mut seen: HashSet<&str> = HashSet::from([target.id.as_str()]);
        let mut frontier = vec![target];

        for depth in 0..=max_depth {
            let mut next = Vec::new();
            for entity in frontier {
                let holder = Holder {
                    // At depth 0 the call is in the subject's own body,
                    // which is `here` rather than `via` anything.
                    via: (depth > 0).then_some(entity),
                    depth,
                    language: Language::from_path(&entity.file_path),
                };
                let onward = surface.harvest(graph, entity, holder);
                next.extend(
                    onward
                        .into_iter()
                        .filter(|c| depth < max_depth && seen.insert(&c.id)),
                );
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        surface
    }

    /// The surface of a file: every entity it holds, one hop, no walk.
    ///
    /// The file view of `impact` is one hop in each direction by
    /// construction, and an effect surface that reached further would be
    /// answering a question the rest of that report is not.
    pub(crate) fn of_file(
        graph: &'g DependencyGraph,
        file: &Path,
        local: &HashSet<&'g str>,
    ) -> Self {
        let mut surface = Self::new(graph, Language::from_path(file));
        let mut ids: Vec<&str> = local.iter().copied().collect();
        ids.sort_unstable();
        // Read one node at a time rather than through [`Self::harvest`]:
        // `local` already holds every node in the file, branch and loop
        // scopes included, so expanding each declaration into its own body
        // scopes here would count every guarded call twice.
        let holder = Holder {
            via: None,
            depth: 0,
            language: surface.language,
        };
        for id in ids {
            surface.reach(graph, id, holder);
        }
        surface
    }

    /// Classify every unbound target `entity` reaches, and hand back the
    /// repo code it reaches, for the next hop.
    ///
    /// Read from the entity *and* the branch and loop nodes its body is cut
    /// into: a call written inside an `if` leaves from the branch and never
    /// touches the callable's own id, so a walk that skipped them would
    /// report a function full of guarded file writes as effect-free.
    fn harvest(
        &mut self,
        graph: &'g DependencyGraph,
        entity: &CodeEntity,
        holder: Holder<'g>,
    ) -> Vec<&'g CodeEntity> {
        let mut onward = Vec::new();
        for id in body_scope_ids(graph, entity) {
            onward.extend(self.reach(graph, &id, holder));
        }
        onward
    }

    /// One node's outgoing edges: unbound targets classified, repo code
    /// handed back lifted to the callable a reader can open.
    fn reach(
        &mut self,
        graph: &'g DependencyGraph,
        id: &str,
        holder: Holder<'g>,
    ) -> Vec<&'g CodeEntity> {
        let mut onward = Vec::new();
        for (dep, rel) in graph.dependencies(id) {
            if rel.kind == RelationshipKind::Contains {
                continue;
            }
            match dep.tags.contains("ghost") {
                true => self.add(dep, holder),
                false => onward.extend(lifted(graph, dep).filter(|e| is_listed(e))),
            }
        }
        onward
    }

    /// Record one unbound target, if these tables recognise what it does.
    ///
    /// One entry per (class, target, body): two callers that both write
    /// files are two places to go and read, and the row for each says what
    /// that one does.
    fn add(&mut self, ghost: &CodeEntity, holder: Holder<'g>) {
        let qualified = match ghost.qualified_name.is_empty() {
            true => ghost.name.as_str(),
            false => ghost.qualified_name.as_str(),
        };
        let (owner, member) = split(holder.language, qualified);
        if self.declares(holder.language, owner.unwrap_or(member)) {
            return;
        }
        let Some(effect) = effects::classify(holder.language, owner, member) else {
            return;
        };
        let body = holder.via.map(|e| e.id.clone()).unwrap_or_default();
        self.found
            .entry((effect, qualified.to_string(), body))
            .or_insert(Site {
                via: holder.via,
                depth: holder.depth,
                calls: 0,
            })
            .calls += 1;
    }

    /// Whether this repo declares the name an effect would be claimed on.
    ///
    /// MCP-039's rule, carried across: mezz holds that type and still failed
    /// to bind the call, so it is a hole in the graph and not a reach into a
    /// library. A repo with its own `Command` should get silence, not a
    /// `proc` it never had.
    fn declares(&self, language: Language, name: &str) -> bool {
        self.declared.contains(root_segment(language, name))
    }

    /// The classes found, in class order — the one-line answer to "what does
    /// this touch".
    fn classes(&self) -> Vec<Effect> {
        let mut classes: Vec<Effect> = self.found.keys().map(|(effect, ..)| *effect).collect();
        classes.dedup();
        classes
    }

    /// The section, ready to append to a report body.
    ///
    /// `reach` is how far the walk went, or `None` for a subject that does
    /// not walk. `subject` names what the counts are about — "this entity",
    /// "this file".
    pub(crate) fn section(&self, subject: &str, reach: Option<usize>, root: &Path) -> Vec<String> {
        let mut body = vec![String::new()];
        if !effects::has_table(self.language) {
            body.extend(self.silence());
            return body;
        }
        let classes = self.classes();
        body.push(format!(
            "## Effects ({}) — what {} does to the world{}",
            match classes.is_empty() {
                true => "none found".to_string(),
                false => classes
                    .iter()
                    .map(|c| c.tag())
                    .collect::<Vec<_>>()
                    .join(", "),
            },
            subject,
            reach.map_or(String::new(), |d| match d {
                1 => ", itself and 1 hop out".to_string(),
                d => format!(", itself and {d} hops out"),
            }),
        ));
        body.push(legend());
        match classes.is_empty() {
            true => body.push(
                "No call reached from here matched mezz's effect table for this language. \
                 That is \"none recognised\", not \"pure\": the table is a list of names, \
                 and the *Unresolved* targets above are not classified at all."
                    .to_string(),
            ),
            false => body.extend(classes.iter().map(|c| self.row(*c, root))),
        }
        body
    }

    /// The sentence a language with no table prints instead of a count.
    ///
    /// It says which language, and which languages *are* covered, because
    /// the failure this guards against is a reader taking an empty section
    /// as a finding of purity — the `dead_code` precedent that what was
    /// filtered gets stated (MCP-043).
    fn silence(&self) -> Vec<String> {
        vec![
            format!(
                "## Effects — not classified for {}",
                self.language.display_name()
            ),
            format!(
                "mezz carries effect tables for {COVERED}. For any other language this \
                 section is silence, not a finding that the code has no effects."
            ),
        ]
    }

    /// One class, and the calls that introduce it — nearest first.
    fn row(&self, effect: Effect, root: &Path) -> String {
        let mut sites: Vec<(&String, &Site)> = self
            .found
            .iter()
            .filter(|((class, ..), _)| *class == effect)
            .map(|((_, target, _), site)| (target, site))
            .collect();
        sites.sort_by_key(|(target, site)| {
            (site.depth, std::cmp::Reverse(site.calls), target.as_str())
        });
        let named: Vec<String> = sites
            .iter()
            .take(MAX_TARGETS)
            .map(|(target, site)| call(target, site, root))
            .collect();
        let rest = match sites.len().saturating_sub(MAX_TARGETS) {
            0 => String::new(),
            n => format!("; … and {n} more"),
        };
        format!("- {} — {}{}", effect.tag(), named.join("; "), rest)
    }

}

/// `` `std::fs::write` (2×) here ``, or the same with the body to go and open.
///
/// The `file:line` is the whole point of the attribution: a class without
/// somewhere to read it is a warning, and a warning about an effect the
/// reader cannot see is one they have to take on trust.
fn call(target: &str, site: &Site, root: &Path) -> String {
    let where_ = match site.via {
        None => " here".to_string(),
        Some(via) => format!(
            " via {} `{}` — {}:{}",
            via.kind.display_name(),
            via.name,
            rel_path(&via.file_path, root),
            via.span.start.line + 1,
        ),
    };
    format!("`{target}` ({}×){where_}", site.calls)
}

/// The four classes spelled out, on every printed section.
///
/// Repeated rather than assumed: a `proc` tag is unreadable to an agent
/// meeting it for the first time, and the caveat that this is a name
/// classification rather than dataflow belongs beside the answer, not in
/// a document the reader does not have open.
fn legend() -> String {
    let classes: Vec<String> = Effect::all()
        .iter()
        .map(|c| format!("`{}` {}", c.tag(), c.meaning()))
        .collect();
    format!(
        "Classified from call target names, not from dataflow — {}. Each class is listed \
         once, under the nearest call that introduces it.",
        classes.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::AnalysisResult;
    use crate::models::{EntityKind, Relationship, Span};

    fn entity(name: &str, file: &str) -> CodeEntity {
        CodeEntity::new(name, EntityKind::Function, file, Span::default())
    }

    fn ghost(qualified: &str) -> CodeEntity {
        let mut e = CodeEntity::new(
            qualified.rsplit("::").next().unwrap_or(qualified),
            EntityKind::Function,
            "",
            Span::default(),
        );
        e.qualified_name = qualified.to_string();
        e.tags.insert("ghost".to_string());
        e
    }

    fn calls(from: &CodeEntity, to: &CodeEntity) -> Relationship {
        Relationship::new(&from.id, &to.id, RelationshipKind::Calls)
    }

    /// A graph of `entities` wired by `edges`, given as index pairs.
    fn graph_of(entities: Vec<CodeEntity>, edges: &[(usize, usize)]) -> DependencyGraph {
        let relationships = edges
            .iter()
            .map(|(from, to)| calls(&entities[*from], &entities[*to]))
            .collect();
        DependencyGraph::from_analysis(&AnalysisResult {
            entities,
            relationships,
            files: Vec::new(),
            import_sites: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn surface_of(graph: &DependencyGraph, name: &str, depth: usize) -> String {
        let target = graph
            .entities()
            .find(|e| e.name == name)
            .expect("the fixture declares it");
        EffectSurface::of_entity(graph, target, depth)
            .section("this entity", Some(depth), Path::new(""))
            .join("\n")
    }

    /// The direct case: a call in the entity's own body is named, with its
    /// class and its count, and marked as being right here.
    #[test]
    fn an_entitys_own_calls_are_classified_and_named() {
        let graph = graph_of(
            vec![entity("save", "a.rs"), ghost("std::fs::write")],
            &[(0, 1), (0, 1)],
        );
        let out = surface_of(&graph, "save", 2);

        assert!(out.contains("## Effects (fs)"), "{out}");
        assert!(out.contains("`std::fs::write` (2×) here"), "{out}");
    }

    /// The case the ticket exists for: the pure-looking caller, and the
    /// effect two hops down. The body that holds the call is named, so the
    /// row is somewhere to go rather than a claim to trust.
    #[test]
    fn an_effect_two_hops_down_is_found_and_attributed_to_the_body_holding_it() {
        let graph = graph_of(
            vec![
                entity("handle", "a.rs"),
                entity("render", "b.rs"),
                entity("flush", "c.rs"),
                ghost("std::fs::write"),
            ],
            &[(0, 1), (1, 2), (2, 3)],
        );

        let out = surface_of(&graph, "handle", 2);
        assert!(
            out.contains("`std::fs::write` (1×) via function `flush` — c.rs:1"),
            "the introducing body was not named:\n{out}"
        );
        // …and the depth is honoured rather than ignored: one hop does not
        // reach it, and says so without claiming purity.
        let shallow = surface_of(&graph, "handle", 1);
        assert!(shallow.contains("## Effects (none found)"), "{shallow}");
        assert!(shallow.contains("not \"pure\""), "{shallow}");
    }

    /// One class, however many calls introduce it. Four ways of writing a
    /// file is one thing to know, not four rows.
    #[test]
    fn a_class_is_one_row_with_every_call_that_introduces_it() {
        let graph = graph_of(
            vec![
                entity("run", "a.rs"),
                ghost("std::fs::write"),
                ghost("File::create"),
                ghost("std::env::var"),
            ],
            &[(0, 1), (0, 2), (0, 3)],
        );
        let out = surface_of(&graph, "run", 2);

        assert!(out.contains("## Effects (fs, env)"), "{out}");
        assert_eq!(
            out.lines().filter(|l| l.starts_with("- fs —")).count(),
            1,
            "one class, one row:\n{out}"
        );
        assert!(out.contains("`File::create`") && out.contains("`std::fs::write`"), "{out}");
    }

    /// The acceptance criterion that silence must not read as purity: a
    /// language with no table says which language and which are covered.
    #[test]
    fn a_language_without_a_table_says_so_rather_than_reporting_none() {
        let graph = graph_of(
            vec![entity("save", "a.rb"), ghost("File::read")],
            &[(0, 1)],
        );
        let out = surface_of(&graph, "save", 2);

        assert!(out.contains("## Effects — not classified for"), "{out}");
        assert!(out.contains("Ruby"), "{out}");
        assert!(out.contains("silence, not a finding"), "{out}");
        assert!(!out.contains("none found"), "a silence was printed as a zero:\n{out}");
    }

    /// MCP-039's rule, carried across: mezz holds this type and still failed
    /// to bind the call, so it is a hole in the graph and not a `proc` the
    /// code has.
    #[test]
    fn an_owner_this_repo_declares_is_not_claimed_as_an_effect() {
        let graph = graph_of(
            vec![
                entity("run", "a.rs"),
                entity("Command", "a.rs"),
                ghost("Command::new"),
            ],
            &[(0, 2)],
        );
        let out = surface_of(&graph, "run", 2);

        assert!(out.contains("## Effects (none found)"), "{out}");
    }

    /// Two bodies that both write files are two places to go and read, and
    /// each row counts what that one body does. Summing them and naming one
    /// would report `write_a` as doing twice what it does.
    #[test]
    fn two_bodies_reaching_one_target_are_counted_separately() {
        let graph = graph_of(
            vec![
                entity("run", "a.rs"),
                entity("write_a", "b.rs"),
                entity("write_b", "c.rs"),
                ghost("std::fs::write"),
            ],
            &[(0, 1), (0, 2), (1, 3), (2, 3), (2, 3)],
        );
        let out = surface_of(&graph, "run", 2);

        assert!(
            out.contains("`std::fs::write` (1×) via function `write_a` — b.rs:1"),
            "{out}"
        );
        assert!(
            out.contains("`std::fs::write` (2×) via function `write_b` — c.rs:1"),
            "{out}"
        );
        // Still one row: the class is what a reader scans for.
        assert_eq!(out.lines().filter(|l| l.starts_with("- fs —")).count(), 1, "{out}");
    }

    /// A cycle is a shape the walk meets constantly and must not spin on.
    #[test]
    fn a_cycle_in_the_call_graph_terminates() {
        let graph = graph_of(
            vec![
                entity("a", "a.rs"),
                entity("b", "a.rs"),
                ghost("std::env::var"),
            ],
            &[(0, 1), (1, 0), (1, 2)],
        );
        let out = surface_of(&graph, "a", 5);

        assert!(out.contains("## Effects (env)"), "{out}");
    }
}
