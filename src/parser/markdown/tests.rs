//! Tests for the markdown link parser.
//!
//! Named for the behaviour they pin rather than the function they call —
//! each one states a claim the document graph makes.

use super::*;
use crate::models::RelationshipKind;
use std::path::PathBuf;

fn parse(path: &str, content: &str) -> ParseResult {
    MarkdownParser::new()
        .parse(&PathBuf::from(path), content)
        .expect("markdown parsing never fails")
}

fn note(result: &ParseResult) -> &CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Note)
        .expect("every file yields exactly one note")
}

/// Every relationship target, in emission order.
fn targets(result: &ParseResult) -> Vec<&str> {
    result
        .relationships
        .iter()
        .map(|r| r.target_id.as_str())
        .collect()
}

fn code_refs(result: &ParseResult) -> Vec<&str> {
    note(result)
        .attributes
        .iter()
        .filter_map(|a| a.strip_prefix("mdref:"))
        .collect()
}

// =====================================================================
// The note itself
// =====================================================================

#[test]
fn a_file_becomes_exactly_one_note() {
    let result = parse("/vault/setup.md", "# Setup\n\nHow to install.\n");
    assert_eq!(result.entities.len(), 1);
    assert_eq!(note(&result).kind, EntityKind::Note);
}

#[test]
fn a_notes_id_is_its_path_so_a_link_can_predict_it() {
    let result = parse("/vault/docs/setup.md", "# Setup\n");
    assert_eq!(note(&result).id, "md::note./vault/docs/setup.md");
}

#[test]
fn the_first_h1_titles_the_note() {
    let result = parse("/vault/a.md", "# Real Title\n\nBody.\n");
    assert_eq!(note(&result).name, "Real Title");
}

#[test]
fn frontmatter_title_beats_the_h1() {
    let result = parse("/vault/a.md", "---\ntitle: Declared\n---\n\n# Written\n");
    assert_eq!(note(&result).name, "Declared");
}

#[test]
fn a_note_with_no_heading_is_named_for_its_file() {
    let result = parse("/vault/orphan-note.md", "Just prose.\n");
    assert_eq!(note(&result).name, "orphan-note");
}

#[test]
fn the_filename_is_kept_so_a_wikilink_can_match_either_name() {
    let result = parse("/vault/setup-guide.md", "# Getting Started\n");
    assert!(note(&result)
        .attributes
        .contains(&"stem:setup-guide".to_string()));
}

#[test]
fn the_first_paragraph_becomes_the_summary() {
    let result = parse("/vault/a.md", "# Title\n\nThe opening line.\n\nA second one.\n");
    assert_eq!(
        note(&result).documentation.as_deref(),
        Some("The opening line.")
    );
}

#[test]
fn frontmatter_is_not_mistaken_for_the_summary() {
    let result = parse("/vault/a.md", "---\ntags: [x]\n---\n\nReal prose.\n");
    assert_eq!(note(&result).documentation.as_deref(), Some("Real prose."));
}

// =====================================================================
// Links between notes
// =====================================================================

#[test]
fn a_relative_link_resolves_to_the_target_notes_own_id() {
    let result = parse("/vault/docs/a.md", "See [B](../guides/b.md).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/guides/b.md"]);
}

#[test]
fn a_sibling_link_needs_no_dot_prefix() {
    let result = parse("/vault/a.md", "See [B](b.md).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
}

#[test]
fn a_dot_slash_link_resolves_the_same_as_a_bare_one() {
    let result = parse("/vault/a.md", "See [B](./b.md).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
}

#[test]
fn a_link_is_a_reference_edge() {
    let result = parse("/vault/a.md", "See [B](b.md).\n");
    assert_eq!(result.relationships[0].kind, RelationshipKind::References);
    assert_eq!(
        result.relationships[0].metadata.get("link").map(String::as_str),
        Some("inline")
    );
}

#[test]
fn a_section_link_keeps_the_anchor_and_still_reaches_the_note() {
    let result = parse("/vault/a.md", "See [B](b.md#install).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
    assert_eq!(
        result.relationships[0].metadata.get("anchor").map(String::as_str),
        Some("install")
    );
}

#[test]
fn a_space_in_a_filename_survives_percent_encoding() {
    let result = parse("/vault/a.md", "See [B](my%20note.md).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/my note.md"]);
}

#[test]
fn a_link_title_is_not_part_of_the_destination() {
    let result = parse("/vault/a.md", "See [B](b.md \"The B Note\").\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
}

#[test]
fn angle_brackets_around_a_destination_are_stripped() {
    let result = parse("/vault/a.md", "See [B](<b.md>).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
}

// =====================================================================
// Wikilinks
// =====================================================================

#[test]
fn a_wikilink_targets_a_name_the_analyzer_will_resolve() {
    let result = parse("/vault/a.md", "See [[Other Note]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.other note"]);
}

#[test]
fn a_wikilinks_case_does_not_matter() {
    let result = parse("/vault/a.md", "See [[OTHER note]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.other note"]);
}

#[test]
fn an_alias_changes_the_label_not_the_target() {
    let result = parse("/vault/a.md", "See [[Other Note|that one]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.other note"]);
}

#[test]
fn an_embed_is_marked_as_one() {
    let result = parse("/vault/a.md", "![[Other Note]]\n");
    assert_eq!(targets(&result), vec!["md::wiki.other note"]);
    assert_eq!(
        result.relationships[0].metadata.get("link").map(String::as_str),
        Some("embed")
    );
}

#[test]
fn a_path_shaped_wikilink_resolves_on_its_last_segment() {
    let result = parse("/vault/a.md", "See [[docs/setup]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.setup"]);
}

#[test]
fn a_wikilink_naming_a_file_drops_the_extension() {
    let result = parse("/vault/a.md", "See [[setup.md]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.setup"]);
}

#[test]
fn two_wikilinks_on_one_line_both_count() {
    let result = parse("/vault/a.md", "[[One]] and [[Two]].\n");
    assert_eq!(targets(&result), vec!["md::wiki.one", "md::wiki.two"]);
}

// =====================================================================
// Links that leave the docs — the half Obsidian cannot draw
// =====================================================================

#[test]
fn a_link_to_source_becomes_a_code_ref_not_an_edge() {
    let result = parse("/repo/docs/adr/0003.md", "See [the parser](../../src/parser/mod.rs).\n");
    assert!(result.relationships.is_empty());
    assert_eq!(code_refs(&result), vec!["/repo/src/parser/mod.rs"]);
}

#[test]
fn a_link_to_a_folder_is_a_code_ref_too() {
    let result = parse("/repo/docs/a.md", "Lives in [here](../src/parser).\n");
    assert_eq!(code_refs(&result), vec!["/repo/src/parser"]);
}

#[test]
fn the_same_file_referenced_twice_yields_one_ref() {
    let result = parse(
        "/repo/docs/a.md",
        "See [x](../src/a.rs) and again [x](../src/a.rs).\n",
    );
    assert_eq!(code_refs(&result), vec!["/repo/src/a.rs", "/repo/src/a.rs"]);
}

// =====================================================================
// Links that are not claims about the graph
// =====================================================================

#[test]
fn an_external_url_is_not_a_link_in_the_graph() {
    let result = parse("/vault/a.md", "See [docs](https://example.com/b.md).\n");
    assert!(result.relationships.is_empty());
    assert!(code_refs(&result).is_empty());
}

#[test]
fn a_page_local_anchor_reaches_nothing() {
    let result = parse("/vault/a.md", "Jump to [install](#install).\n");
    assert!(result.relationships.is_empty());
}

#[test]
fn a_mailto_is_not_a_file() {
    let result = parse("/vault/a.md", "Mail [us](mailto:a@example.com).\n");
    assert!(result.relationships.is_empty());
    assert!(code_refs(&result).is_empty());
}

#[test]
fn a_link_that_climbs_above_the_root_is_dropped_rather_than_guessed() {
    let result = parse("/a.md", "See [B](../../../b.md).\n");
    assert!(result.relationships.is_empty());
}

#[test]
fn a_link_inside_a_fenced_block_is_a_code_sample() {
    let result = parse(
        "/vault/a.md",
        "Example:\n\n```markdown\n[B](b.md) and [[Other]]\n```\n",
    );
    assert!(result.relationships.is_empty());
}

#[test]
fn a_tilde_fence_masks_just_as_well() {
    let result = parse("/vault/a.md", "~~~\n[B](b.md)\n~~~\n");
    assert!(result.relationships.is_empty());
}

#[test]
fn a_backtick_run_inside_a_tilde_fence_does_not_end_it() {
    let result = parse("/vault/a.md", "~~~\n```\n[B](b.md)\n```\n~~~\n");
    assert!(result.relationships.is_empty());
}

#[test]
fn a_link_inside_inline_code_is_a_code_sample_too() {
    let result = parse("/vault/a.md", "Write `[B](b.md)` to link.\n");
    assert!(result.relationships.is_empty());
}

#[test]
fn prose_after_a_fence_is_read_again() {
    let result = parse("/vault/a.md", "```\n[X](x.md)\n```\n\nSee [B](b.md).\n");
    assert_eq!(targets(&result), vec!["md::note./vault/b.md"]);
}

#[test]
fn an_unterminated_fence_does_not_eat_the_rest_of_the_file() {
    // Everything after an unclosed fence is genuinely inside it. The claim
    // is that parsing still completes and yields the note.
    let result = parse("/vault/a.md", "```\n[B](b.md)\n");
    assert_eq!(result.entities.len(), 1);
    assert!(result.relationships.is_empty());
}

// =====================================================================
// Determinism and robustness
// =====================================================================

#[test]
fn parsing_the_same_file_twice_gives_the_same_graph() {
    let source = "# A\n\n[B](b.md), [[C]], [src](../src/x.rs)\n";
    let first = parse("/vault/a.md", source);
    let second = parse("/vault/a.md", source);
    assert_eq!(targets(&first), targets(&second));
    assert_eq!(code_refs(&first), code_refs(&second));
}

#[test]
fn an_empty_file_still_yields_its_note() {
    let result = parse("/vault/empty.md", "");
    assert_eq!(result.entities.len(), 1);
    assert_eq!(note(&result).name, "empty");
}

#[test]
fn an_unclosed_bracket_costs_one_link_not_the_file() {
    let result = parse("/vault/a.md", "Broken [B](b.md and fine [C](c.md).\n");
    // The first `](` runs to the only `)` on the line, so it swallows the
    // second link's text — but the file still parses and the note survives.
    assert_eq!(result.entities.len(), 1);
}

#[test]
fn unicode_in_a_title_does_not_panic_the_truncator() {
    let long = "é".repeat(400);
    let result = parse("/vault/a.md", &format!("# T\n\n{}\n", long));
    assert!(note(&result).documentation.as_deref().unwrap().ends_with('…'));
}
