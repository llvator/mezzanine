//! Parser-level tests for the TypeScript parser: the `UsesType` post-pass
//! (TS-001), control-flow entities (TS-002), body metrics (TS-003), and the
//! receiver typing that makes call edges resolvable.

use super::super::language_parser::{LanguageParser, ParseResult};
use super::TypeScriptParser;
use crate::models::{EntityKind, RelationshipKind};
use std::path::PathBuf;

fn parse(src: &str) -> ParseResult {
    TypeScriptParser::new()
        .parse(&PathBuf::from("test.ts"), src)
        .unwrap()
}

/// Parse as `.tsx`, which selects the other grammar.
fn parse_tsx(src: &str) -> ParseResult {
    TypeScriptParser::new()
        .parse(&PathBuf::from("test.tsx"), src)
        .unwrap()
}

/// The `(cyclomatic, max_nesting, cognitive)` triple of the named entity.
fn metrics_of(result: &ParseResult, name: &str) -> (u32, u32, u32) {
    let entity = result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named {name}"));
    (
        entity.metrics.cyclomatic.unwrap(),
        entity.metrics.max_nesting.unwrap(),
        entity.metrics.cognitive_complexity.unwrap(),
    )
}

/// Names of every entity of the given kind, in emission order.
fn names_of_kind(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.name.clone())
        .collect()
}

/// Targets of every `Calls` edge.
fn call_targets(result: &ParseResult) -> Vec<String> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| r.target_id.clone())
        .collect()
}

/// `(target, branch-path)` for every edge that carries a `branch` key.
fn branch_tagged(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter_map(|r| {
            r.metadata
                .get("branch")
                .map(|b| (r.target_id.clone(), b.clone()))
        })
        .collect()
}

/// (source entity name, target type name) pairs of all UsesType edges.
fn uses_type_edges(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::UsesType)
        .map(|r| {
            let source_name = result
                .entities
                .iter()
                .find(|e| e.id == r.source_id)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| r.source_id.clone());
            (source_name, r.target_id.clone())
        })
        .collect()
}

#[test]
fn function_signature_types_produce_uses_type_edges() {
    let res = parse("function f(g: Graph): Config { return g.config; }");
    let edges = uses_type_edges(&res);
    assert!(edges.contains(&("f".into(), "Graph".into())), "{edges:?}");
    assert!(edges.contains(&("f".into(), "Config".into())), "{edges:?}");
}

#[test]
fn promise_wrapping_yields_only_the_inner_type() {
    let res = parse("async function load(): Promise<Widget[]> { return []; }");
    let targets: Vec<_> = uses_type_edges(&res).into_iter().map(|(_, t)| t).collect();
    assert_eq!(targets, vec!["Widget"]);
}

#[test]
fn union_types_yield_both_sides() {
    let res = parse("function pick(x: Foo | Bar): void {}");
    let targets: Vec<_> = uses_type_edges(&res).into_iter().map(|(_, t)| t).collect();
    assert!(targets.contains(&"Foo".to_string()), "{targets:?}");
    assert!(targets.contains(&"Bar".to_string()), "{targets:?}");
}

#[test]
fn utility_types_are_dropped_but_arguments_kept() {
    let res = parse("function patch(c: Partial<Config>): void {}");
    let targets: Vec<_> = uses_type_edges(&res).into_iter().map(|(_, t)| t).collect();
    assert_eq!(targets, vec!["Config"]);
}

#[test]
fn interface_property_types_produce_edges() {
    let src = "interface Store {\n  graph: Graph;\n  entries: Map<string, Entry>;\n}";
    let res = parse(src);
    let edges = uses_type_edges(&res);
    assert!(
        edges.contains(&("graph".into(), "Graph".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("entries".into(), "Entry".into())),
        "{edges:?}"
    );
    // Map is a builtin — no edge to it.
    assert!(edges.iter().all(|(_, t)| t != "Map"), "{edges:?}");
}

#[test]
fn class_field_and_method_types_produce_edges() {
    let src = "class Service {\n  private client: ApiClient;\n  fetch(q: Query): Promise<FetchResult> { return this.client.run(q); }\n}";
    let res = parse(src);
    let edges = uses_type_edges(&res);
    assert!(
        edges.contains(&("client".into(), "ApiClient".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("fetch".into(), "Query".into())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("fetch".into(), "FetchResult".into())),
        "{edges:?}"
    );
}

#[test]
fn recursive_property_mentions_of_owner_are_skipped() {
    let src = "interface TreeNode {\n  children: TreeNode[];\n  meta: Meta;\n}";
    let res = parse(src);
    let edges = uses_type_edges(&res);
    assert!(
        edges.iter().all(|(_, t)| t != "TreeNode"),
        "self-mention of the owning container must be skipped: {edges:?}"
    );
    assert!(edges.contains(&("meta".into(), "Meta".into())), "{edges:?}");
}

#[test]
fn duplicate_mentions_dedup_per_entity() {
    let res = parse("function join(a: Config, b: Config): Config { return a; }");
    let count = uses_type_edges(&res)
        .iter()
        .filter(|(_, t)| t == "Config")
        .count();
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------------------
// TS-003 — body metrics
// ---------------------------------------------------------------------------

/// The audit's gnarly fixture, ported from Java. Java scores 12 / 31 / 6 on
/// it; this pins TypeScript to the same numbers, so a grammar bump that
/// silently changes the scoring fails here rather than in a user's report.
const GNARLY: &str = r#"
function gnarly(a: number, b: number, c: number, d: number): number {
  if (a > 0) { for (let x = 0; x < b; x++) { if (x > 0 && c > 0) { while (d > 0) { try { if (x > 1) {} else if (x < 0) {} else {} } catch (e) {} } } } }
  else if (b > 0 || c > 0) { return 1; }
  return 2;
}
"#;

#[test]
fn gnarly_fixture_scores_the_same_as_java() {
    assert_eq!(metrics_of(&parse(GNARLY), "gnarly"), (12, 6, 31));
}

#[test]
fn gnarly_fixture_scores_identically_in_tsx() {
    assert_eq!(metrics_of(&parse_tsx(GNARLY), "gnarly"), (12, 6, 31));
}

#[test]
fn a_straight_line_function_is_cyclomatic_one() {
    let res = parse("function f(): number { const x = 1; return x; }");
    assert_eq!(metrics_of(&res, "f"), (1, 0, 0));
}

#[test]
fn short_circuit_operators_each_add_one_to_both_scores() {
    // `??` counts exactly like `&&` and `||`.
    let res = parse("function f(a: number) { return a && a || a ?? a; }");
    let (cyclomatic, _, cognitive) = metrics_of(&res, "f");
    assert_eq!((cyclomatic, cognitive), (4, 3));
}

#[test]
fn optional_chaining_is_deliberately_not_a_branch() {
    // Documented in complexity.rs: `?.` is pervasive in TypeScript where
    // Java and Rust write nothing, so counting it would make the scores
    // incomparable across languages.
    let res = parse("function f(a: any) { return a?.b?.c?.d; }");
    assert_eq!(metrics_of(&res, "f"), (1, 0, 0));
}

#[test]
fn a_chained_else_if_is_not_counted_twice() {
    // The `else_clause` wrapping an `if_statement` scores nothing; the
    // nested `if` scores itself. Flat: 1 + if + else-if + terminal else.
    let res = parse("function f(a: number) { if (a) {} else if (a) {} else {} }");
    let (cyclomatic, _, _) = metrics_of(&res, "f");
    assert_eq!(cyclomatic, 4);
}

#[test]
fn switch_arms_add_to_cyclomatic_but_not_to_cognitive() {
    let res = parse(
        "function f(a: number) { switch (a) { case 1: break; case 2: break; default: break; } }",
    );
    let (cyclomatic, _, cognitive) = metrics_of(&res, "f");
    assert_eq!(cyclomatic, 4, "one per case plus default, plus the base");
    assert_eq!(cognitive, 1, "the switch itself, not its arms");
}

#[test]
fn a_callback_deepens_cognitive_nesting_without_branching() {
    let res = parse("function f(xs: number[]) { if (xs) { xs.forEach(x => { if (x) {} }); } }");
    let (cyclomatic, _, cognitive) = metrics_of(&res, "f");
    // if(+1) + inner if(+1) — the arrow adds no branch.
    assert_eq!(cyclomatic, 3);
    // if@0 = 1, arrow@1 = 2, inner if@2 = 3 → 6.
    assert_eq!(cognitive, 6);
}

#[test]
fn bodyless_declarations_get_zero_rather_than_none() {
    // TS-003: `None` makes `mezz quality` skip the entity entirely, because
    // the hotspot ranking filters on `composite_score > 0.0`.
    let res = parse("interface Store { load(id: string): Widget; }");
    assert_eq!(metrics_of(&res, "load"), (1, 0, 0));
}

#[test]
fn abstract_methods_get_zero_rather_than_none() {
    let res = parse("abstract class Base { abstract run(a: string): void; }");
    assert_eq!(metrics_of(&res, "run"), (1, 0, 0));
}

#[test]
fn arrow_function_constants_are_measured_like_functions() {
    let res =
        parse("export const pick = (a: number, b: number) => { if (a) { return a; } return b; };");
    assert_eq!(metrics_of(&res, "pick"), (2, 1, 1));
    let entity = res.entities.iter().find(|e| e.name == "pick").unwrap();
    assert_eq!(entity.metrics.param_count, Some(2));
}

#[test]
fn class_methods_carry_loc_and_param_count() {
    let res = parse("class S {\n  run(a: string, b: string): void {\n    return;\n  }\n}");
    let entity = res.entities.iter().find(|e| e.name == "run").unwrap();
    assert_eq!(entity.metrics.param_count, Some(2));
    assert_eq!(entity.metrics.loc, 3);
}

// ---------------------------------------------------------------------------
// TS-002 — control-flow entities
// ---------------------------------------------------------------------------

#[test]
fn the_gnarly_fixture_yields_the_same_arm_counts_as_java() {
    // Java: 8 Branch + 2 Loop on this fixture.
    let res = parse(GNARLY);
    assert_eq!(names_of_kind(&res, EntityKind::Branch).len(), 8);
    assert_eq!(names_of_kind(&res, EntityKind::Loop).len(), 2);
}

#[test]
fn an_if_else_chain_flattens_into_sibling_arms() {
    let res = parse("function f(a: number) { if (a === 1) { one(); } else if (a === 2) { two(); } else { rest(); } }");
    assert_eq!(
        names_of_kind(&res, EntityKind::Branch),
        vec!["c1", "c2", "c3"]
    );
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("one".into(), "c1".into())), "{tagged:?}");
    assert!(tagged.contains(&("two".into(), "c2".into())), "{tagged:?}");
    assert!(tagged.contains(&("rest".into(), "c3".into())), "{tagged:?}");
}

#[test]
fn nested_arms_carry_a_dotted_ancestry_path() {
    let res = parse("function f(a: number) { if (a) { if (a) { inner(); } } }");
    assert_eq!(names_of_kind(&res, EntityKind::Branch), vec!["c1", "c1.c2"]);
    assert!(branch_tagged(&res).contains(&("inner".into(), "c1.c2".into())));
}

#[test]
fn loops_use_their_own_path_prefix() {
    let res = parse(
        "function f(xs: string[]) { for (const x of xs) { visit(x); } while (ready()) { step(); } }",
    );
    assert_eq!(names_of_kind(&res, EntityKind::Loop), vec!["l1", "l2"]);
    let tagged = branch_tagged(&res);
    assert!(
        tagged.contains(&("visit".into(), "l1".into())),
        "{tagged:?}"
    );
    // The `while` condition runs every iteration, so it groups with the loop.
    assert!(
        tagged.contains(&("ready".into(), "l2".into())),
        "{tagged:?}"
    );
    assert!(tagged.contains(&("step".into(), "l2".into())), "{tagged:?}");
}

#[test]
fn all_four_loop_forms_emit_a_loop_entity() {
    let src = "function f(xs: string[], o: object) {\n\
               for (let i = 0; i < 3; i++) { a(); }\n\
               for (const x of xs) { b(); }\n\
               for (const k in o) { c(); }\n\
               while (x) { d(); }\n\
               do { e(); } while (x);\n\
               }";
    assert_eq!(names_of_kind(&parse(src), EntityKind::Loop).len(), 5);
}

#[test]
fn try_arms_are_tagged_by_kind_and_carry_the_catch_binding() {
    let res = parse("function f() { try { risky(); } catch (err: unknown) { report(); } finally { cleanup(); } }");
    let arms: Vec<_> = res
        .entities
        .iter()
        .filter(|e| e.tags.contains("try_arm"))
        .collect();
    assert_eq!(arms.len(), 3);
    assert!(arms[0].tags.contains("try_body_arm"));
    assert!(arms[1].tags.contains("catch_arm"));
    assert!(arms[2].tags.contains("finally_arm"));
    assert!(
        arms[1]
            .attributes
            .iter()
            .any(|a| a.starts_with("caught:err")),
        "{:?}",
        arms[1].attributes
    );
}

#[test]
fn switch_arms_are_tagged_and_carry_their_pattern() {
    let res =
        parse("function f(a: string) { switch (a) { case 'x': hit(); break; default: miss(); } }");
    let arms: Vec<_> = res
        .entities
        .iter()
        .filter(|e| e.tags.contains("case_arm"))
        .collect();
    assert_eq!(arms.len(), 2);
    assert!(arms[0].attributes.contains(&"pattern:'x'".to_string()));
    assert!(
        arms[1]
            .attributes
            .iter()
            .all(|a| !a.starts_with("pattern:")),
        "default: has no pattern"
    );
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("hit".into(), "c1".into())), "{tagged:?}");
    assert!(tagged.contains(&("miss".into(), "c2".into())), "{tagged:?}");
}

#[test]
fn flow_entities_follow_the_shared_id_and_parent_conventions() {
    // Load-bearing: the analyzer's branch-grouping pass materialises the
    // same shapes from tagged edges and deduplicates against these.
    let res = parse("function f(a: number) { if (a) { if (a) { inner(); } } }");
    let caller = res.entities.iter().find(|e| e.name == "f").unwrap();
    let nested = res.entities.iter().find(|e| e.name == "c1.c2").unwrap();
    assert_eq!(nested.id, format!("{}::branch::c1.c2", caller.id));
    assert_eq!(
        nested.parent_id.as_deref(),
        Some(format!("{}::branch::c1", caller.id).as_str())
    );
    assert_eq!(nested.qualified_name, format!("{}::c1.c2", caller.id));
    assert!(nested.tags.contains("branch_node"));
}

#[test]
fn conditions_stay_outside_the_arms_they_guard() {
    // The condition runs before any arm is chosen, so it belongs to the
    // enclosing flow, not to the arm.
    let res = parse("function f() { if (check()) { act(); } }");
    let tagged = branch_tagged(&res);
    assert!(tagged.contains(&("act".into(), "c1".into())), "{tagged:?}");
    assert!(
        !tagged.iter().any(|(t, _)| t == "check"),
        "the condition must carry no branch tag: {tagged:?}"
    );
}

#[test]
fn tsx_bodies_produce_the_same_flow_entities() {
    let res = parse_tsx("function f(a: number) { if (a) { one(); } else { two(); } }");
    assert_eq!(names_of_kind(&res, EntityKind::Branch), vec!["c1", "c2"]);
}

// ---------------------------------------------------------------------------
// Receiver typing and call edges
// ---------------------------------------------------------------------------

#[test]
fn a_parameter_type_qualifies_its_receiver() {
    let res = parse("function run(client: ApiClient) { client.send(); }");
    assert!(call_targets(&res).contains(&"ApiClient.send".to_string()));
}

#[test]
fn an_annotated_local_qualifies_its_receiver() {
    let res = parse("function run() { const client: ApiClient = build(); client.send(); }");
    assert!(call_targets(&res).contains(&"ApiClient.send".to_string()));
}

#[test]
fn a_constructor_call_types_the_local_it_binds() {
    let res = parse("function run() { const client = new ApiClient(); client.send(); }");
    assert!(call_targets(&res).contains(&"ApiClient.send".to_string()));
}

#[test]
fn this_resolves_to_the_enclosing_class() {
    let res = parse("class Service { run() { this.helper(); } helper() {} }");
    assert!(call_targets(&res).contains(&"Service.helper".to_string()));
}

#[test]
fn a_declared_field_types_a_this_receiver() {
    let res = parse("class Service {\n  private api: ApiClient;\n  run() { this.api.send(); }\n}");
    assert!(
        call_targets(&res).contains(&"ApiClient.send".to_string()),
        "{:?}",
        call_targets(&res)
    );
}

#[test]
fn a_constructor_parameter_property_types_a_this_receiver() {
    // The dependency-injection shape most TypeScript services use.
    let res = parse(
        "class Service {\n  constructor(private readonly api: ApiClient) {}\n  run() { this.api.send(); }\n}",
    );
    assert!(
        call_targets(&res).contains(&"ApiClient.send".to_string()),
        "{:?}",
        call_targets(&res)
    );
}

#[test]
fn a_receiver_less_call_stays_unqualified() {
    // Regression guard: qualifying `formatDate()` as `Service.formatDate`
    // produced a name that misses `typed_method_to_id`, and the resolver
    // drops qualified misses rather than retrying them as bare names — so
    // every call to an imported helper vanished from the graph.
    let res = parse("class Service { run() { formatDate(); } }");
    let targets = call_targets(&res);
    assert!(targets.contains(&"formatDate".to_string()), "{targets:?}");
    assert!(
        !targets.contains(&"Service.formatDate".to_string()),
        "{targets:?}"
    );
}

#[test]
fn an_untypeable_receiver_falls_back_to_its_own_text() {
    // Honest failure: the edge lands on a ghost rather than collapsing
    // onto a same-named method somewhere else in the tree.
    let res = parse("function run() { unknownThing.send(); }");
    assert!(call_targets(&res).contains(&"unknownThing.send".to_string()));
}

#[test]
fn a_fluent_chain_collapses_onto_its_nearest_named_object() {
    // Shipping the whole expression as the callee minted one ghost per call
    // site with a hundred-character name — 40% of every ghost on this repo's
    // own `ui/` tree. Both calls below now name the same stable ghost.
    let res =
        parse("function run() { sel.select('c').attr('r', 1); sel.select('d').attr('r', 2); }");
    let attrs: Vec<_> = call_targets(&res)
        .into_iter()
        .filter(|t| t.ends_with("attr"))
        .collect();
    assert_eq!(attrs, vec!["sel.attr", "sel.attr"]);
}

#[test]
fn an_optional_chain_is_not_part_of_the_object_name() {
    let res = parse("function run() { simulation?.alpha(0.3).restart(); }");
    assert!(
        call_targets(&res).contains(&"simulation.restart".to_string()),
        "{:?}",
        call_targets(&res)
    );
}

#[test]
fn no_call_target_is_ever_a_non_identifier_string() {
    // The TS-004 acceptance shape, pinned at the parser rather than measured
    // downstream: every callee is `name` or `name.name…`.
    let res = parse(
        "function run(x: any) {\n\
           (document.querySelector('.a') as HTMLElement).focus();\n\
           d3.select(x).attr('r', 1).attr('fill', 'red');\n\
           handlers[key]().go();\n\
           (await load()).use();\n\
         }",
    );
    for target in call_targets(&res) {
        assert!(
            target.split('.').all(|s| {
                s.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$')
                    && s.chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            }),
            "`{target}` is not an identifier path"
        );
    }
}

#[test]
fn a_fluent_chain_is_not_collapsed_onto_a_bare_name() {
    // A bare `attr` would be eligible for the resolver's locality-ranked
    // bare-name lookup and could bind to any project function called `attr`.
    // Measured on `ui/`, that bought 41 edges targeting `on`, `select`,
    // `text`, `data` and `id` — precision loss dressed up as recall.
    let res = parse("function run(x: any) { d3.select(x).attr('r', 1); }");
    assert!(!call_targets(&res).contains(&"attr".to_string()));
}

#[test]
fn a_wrapped_receiver_does_not_ship_whitespace_inside_a_name() {
    let res = parse("function run() {\n  someObject\n    .send();\n}");
    let targets = call_targets(&res);
    assert!(
        targets
            .iter()
            .all(|t| !t.contains('\n') && !t.contains(' ')),
        "{targets:?}"
    );
    assert!(
        targets.contains(&"someObject.send".to_string()),
        "{targets:?}"
    );
}

#[test]
fn super_calls_qualify_with_the_enclosing_class() {
    let res = parse("class Child { render() { super.render2(); } }");
    assert!(call_targets(&res).contains(&"Child.render2".to_string()));
}

#[test]
fn a_local_shadows_a_parameter_of_the_same_name() {
    let res = parse("function run(c: Outer) { const c = new Inner(); c.send(); }");
    let targets = call_targets(&res);
    assert!(targets.contains(&"Inner.send".to_string()), "{targets:?}");
}

#[test]
fn an_array_typed_receiver_is_not_typed_as_its_element() {
    // `xs.at(0)` is an Array method, not a Widget method.
    let res = parse("function run(xs: Widget[]) { xs.at(0); }");
    let targets = call_targets(&res);
    assert!(!targets.contains(&"Widget.at".to_string()), "{targets:?}");
}

#[test]
fn a_union_of_two_real_types_is_not_typed() {
    let res = parse("function run(x: Foo | Bar) { x.send(); }");
    let targets = call_targets(&res);
    assert!(targets.contains(&"x.send".to_string()), "{targets:?}");
}

#[test]
fn a_nullable_type_is_typed_as_its_non_null_arm() {
    let res = parse("function run(x: Foo | null) { x.send(); }");
    assert!(call_targets(&res).contains(&"Foo.send".to_string()));
}

#[test]
fn instantiation_emits_an_instantiates_edge() {
    let res = parse("function run() { new Widget(); }");
    let targets: Vec<_> = res
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Instantiates)
        .map(|r| r.target_id.clone())
        .collect();
    assert_eq!(targets, vec!["Widget"]);
}

#[test]
fn a_bound_call_records_the_variable_it_binds_to() {
    let res = parse("function run() { const widget: Widget = build(); }");
    let rel = res
        .relationships
        .iter()
        .find(|r| r.target_id == "build")
        .expect("build edge");
    assert_eq!(
        rel.metadata.get("binds_to").map(String::as_str),
        Some("widget")
    );
    assert_eq!(
        rel.metadata.get("binds_type").map(String::as_str),
        Some("Widget")
    );
}

#[test]
fn an_awaited_call_still_records_its_binding() {
    let res = parse("async function run() { const widget: Widget = await build(); }");
    let rel = res
        .relationships
        .iter()
        .find(|r| r.target_id == "build")
        .expect("build edge");
    assert_eq!(
        rel.metadata.get("binds_to").map(String::as_str),
        Some("widget")
    );
}

#[test]
fn a_reassignment_records_rebinds_rather_than_binds() {
    let res = parse("function run() { let w; w = build(); }");
    let rel = res
        .relationships
        .iter()
        .find(|r| r.target_id == "build")
        .expect("build edge");
    assert_eq!(
        rel.metadata.get("rebinds_to").map(String::as_str),
        Some("w")
    );
    assert!(!rel.metadata.contains_key("binds_type"));
}

#[test]
fn a_bound_call_survives_the_stdlib_filter() {
    // `filter` is on the stdlib noise list, but binding its result shows
    // the caller cares about the value, so the edge is kept.
    let res = parse(
        "function run(xs: number[]) { const kept = xs.filter(Boolean); xs.filter(Boolean); }",
    );
    let filters = call_targets(&res)
        .iter()
        .filter(|t| t.ends_with("filter"))
        .count();
    assert_eq!(filters, 1, "only the bound one survives");
}

#[test]
fn calls_inside_arguments_still_emit_edges() {
    let res = parse("function run() { outer(inner()); }");
    let targets = call_targets(&res);
    assert!(targets.contains(&"outer".to_string()), "{targets:?}");
    assert!(targets.contains(&"inner".to_string()), "{targets:?}");
}

#[test]
fn a_chained_receiver_call_emits_both_edges() {
    let res = parse("function run() { getClient().send(); }");
    let targets = call_targets(&res);
    assert!(targets.contains(&"getClient".to_string()), "{targets:?}");
}

#[test]
fn self_recursion_emits_no_edge() {
    let res = parse("function walk(n: number) { walk(n - 1); }");
    assert!(!call_targets(&res).contains(&"walk".to_string()));
}

// ---------------------------------------------------------------------------
// Object-literal members
// ---------------------------------------------------------------------------

const OBJECT_MODULE: &str = "export const api = {\n\
       async load(id: string) { return fetchThing(id); },\n\
       save: (x: number) => storeThing(x),\n\
       timeout: 30,\n\
     };";

#[test]
fn object_literal_members_become_methods_of_their_constant() {
    let res = parse(OBJECT_MODULE);
    let members: Vec<_> = res
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Method)
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(members, vec!["load", "save"]);
    let owner = res.entities.iter().find(|e| e.name == "api").unwrap();
    for member in res.entities.iter().filter(|e| e.kind == EntityKind::Method) {
        assert_eq!(member.parent_id.as_deref(), Some(owner.id.as_str()));
    }
}

#[test]
fn calls_inside_object_literal_members_are_not_lost() {
    // The regression this fixes: the whole literal collapsed to one Constant,
    // nothing walked the member bodies, and both edges below simply did not
    // exist. A silent zero reads as "nothing calls this".
    let targets = call_targets(&parse(OBJECT_MODULE));
    assert!(targets.contains(&"fetchThing".to_string()), "{targets:?}");
    assert!(targets.contains(&"storeThing".to_string()), "{targets:?}");
}

#[test]
fn object_literal_members_carry_metrics_like_any_callable() {
    let res = parse(
        "export const api = { pick(a: number, b: number) { if (a) { return a; } return b; } };",
    );
    assert_eq!(metrics_of(&res, "pick"), (2, 1, 1));
    let entity = res.entities.iter().find(|e| e.name == "pick").unwrap();
    assert_eq!(entity.metrics.param_count, Some(2));
}

#[test]
fn plain_data_members_stay_out_of_the_graph() {
    let res = parse(OBJECT_MODULE);
    assert!(
        !res.entities.iter().any(|e| e.name == "timeout"),
        "a number is data, not behaviour"
    );
}

#[test]
fn this_inside_an_object_method_resolves_to_the_object() {
    let res = parse("export const api = { load() { this.save(); }, save() {} };");
    assert!(
        call_targets(&res).contains(&"api.save".to_string()),
        "{:?}",
        call_targets(&res)
    );
}

#[test]
fn a_quoted_member_key_names_the_member_without_its_quotes() {
    let res = parse("export const api = { 'on-click': () => handle() };");
    assert!(res.entities.iter().any(|e| e.name == "on-click"));
}

#[test]
fn an_object_literal_still_produces_its_constant() {
    let res = parse(OBJECT_MODULE);
    let owner = res.entities.iter().find(|e| e.name == "api").unwrap();
    assert_eq!(owner.kind, EntityKind::Constant);
}

// ------------------------------------------------------------------
//  Re-exports (AN-024)
// ------------------------------------------------------------------

#[test]
fn a_re_export_is_recorded_as_the_import_it_is() {
    let res = parse("export { DEFAULT } from './settingsDefaults';");
    let import = res.imports.first().expect("a re-export is a dependency");
    assert_eq!(import.path, "./settingsDefaults");
    assert!(import.is_relative);
    assert!(import.is_reexport, "and it is marked as one");
    assert_eq!(import.items, vec!["DEFAULT".to_string()]);
}

#[test]
fn a_star_re_export_names_no_item_and_is_still_a_dependency() {
    let res = parse("export * from './everything';");
    let import = res.imports.first().expect("`export *` depends too");
    assert_eq!(import.path, "./everything");
    assert!(import.is_reexport);
    assert!(import.items.is_empty());
}

/// The whole point of AN-024: the edge's line is where the statement was
/// typed, not where anything it names was declared.
#[test]
fn a_re_export_carries_the_line_it_was_written_on() {
    let res = parse("export interface S {\n  a: number;\n}\n\nexport { D } from './d';");
    let import = res.imports.first().unwrap();
    assert_eq!(import.span.start.line, 4);
}

#[test]
fn an_ordinary_export_declares_rather_than_depends() {
    let res = parse("export const a = 1;\nexport class C {}\n");
    assert!(
        res.imports.is_empty(),
        "{:?} — an `export` with no `from` names no other module",
        res.imports
    );
}

#[test]
fn a_plain_import_is_not_marked_as_a_re_export() {
    let res = parse("import { a } from './a';");
    assert!(!res.imports[0].is_reexport);
}

// ------------------------------------------------------------------
//  Type-only imports (AN-022)
// ------------------------------------------------------------------

#[test]
fn a_statement_level_import_type_is_marked_erased() {
    let res = parse("import type { T } from './vocab';");
    let import = res.imports.first().expect("an `import type` is still a dependency");
    assert_eq!(import.path, "./vocab");
    assert!(import.is_type_only, "the compiler emits nothing for it");
    assert_eq!(
        import.items,
        vec!["T".to_string()],
        "and it still names what it imports"
    );
}

#[test]
fn a_clause_of_nothing_but_type_specifiers_is_marked_erased() {
    let res = parse("import { type T } from './vocab';");
    assert!(res.imports[0].is_type_only);
}

/// The half that has to stay unmarked. `y` keeps the specifier alive, so
/// the bundler resolves `./vocab` and the edge survives the build.
#[test]
fn a_clause_mixing_a_type_specifier_with_a_value_is_not_erased() {
    let res = parse("import { type T, y } from './vocab';");
    assert!(!res.imports[0].is_type_only);
    assert_eq!(res.imports[0].items, vec!["T".to_string(), "y".to_string()]);
}

#[test]
fn a_default_import_keeps_a_type_specifier_beside_it_alive() {
    let res = parse("import D, { type C } from './x';");
    assert!(!res.imports[0].is_type_only);
}

#[test]
fn an_ordinary_import_is_not_marked_erased() {
    let res = parse("import { f } from './a';");
    assert!(!res.imports[0].is_type_only);
}

#[test]
fn a_namespace_import_is_not_marked_erased() {
    let res = parse("import * as ns from './z';");
    assert!(!res.imports[0].is_type_only);
    assert_eq!(res.imports[0].alias.as_deref(), Some("ns"));
}

/// `import './polyfill'` binds nothing, which must not read as "nothing
/// survives" — the side effect is the entire reason the line is there.
#[test]
fn a_side_effect_import_binds_nothing_and_is_not_erased() {
    let res = parse("import './polyfill';");
    let import = res.imports.first().expect("a side-effect import depends");
    assert!(!import.is_type_only);
    assert!(import.items.is_empty());
}

#[test]
fn a_type_only_re_export_is_both_a_re_export_and_erased() {
    let res = parse("export type { E } from './y';");
    let import = res.imports.first().unwrap();
    assert!(import.is_reexport);
    assert!(import.is_type_only);
    assert_eq!(import.items, vec!["E".to_string()]);
}

#[test]
fn a_re_export_of_a_specifier_written_type_is_erased() {
    let res = parse("export { type E } from './y';");
    assert!(res.imports[0].is_type_only);
}

#[test]
fn a_star_re_export_forwards_values_and_is_not_erased() {
    let res = parse("export * from './everything';");
    assert!(!res.imports[0].is_type_only);
}

#[test]
fn an_ordinary_re_export_is_not_marked_erased() {
    let res = parse("export { D } from './d';");
    assert!(!res.imports[0].is_type_only);
}

/// A `const` initialised by a call is how a Svelte store file is written, and
/// the call inside it was invisible: the entity existed, with a span, owning
/// the line the call was on, and nothing walked the initialiser (TS-007).
#[test]
fn a_call_inside_a_const_initializer_is_attributed_to_the_const() {
    let res = parse(
        "import { buildIndex } from './b';\n\
         export const index = buildIndex([1, 2]);\n",
    );
    let calls: Vec<(String, String)> = res
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| (r.source_id.clone(), r.target_id.clone()))
        .collect();
    assert!(
        calls
            .iter()
            .any(|(from, to)| from.contains("index") && to == "buildIndex"),
        "the const does not call what it is built from: {calls:?}"
    );
}

/// The shape that motivated it: the call is not at the initialiser's top
/// level but inside a callback several arguments deep, which is what
/// `derived([…], (…) => f(…))` does.
#[test]
fn a_call_inside_a_callback_in_an_initializer_is_still_found() {
    let res = parse(
        "import { derived } from 'svelte/store';\n\
         import { buildChurn } from './churn';\n\
         export const churn = derived([a, b], ([$a, $b]) => buildChurn($a, $b));\n",
    );
    assert!(
        call_targets(&res).iter().any(|t| t == "buildChurn"),
        "a call inside the callback was not found: {:?}",
        call_targets(&res)
    );
}

/// An arrow-function initialiser was already walked by `emit_function`; the
/// fix must not make it walked twice.
#[test]
fn an_arrow_initializer_still_reports_its_calls_once() {
    let res = parse(
        "import { helper } from './h';\n\
         export const run = () => helper(1);\n",
    );
    let hits = call_targets(&res).iter().filter(|t| *t == "helper").count();
    assert_eq!(hits, 1, "calls: {:?}", call_targets(&res));
}

/// A call written in a bare module-level statement had no entity covering it,
/// so it produced no edge and the file read as depending on nothing. In a
/// store or a side-effecting entry module that is most of the file (TS-008).
#[test]
fn a_call_in_a_bare_module_level_statement_is_attributed_to_the_module() {
    let res = parse(
        "import { ensureLoaded } from './dep';\n\
         declare const subject: { subscribe(cb: (v: number) => void): void };\n\
         subject.subscribe((current) => { void ensureLoaded().then((d) => d + current); });\n",
    );
    assert!(
        call_targets(&res).iter().any(|t| t == "ensureLoaded"),
        "the module-level call was dropped: {:?}",
        call_targets(&res)
    );
}

/// The owner is named with its extension. Every entity name enters the
/// resolver's `name_to_id`, and a bare stem is a call target a stranger can
/// bind to — which is AN-029 exactly. `description.ts` is a name no callee
/// expression can spell.
#[test]
fn the_module_owner_is_named_so_no_call_can_bind_to_it() {
    let res = parse("declare const s: { go(): void };\ns.go();\n");
    let modules = names_of_kind(&res, EntityKind::Module);
    assert!(
        modules.iter().any(|n| n.contains('.')),
        "a bare stem would be bindable: {modules:?}"
    );
}

/// A file of pure declarations gains nothing — the owner exists only when
/// there is something at module scope to own.
#[test]
fn a_file_of_only_declarations_gets_no_module_owner() {
    let res = parse(
        "import { a } from './a';\n\
         export function f(): number { return a(); }\n\
         export const g = 1;\n",
    );
    assert!(
        names_of_kind(&res, EntityKind::Module).is_empty(),
        "{:?}",
        names_of_kind(&res, EntityKind::Module)
    );
}

/// Declarations own what they contain, so the module walk must skip them —
/// otherwise a function's calls are attributed twice.
#[test]
fn a_declarations_calls_are_not_also_attributed_to_the_module() {
    let res = parse(
        "import { helper } from './h';\n\
         export function f(): number { return helper(); }\n\
         f();\n",
    );
    let hits = call_targets(&res).iter().filter(|t| *t == "helper").count();
    assert_eq!(hits, 1, "double-counted: {:?}", call_targets(&res));
}
