//! Parser tests for the flat-syntax Elevator (`.elv`) language.

use super::*;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use std::path::PathBuf;

fn parse(src: &str) -> ParseResult {
    let parser = ElevatorParser::new();
    parser
        .parse(&PathBuf::from("test.elv"), src)
        .expect("parse should not fail")
}

fn find<'a>(result: &'a ParseResult, id: &str) -> Option<&'a CodeEntity> {
    result.entities.iter().find(|e| e.id == id)
}

fn has_rel(result: &ParseResult, src: &str, tgt: &str, kind: RelationshipKind) -> bool {
    result
        .relationships
        .iter()
        .any(|r| r.source_id == src && r.target_id == tgt && r.kind == kind)
}

#[test]
fn empty_file_produces_nothing() {
    let r = parse("");
    assert!(r.entities.is_empty());
    assert!(r.relationships.is_empty());
}

#[test]
fn category_definition_creates_category_entity() {
    let r = parse("c library");
    let e = find(&r, "elevator::c.library").expect("category exists");
    assert_eq!(e.kind, EntityKind::Category);
    assert!(e.parent_id.is_none());
}

#[test]
fn flat_feature_definition() {
    let r = parse("f protocol");
    let e = find(&r, "elevator::f.protocol").expect("feature exists");
    assert_eq!(e.kind, EntityKind::Feature);
}

#[test]
fn nested_definitions_disallowed_top_level_only() {
    // Even if the user writes definitions in any order, every
    // definition is independent — there is no nesting.
    let src = r#"
        c library {
            f protocol
        }
        f protocol {
            d: "Structured prompts."
        }
    "#;
    let r = parse(src);
    let cat = find(&r, "elevator::c.library").expect("library exists");
    let feat = find(&r, "elevator::f.protocol").expect("protocol exists");
    // Cross-reference produces a Contains edge. `parent_id` itself is
    // set by the analyzer's post-merge `derive_parent_from_contains`
    // pass, not by the parser, so we don't assert it here.
    assert!(has_rel(&r, &cat.id, &feat.id, RelationshipKind::Contains));
    // The definition's own description survives.
    assert_eq!(feat.documentation.as_deref(), Some("Structured prompts."));
}

#[test]
fn bare_fu_inside_feature_resolves_to_qualified_id() {
    let src = r#"
        f protocol {
            fu creation
            fu edition
        }
        fu f.protocol.creation {
            where: ui.library.protocol
        }
    "#;
    let r = parse(src);
    let creation = find(&r, "elevator::fu.protocol.creation")
        .expect("functionality `protocol.creation` exists");
    assert_eq!(creation.kind, EntityKind::Functionality);
    // Contains edge encodes the parent relationship (parent_id itself
    // is filled in by the analyzer post-merge).
    assert!(has_rel(
        &r,
        "elevator::f.protocol",
        "elevator::fu.protocol.creation",
        RelationshipKind::Contains
    ));
    // Edition is referenced as a child but never defined. The Contains
    // edge is still emitted (so cross-file resolution can pick it up
    // when sibling .elv files define it), but no stub entity is
    // created — that would race the real definition during merge.
    assert!(find(&r, "elevator::fu.protocol.edition").is_none());
    assert!(has_rel(
        &r,
        "elevator::f.protocol",
        "elevator::fu.protocol.edition",
        RelationshipKind::Contains
    ));
}

#[test]
fn fu_must_be_qualified_at_top_level() {
    let r = parse("fu creation");
    // Definition is still parsed but a warning surfaces.
    assert!(
        r.warnings.iter().any(|w| w.contains("must be qualified")),
        "expected qualification warning, got {:?}",
        r.warnings
    );
}

#[test]
fn category_must_be_bare_name() {
    let r = parse("c library.foo");
    assert!(
        r.warnings.iter().any(|w| w.contains("must be a bare name")),
        "expected bare-name warning, got {:?}",
        r.warnings
    );
}

#[test]
fn where_field_emits_uipage_edge_with_link_metadata() {
    let src = r#"
        fu f.protocol.creation {
            where: ui.library.protocol
        }
    "#;
    let r = parse(src);
    let edge = r
        .relationships
        .iter()
        .find(|rel| {
            rel.source_id == "elevator::fu.protocol.creation"
                && rel.target_id == "elevator::ui.library.protocol"
        })
        .expect("where edge exists");
    assert_eq!(edge.kind, RelationshipKind::References);
    assert_eq!(edge.metadata.get("link").map(String::as_str), Some("where"));
    // UI page auto-created.
    let page = find(&r, "elevator::ui.library.protocol").expect("ui page exists");
    assert_eq!(page.kind, EntityKind::UiPage);
    assert!(page.tags.contains("auto_created"));
}

#[test]
fn references_field_creates_feature_to_feature_edge() {
    let src = r#"
        f conversation
        fu f.protocol.call_during_conversation {
            references: f.conversation
        }
    "#;
    let r = parse(src);
    assert!(r.relationships.iter().any(|rel| {
        rel.source_id == "elevator::fu.protocol.call_during_conversation"
            && rel.target_id == "elevator::f.conversation"
            && rel.metadata.get("link").map(String::as_str) == Some("references")
    }));
}

#[test]
fn concept_used_by_emits_edges_from_consumers_to_concept() {
    let src = r#"
        f protocol
        f template
        concept llm_invocation {
            d: "Anything that calls the model."
            used_by: f.protocol, f.template
        }
    "#;
    let r = parse(src);
    let concept = find(&r, "elevator::concept.llm_invocation").unwrap();
    assert_eq!(concept.kind, EntityKind::Concept);
    for source in ["elevator::f.protocol", "elevator::f.template"] {
        assert!(
            r.relationships.iter().any(|rel| {
                rel.source_id == source
                    && rel.target_id == "elevator::concept.llm_invocation"
                    && rel.metadata.get("link").map(String::as_str) == Some("used_by")
            }),
            "missing used_by edge from {}",
            source
        );
    }
}

#[test]
fn kind_prefix_in_child_reference_is_stripped() {
    // `f f.protocol` and `f protocol` should be equivalent.
    let prefixed = parse("c library { f f.protocol }");
    let bare = parse("c library { f protocol }");
    let p_target = "elevator::f.protocol";
    let b_target = "elevator::f.protocol";
    assert!(has_rel(
        &prefixed,
        "elevator::c.library",
        p_target,
        RelationshipKind::Contains
    ));
    assert!(has_rel(
        &bare,
        "elevator::c.library",
        b_target,
        RelationshipKind::Contains
    ));
}

#[test]
fn multiple_categories_can_share_a_feature_via_contains_edges() {
    let src = r#"
        c library { f protocol }
        c orchestration { f protocol }
        f protocol
    "#;
    let r = parse(src);
    // Two Contains edges to the same feature — one per parent.
    let edges: Vec<_> = r
        .relationships
        .iter()
        .filter(|rel| {
            rel.target_id == "elevator::f.protocol" && rel.kind == RelationshipKind::Contains
        })
        .collect();
    assert_eq!(edges.len(), 2);
    // (Which parent "wins" for parent_id is decided by
    // `derive_parent_from_contains` — first Contains edge wins —
    // but the parser stays out of it, so this test no longer
    // asserts on parent_id.)
}

#[test]
fn comma_between_siblings_is_optional() {
    let with_commas = parse("c a, c b, c c");
    let without_commas = parse("c a c b c c");
    assert_eq!(with_commas.entities.len(), 3);
    assert_eq!(without_commas.entities.len(), 3);
}

#[test]
fn line_comments_are_ignored() {
    let src = r#"
        # top-level comment
        c library { # trailing comment
            d: "kept"
            # internal comment
            f protocol
        }
    "#;
    let r = parse(src);
    let cat = find(&r, "elevator::c.library").unwrap();
    assert_eq!(cat.documentation.as_deref(), Some("kept"));
    // The child reference produces a Contains edge even without a
    // stub entity. The actual `f protocol` definition would live in
    // another statement (or another file).
    assert!(has_rel(
        &r,
        "elevator::c.library",
        "elevator::f.protocol",
        RelationshipKind::Contains
    ));
}

#[test]
fn no_stub_entity_for_referenced_features() {
    // A child reference inside a body must NOT synthesise a stub
    // entity for non-UI kinds — that would race the real definition
    // in another file during analyzer merge.
    let r = parse("c library { f protocol }");
    assert!(find(&r, "elevator::c.library").is_some());
    assert!(find(&r, "elevator::f.protocol").is_none());
    assert!(has_rel(
        &r,
        "elevator::c.library",
        "elevator::f.protocol",
        RelationshipKind::Contains
    ));
}

#[test]
fn unrecognised_top_level_keyword_warns_and_recovers() {
    let r = parse("garbage c library");
    assert!(find(&r, "elevator::c.library").is_some());
    assert!(!r.warnings.is_empty());
}

#[test]
fn import_statements_recorded_on_parse_result() {
    let src = r#"
        import "categories.elv"
        import "library/protocol.elv"
        f workflows
    "#;
    let r = parse(src);
    assert_eq!(r.imports.len(), 2);
    assert_eq!(r.imports[0].path, "categories.elv");
    assert_eq!(r.imports[1].path, "library/protocol.elv");
    assert!(r.imports.iter().all(|i| i.is_relative));
    // Definitions still parse correctly after imports.
    assert!(find(&r, "elevator::f.workflows").is_some());
}

#[test]
fn import_after_definition_is_warned() {
    let src = r#"
        f workflows
        import "categories.elv"
    "#;
    let r = parse(src);
    assert!(
        r.warnings.iter().any(|w| w.contains("must precede")),
        "expected ordering warning, got {:?}",
        r.warnings
    );
}

#[test]
fn cr_field_attaches_code_refs_to_entity() {
    let src = r#"
        f protocol {
            d: "Structured prompts."
            cr: "src/protocol/"
        }

        fu f.protocol.creation {
            cr: "src/protocol/builder.rs", "src/protocol/validator.rs"
        }
    "#;
    let r = parse(src);
    let proto = find(&r, "elevator::f.protocol").expect("protocol exists");
    assert_eq!(crs_with_prefix(proto, "cr:"), vec!["src/protocol/"]);

    let creation = find(&r, "elevator::fu.protocol.creation").expect("creation exists");
    assert_eq!(
        crs_with_prefix(creation, "cr:"),
        vec!["src/protocol/builder.rs", "src/protocol/validator.rs"]
    );
}

#[test]
fn cr_tagged_partitions_refs_by_layer() {
    // `cr.fe:` and `cr.be:` are stored under their own attribute
    // prefix so the renderer (and anyone else reading attributes)
    // can pick one layer cleanly.
    let src = r#"
        fu f.protocol.creation {
            cr: "shared/protocol.proto"
            cr.fe: "frontend/protocol.tsx"
            cr.be: "backend/protocol/builder.rs", "backend/protocol/validator.rs"
        }
    "#;
    let r = parse(src);
    let creation = find(&r, "elevator::fu.protocol.creation").expect("creation exists");

    // Generic cr: still works.
    assert_eq!(
        crs_with_prefix(creation, "cr:"),
        vec!["shared/protocol.proto"]
    );
    // Tagged cr.fe / cr.be land under their own prefixes.
    assert_eq!(
        crs_with_prefix(creation, "cr.fe:"),
        vec!["frontend/protocol.tsx"]
    );
    assert_eq!(
        crs_with_prefix(creation, "cr.be:"),
        vec![
            "backend/protocol/builder.rs",
            "backend/protocol/validator.rs"
        ]
    );
}

fn crs_with_prefix<'a>(entity: &'a CodeEntity, prefix: &str) -> Vec<&'a str> {
    entity
        .attributes
        .iter()
        .filter_map(|a| a.strip_prefix(prefix))
        // Filter out tagged cr.X: when caller asked for the bare cr:.
        .filter(|p| !(prefix == "cr:" && p.contains(':')))
        .collect()
}

#[test]
fn cr_requires_quoted_strings() {
    // Bare paths would fail lexing on `/`, but even valid identifiers
    // get rejected — the field's contract is "quoted path strings".
    let r = parse(r#"f protocol { cr: src }"#);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("expected string literal after `cr:`")),
        "expected string-literal warning, got {:?}",
        r.warnings
    );
}

#[test]
fn extension_top_level_with_category_and_concept_children() {
    let src = r#"
        e ui_kit {
            d: "Reusable component library."
            c forms
            concept theming
        }

        c forms {
            f text_input
        }

        concept theming {
            d: "Look-and-feel customisation."
        }
    "#;
    let r = parse(src);
    let ext = find(&r, "elevator::e.ui_kit").expect("extension exists");
    assert_eq!(ext.kind, EntityKind::Extension);
    assert_eq!(
        ext.documentation.as_deref(),
        Some("Reusable component library.")
    );

    // Containment edges from the Extension to its children.
    assert!(has_rel(
        &r,
        "elevator::e.ui_kit",
        "elevator::c.forms",
        RelationshipKind::Contains
    ));
    assert!(has_rel(
        &r,
        "elevator::e.ui_kit",
        "elevator::concept.theming",
        RelationshipKind::Contains
    ));
}

#[test]
fn extension_dotted_name_is_warned() {
    // Extensions, like Categories and Features, must be bare names.
    let r = parse("e foo.bar");
    assert!(
        r.warnings.iter().any(|w| w.contains("must be a bare name")),
        "expected bare-name warning, got {:?}",
        r.warnings
    );
}

#[test]
fn used_by_outside_concept_is_warned() {
    let r = parse("f protocol { used_by: f.x }");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("only valid inside `concept`")),
        "expected warning, got {:?}",
        r.warnings
    );
}

// =====================================================================
// Spans, source text and metrics
// =====================================================================

#[test]
fn definition_span_covers_keyword_through_closing_brace() {
    let src = "# header\nf protocol {\n    d: \"Structured prompts.\"\n}\n";
    let r = parse(src);
    let e = find(&r, "elevator::f.protocol").unwrap();
    // Starts at the `f` on line 1 (0-based), column 0.
    assert_eq!((e.span.start.line, e.span.start.column), (1, 0));
    // Ends just past the `}` on line 3.
    assert_eq!((e.span.end.line, e.span.end.column), (3, 1));
    assert_eq!(
        &src[e.span.start.offset..e.span.end.offset],
        e.source_code.as_deref().unwrap()
    );
}

#[test]
fn bodyless_definition_span_ends_at_the_name() {
    let r = parse("f protocol\nf template");
    let e = find(&r, "elevator::f.protocol").unwrap();
    assert_eq!((e.span.start.line, e.span.start.column), (0, 0));
    assert_eq!((e.span.end.line, e.span.end.column), (0, 10));
    assert_eq!(e.source_code.as_deref(), Some("f protocol"));
    assert_eq!(e.metrics.loc, 1);
}

#[test]
fn source_code_captures_the_whole_statement() {
    let src = r#"f protocol {
    d: "Structured prompts."
    cr: "src/protocol/"
    fu creation
}"#;
    let r = parse(src);
    let e = find(&r, "elevator::f.protocol").unwrap();
    // The full statement, so `nao diff` sees a reworded `d:` as a
    // change and MCP `context` has something to quote.
    assert_eq!(e.source_code.as_deref(), Some(src));
}

#[test]
fn loc_counts_the_lines_the_definition_spans() {
    let r = parse("f a\n\nf b {\n  d: \"x\"\n  fu c\n}\n");
    assert_eq!(find(&r, "elevator::f.a").unwrap().metrics.loc, 1);
    assert_eq!(find(&r, "elevator::f.b").unwrap().metrics.loc, 4);
}

#[test]
fn non_ascii_description_keeps_source_slice_valid() {
    // Byte offsets, not char counts — a multi-byte `d:` must not
    // truncate or panic the source slice.
    let src = "f café {\n    d: \"naïve ☕ prompts\"\n}";
    let r = parse(src);
    let e = find(&r, "elevator::f.café");
    // The name itself is non-ASCII so it isn't a valid identifier;
    // what matters is that parsing doesn't panic and the file still
    // yields the diagnostics rather than dying.
    assert!(e.is_none());
    assert!(!r.warnings.is_empty());
}

#[test]
fn import_carries_a_real_span() {
    let r = parse("import \"categories.elv\"\nf a");
    let imp = &r.imports[0];
    assert_eq!(imp.span.start.line, 0);
    assert_eq!(imp.span.start.column, 0);
    assert_eq!(imp.span.end.column, 23);
}

// =====================================================================
// Error recovery
// =====================================================================

#[test]
fn stray_character_costs_one_token_not_the_file() {
    // Regression: the lexer used to bail on the first unexpected
    // character, so a single typo produced zero entities for the
    // whole file.
    let src = r#"
        c alpha { f one }
        f one { d: "kept" @ }
        f two { d: "also kept" }
    "#;
    let r = parse(src);
    assert!(find(&r, "elevator::c.alpha").is_some());
    assert!(find(&r, "elevator::f.one").is_some());
    assert!(find(&r, "elevator::f.two").is_some());
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("unexpected character `@`")),
        "expected a lex warning, got {:?}",
        r.warnings
    );
}

#[test]
fn unterminated_string_does_not_zero_the_rest_of_the_file() {
    let src = "f one { d: \"missing quote\nf two { d: \"kept\" }\n";
    let r = parse(src);
    assert!(find(&r, "elevator::f.two").is_some());
    assert_eq!(
        find(&r, "elevator::f.two")
            .unwrap()
            .documentation
            .as_deref(),
        Some("kept")
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("unterminated string")),
        "expected an unterminated-string warning, got {:?}",
        r.warnings
    );
}

#[test]
fn unclosed_body_reports_where_it_opened() {
    let r = parse("f a\nc library {\n    f a\n");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("inside body opened here at line 2")),
        "expected an unclosed-body warning naming line 2, got {:?}",
        r.warnings
    );
    // Everything parsed before the break is still emitted.
    assert!(find(&r, "elevator::c.library").is_some());
    assert!(has_rel(
        &r,
        "elevator::c.library",
        "elevator::f.a",
        RelationshipKind::Contains
    ));
}

#[test]
fn nested_definition_lands_its_body_on_the_child_not_the_parent() {
    // Regression: recovery used to skip the `{` and merge the inner
    // body into the enclosing definition, so `library` silently
    // acquired protocol's description and protocol got none.
    let r = parse("c library {\n    f protocol {\n        d: \"belongs to protocol\"\n    }\n}");
    assert_eq!(
        find(&r, "elevator::f.protocol")
            .unwrap()
            .documentation
            .as_deref(),
        Some("belongs to protocol")
    );
    assert_eq!(find(&r, "elevator::c.library").unwrap().documentation, None);
    // The containment the author meant is still recorded.
    assert!(has_rel(
        &r,
        "elevator::c.library",
        "elevator::f.protocol",
        RelationshipKind::Contains
    ));
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("definitions never nest")),
        "got {:?}",
        r.warnings
    );
}

#[test]
fn nested_bare_functionality_is_parent_qualified() {
    let r = parse("f protocol {\n    fu creation { d: \"makes one\" }\n}");
    let creation = find(&r, "elevator::fu.protocol.creation").expect("qualified by its Feature");
    assert_eq!(creation.documentation.as_deref(), Some("makes one"));
}

#[test]
fn runaway_braces_terminate_instead_of_blowing_the_stack() {
    let src = format!("c a {}", "{ f b ".repeat(40));
    let r = parse(&src);
    assert!(find(&r, "elevator::c.a").is_some());
    assert!(r.warnings.iter().any(|w| w.contains("nesting too deep")));
}

// =====================================================================
// Diagnostics carry locations
// =====================================================================

#[test]
fn every_warning_names_a_line_and_column() {
    let src = "f a.b\nc x { bogus }\nfu naked\n";
    let r = parse(src);
    assert!(r.warnings.len() >= 3, "got {:?}", r.warnings);
    for w in &r.warnings {
        assert!(
            w.contains(" at line ") && w.contains(" column "),
            "warning without a location: {}",
            w
        );
    }
}

#[test]
fn unexpected_body_keyword_warns_and_keeps_the_rest_of_the_body() {
    let r = parse("f a { bogus d: \"kept\" }");
    assert!(r
        .warnings
        .iter()
        .any(|w| w.contains("unexpected keyword `bogus`")));
    assert_eq!(
        find(&r, "elevator::f.a").unwrap().documentation.as_deref(),
        Some("kept")
    );
}

#[test]
fn empty_edge_list_is_warned() {
    let r = parse("f a { where: }");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("expected at least one reference after `:`")),
        "got {:?}",
        r.warnings
    );
}

#[test]
fn duplicate_description_is_warned_and_the_later_one_wins() {
    let r = parse("f a { d: \"first\" d: \"second\" }");
    assert!(
        r.warnings.iter().any(|w| w.contains("duplicate `d:`")),
        "got {:?}",
        r.warnings
    );
    assert_eq!(
        find(&r, "elevator::f.a").unwrap().documentation.as_deref(),
        Some("second")
    );
}

#[test]
fn bare_fu_child_outside_a_feature_is_warned() {
    // `fu creation` inside a Category would qualify to
    // `fu.<category>.creation`, which no Feature can ever define.
    let r = parse("c library { fu creation }");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("a Functionality is qualified by its Feature")),
        "got {:?}",
        r.warnings
    );
    // Inside a Feature the same reference is silent.
    let ok = parse("f protocol { fu creation }");
    assert!(ok.warnings.is_empty(), "got {:?}", ok.warnings);
}

// =====================================================================
// Duplicate definitions
// =====================================================================

#[test]
fn duplicate_definition_warns_and_keeps_the_first() {
    let r = parse("f dup { d: \"first\" }\nf dup { d: \"second\" }");
    assert_eq!(r.entities.len(), 1);
    assert_eq!(
        find(&r, "elevator::f.dup")
            .unwrap()
            .documentation
            .as_deref(),
        Some("first")
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("is defined twice")),
        "got {:?}",
        r.warnings
    );
}

#[test]
fn a_duplicates_children_are_still_wired_up() {
    // The duplicate loses its entity, but dropping its children would
    // turn an authoring slip into a silently missing graph branch.
    let r = parse("c a { f one }\nc a { f two }");
    assert!(has_rel(
        &r,
        "elevator::c.a",
        "elevator::f.one",
        RelationshipKind::Contains
    ));
    assert!(has_rel(
        &r,
        "elevator::c.a",
        "elevator::f.two",
        RelationshipKind::Contains
    ));
}

#[test]
fn identical_edges_are_emitted_once() {
    let r = parse("c a { f one, f one }");
    let n = r
        .relationships
        .iter()
        .filter(|rel| rel.target_id == "elevator::f.one")
        .count();
    assert_eq!(n, 1);
}

// =====================================================================
// Explicit kind prefixes on edge targets
// =====================================================================

#[test]
fn references_honours_an_explicit_concept_prefix() {
    // Regression: the prefix used to be stripped and then ignored, so
    // `references: concept.tax` silently pointed at a Feature `tax`
    // that no spec would ever define.
    let src = r#"
        concept tax
        f billing { references: concept.tax }
    "#;
    let r = parse(src);
    assert!(
        r.relationships.iter().any(|rel| {
            rel.source_id == "elevator::f.billing"
                && rel.target_id == "elevator::concept.tax"
                && rel.metadata.get("link").map(String::as_str) == Some("references")
        }),
        "got {:?}",
        r.relationships
    );
}

#[test]
fn references_still_defaults_to_feature_without_a_prefix() {
    let r = parse("f billing { references: conversation }");
    assert!(has_link(
        &r,
        "elevator::f.billing",
        "elevator::f.conversation",
        "references"
    ));
}

#[test]
fn where_honours_an_explicit_prefix_and_skips_the_ui_stub() {
    // `where: f.x` is unusual but legal; it must not fabricate a UI
    // page entity for a target that isn't one.
    let r = parse("fu f.a.b { where: f.settings }");
    assert!(has_link(
        &r,
        "elevator::fu.a.b",
        "elevator::f.settings",
        "where"
    ));
    assert!(find(&r, "elevator::ui.settings").is_none());
    assert!(find(&r, "elevator::f.settings").is_none());
}

#[test]
fn used_by_honours_an_explicit_prefix() {
    let r = parse("concept theming { used_by: c.ui_kit }");
    assert!(has_link(
        &r,
        "elevator::c.ui_kit",
        "elevator::concept.theming",
        "used_by"
    ));
}

fn has_link(result: &ParseResult, src: &str, tgt: &str, link: &str) -> bool {
    result.relationships.iter().any(|rel| {
        rel.source_id == src
            && rel.target_id == tgt
            && rel.metadata.get("link").map(String::as_str) == Some(link)
    })
}
