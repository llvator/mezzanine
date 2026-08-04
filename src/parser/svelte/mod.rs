//! Svelte single-file-component parser.
//!
//! A `.svelte` file is three languages stitched together: one or two
//! `<script>` blocks (JS/TS logic), HTML-ish template markup, and a
//! `<style>` block (CSS). Rather than add a second tree-sitter grammar
//! at a version that conflicts with the pinned one, this parser:
//!
//! 1. **Masks** every byte outside `<script>…</script>` inner content
//!    with spaces (newlines preserved), so byte offsets, line and column
//!    numbers stay identical to the original file.
//! 2. **Delegates** the masked source to the mature [`TypeScriptParser`],
//!    which yields the script's imports, functions, consts and types with
//!    correct file coordinates — no offset arithmetic.
//! 3. Wraps the file in a single **Component** entity (named after the
//!    file stem) that *contains* those script members.
//! 4. **Scans the template markup** for child-component ("widget") tags
//!    like `<Sidebar/>` and emits an `Instantiates` edge per widget from
//!    the Component. Those edges resolve to the target component by name
//!    (`Sidebar` → `Sidebar.svelte`'s Component), which is how the widget
//!    map inside a file is recovered.
//!
//! Not covered (deliberate, for now): `<style>` rules, slot topology, and
//! dynamic `<svelte:component this={X}>` widgets whose identity is a
//! runtime value rather than a literal tag.

use super::language_parser::{LanguageParser, ParseResult};
use super::TypeScriptParser;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind, Span};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct SvelteParser;

impl SvelteParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SvelteParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for SvelteParser {
    fn language(&self) -> Language {
        Language::Svelte
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let script_ranges = element_inner_ranges(content, b"script");

        // 1 + 2: mask everything but the script blocks, then let the
        // TypeScript extractor do the heavy lifting. Spans come back in
        // real-file coordinates because masking preserves byte layout.
        let masked = mask_outside(content, &script_ranges);
        let mut result = TypeScriptParser::new().parse(path, &masked)?;

        // Every name the script puts in scope, collected before the
        // component entity joins them. The template can only mean one of
        // these (SV-002), which is what keeps the markup scan from inventing
        // names: an identifier the script never declared or imported is not
        // a reference we can resolve, so we don't emit one.
        let scope = script_scope(&result);

        // 3: the component anchor. Every script member reparents under it
        // so the graph shows "component contains its logic", and template
        // widget edges have a stable source to hang off.
        let comp = component_entity(path, content);
        let comp_id = comp.id.clone();
        let comp_name = comp.name.clone();
        for entity in result.entities.iter_mut() {
            if entity.parent_id.is_none() {
                entity.parent_id = Some(comp_id.clone());
                entity.qualified_name = format!("{}.{}", comp_name, entity.qualified_name);
            }
        }
        result.entities.insert(0, comp);

        // 4: widgets used in the template. Skip the script and style
        // block interiors so a `<Foo` in a string or CSS can't masquerade
        // as a widget.
        let mut skip = script_ranges;
        skip.extend(element_inner_ranges(content, b"style"));
        for (widget, count) in scan_widgets(content, &skip) {
            let rel = Relationship::new(comp_id.clone(), widget, RelationshipKind::Instantiates)
                .with_label("renders")
                .with_weight(count);
            result.add_relationship(rel);
        }

        // 5: what the markup *uses*. A helper called only from an
        // interpolation had no edge at all before SV-002, so `impact`
        // reported zero dependents for it — indistinguishable from dead
        // code. The edge hangs off the Component rather than the member that
        // wrote it: recovering that would mean parsing the expressions
        // themselves, which is the option this deliberately isn't.
        for (name, count) in scan_template_references(content, &skip, &scope) {
            let rel = Relationship::new(comp_id.clone(), name, RelationshipKind::Interpolates)
                .with_label("markup")
                .with_weight(count);
            result.add_relationship(rel);
        }

        Ok(result)
    }
}

/// Build the Component entity that represents the whole `.svelte` file,
/// named after the file stem (Svelte's own convention: `Sidebar.svelte`
/// is used as `<Sidebar/>`).
fn component_entity(path: &Path, content: &str) -> CodeEntity {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Component")
        .to_string();
    let line_count = content.lines().count().max(1);
    let span = Span::from_positions(0, 0, line_count.saturating_sub(1), 0);
    let mut entity = CodeEntity::new(name, EntityKind::Component, path, span);
    entity.tags.insert("svelte".to_string());
    entity
}

/// Inner byte ranges (content between the open tag's `>` and the closing
/// `</tag>`) of every `<tag …>…</tag>` element. Case-insensitive on the
/// tag name; operates on raw bytes so multi-byte template text never
/// shifts the offsets.
fn element_inner_ranges(content: &str, tag: &[u8]) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let open_close = b">";
    let mut ranges = Vec::new();
    let mut pos = 0;
    // The pattern we look for is `<script` / `<style` (no trailing byte
    // constraint — attributes may follow before the `>`).
    let mut open_pat = Vec::with_capacity(tag.len() + 1);
    open_pat.push(b'<');
    open_pat.extend_from_slice(tag);
    let mut close_pat = Vec::with_capacity(tag.len() + 3);
    close_pat.extend_from_slice(b"</");
    close_pat.extend_from_slice(tag);
    close_pat.push(b'>');

    while let Some(tag_start) = find_ci(bytes, &open_pat, pos) {
        let after_tag = tag_start + open_pat.len();
        let Some(gt) = find_ci(bytes, open_close, after_tag) else {
            break;
        };
        let inner_start = gt + 1;
        let Some(close_start) = find_ci(bytes, &close_pat, inner_start) else {
            break;
        };
        ranges.push((inner_start, close_start));
        pos = close_start + close_pat.len();
    }
    ranges
}

/// Replace every byte outside `keep` with a space, preserving newlines so
/// line/column/offset coordinates survive the round-trip. The kept ranges
/// are contiguous slices of the original (script interiors), so the result
/// is always valid UTF-8.
fn mask_outside(content: &str, keep: &[(usize, usize)]) -> String {
    let bytes = content.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    for &(s, e) in keep {
        out[s..e].copy_from_slice(&bytes[s..e]);
    }
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out[i] = b'\n';
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| content.to_string())
}

/// Find widget tags — `<Name` where `Name` starts with an uppercase
/// ASCII letter (Svelte's rule for "this is a component, not an HTML
/// element"). Returns a deterministic name → occurrence-count map. Byte
/// ranges in `skip` (script/style interiors) are ignored.
fn scan_widgets(content: &str, skip: &[(usize, usize)]) -> BTreeMap<String, u32> {
    let bytes = content.as_bytes();
    let mut widgets: BTreeMap<String, u32> = BTreeMap::new();
    let mut i = 0;
    while i < bytes.len() {
        if in_ranges(i, skip) {
            i += 1;
            continue;
        }
        if bytes[i] == b'<' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_uppercase() {
            let mut j = i + 1;
            while j < bytes.len() && is_tag_name_byte(bytes[j]) {
                j += 1;
            }
            let name = &content[i + 1..j];
            *widgets.entry(name.to_string()).or_insert(0) += 1;
            i = j;
        } else {
            i += 1;
        }
    }
    widgets
}

/// The names the `<script>` block imports, including renamed ones.
///
/// Imports only, not the script's own declarations. A local `const` used in
/// the markup is already tied to the component by the `Contains` edge, and
/// re-stating it as a dependency would fill every component's fan-out with
/// its own members. What was missing — and what `impact` needs — is the
/// edge that leaves the file.
fn script_scope(result: &ParseResult) -> BTreeSet<String> {
    let mut scope = BTreeSet::new();
    for import in &result.imports {
        scope.extend(import.items.iter().cloned());
        if let Some(alias) = &import.alias {
            scope.insert(alias.clone());
        }
    }
    scope
}

/// Names from `scope` that the template actually mentions, with occurrence
/// counts (SV-002).
///
/// Only `{…}` regions are read. Svelte puts every expression in braces —
/// text interpolation and attribute values alike — so this is where the
/// references are, and everything outside stays what it is: prose. Scanning
/// raw markup instead would turn the word "Card" in a paragraph into a
/// dependency.
///
/// Block syntax (`{#each}`, `{:else}`, `{@html}`) needs no special case:
/// `#each` and `:else` are not identifiers, and the names *inside* a block
/// tag — `{#each items as item}` — are real references when the script
/// declared them and correctly ignored when, like `item`, it did not.
fn scan_template_references(
    content: &str,
    skip: &[(usize, usize)],
    scope: &BTreeSet<String>,
) -> BTreeMap<String, u32> {
    let mut hits: BTreeMap<String, u32> = BTreeMap::new();
    if scope.is_empty() {
        return hits;
    }
    for (start, end) in expression_ranges(content, skip) {
        collect_identifiers(&content[start..end], scope, &mut hits);
    }
    hits
}

/// Inner byte ranges of the template's `{…}` expressions. Tracks brace depth
/// so an object literal doesn't end the expression early, and skips quoted
/// spans so a `{` inside a string doesn't open one.
fn expression_ranges(content: &str, skip: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' || in_ranges(i, skip) {
            i += 1;
            continue;
        }
        let start = i + 1;
        match closing_brace(bytes, start) {
            Some(end) => {
                ranges.push((start, end));
                i = end + 1;
            }
            // Never closed — a stray brace in prose, or an attribute value
            // containing one. Resume just past it, or one unbalanced
            // character would swallow every expression after it.
            None => i = start,
        }
    }
    ranges
}

/// Index of the `}` that closes an expression whose body starts at `from`,
/// or `None` if it never closes. Nested braces raise the depth; quoted spans
/// are opaque, so a brace inside a string literal is just a character.
fn closing_brace(bytes: &[u8], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                i = end_of_string(bytes, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Index just past the string literal opening at `at`; the end of the input
/// if it is unterminated.
fn end_of_string(bytes: &[u8], at: usize) -> usize {
    let quote = bytes[at];
    let mut i = at + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Count identifiers in `src` that name something in `scope`.
///
/// Two exclusions, both about not counting a name that isn't the one it
/// looks like: quoted string contents (`{"Card"}` is text, not a reference)
/// and member positions (`props.Card` is a field of something else).
fn collect_identifiers(src: &str, scope: &BTreeSet<String>, hits: &mut BTreeMap<String, u32>) {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b'\'' | b'"' | b'`') {
            i = end_of_string(bytes, i);
            continue;
        }
        if !is_ident_start(b) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_ident_byte(bytes[i]) {
            i += 1;
        }
        let is_member = start > 0 && bytes[start - 1] == b'.';
        let word = &src[start..i];
        if !is_member && scope.contains(word) {
            *hits.entry(word.to_string()).or_insert(0) += 1;
        }
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_tag_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn in_ranges(i: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|&(s, e)| i >= s && i < e)
}

/// ASCII case-insensitive substring search starting at `from`.
fn find_ci(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    'outer: for start in from..=haystack.len() - needle.len() {
        for (k, &nb) in needle.iter().enumerate() {
            if !haystack[start + k].eq_ignore_ascii_case(&nb) {
                continue 'outer;
            }
        }
        return Some(start);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(src: &str) -> ParseResult {
        SvelteParser::new()
            .parse(&PathBuf::from("Widget.svelte"), src)
            .unwrap()
    }

    #[test]
    fn component_entity_named_after_file_stem() {
        let res = parse("<script lang=\"ts\">const x = 1;</script>\n<div/>");
        let comp = res
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::Component)
            .expect("component entity");
        assert_eq!(comp.name, "Widget");
    }

    #[test]
    fn script_members_are_extracted_and_reparented() {
        let src = "<script lang=\"ts\">\nfunction greet() { return 1; }\n</script>\n<p>hi</p>";
        let res = parse(src);
        let f = res
            .entities
            .iter()
            .find(|e| e.name == "greet")
            .expect("greet function");
        assert_eq!(f.kind, EntityKind::Function);
        assert!(f.parent_id.as_deref().unwrap().contains("Widget"));
        // Reparented under the component in the qualified name.
        assert_eq!(f.qualified_name, "Widget.greet");
    }

    #[test]
    fn template_widgets_become_instantiates_edges() {
        let src = "<script lang=\"ts\">import Foo from './Foo.svelte';</script>\n\
                   <div>\n  <Foo />\n  <Bar>\n    <Foo></Foo>\n  </Bar>\n</div>";
        let res = parse(src);
        let widgets: Vec<_> = res
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Instantiates)
            .map(|r| (r.target_id.as_str(), r.weight))
            .collect();
        assert!(widgets.contains(&("Foo", 2)), "Foo used twice: {widgets:?}");
        assert!(widgets.contains(&("Bar", 1)), "Bar used once: {widgets:?}");
    }

    #[test]
    fn html_elements_are_not_widgets() {
        let res = parse("<div><span>text</span><button>x</button></div>");
        assert!(res
            .relationships
            .iter()
            .all(|r| r.kind != RelationshipKind::Instantiates));
    }

    #[test]
    fn tags_inside_script_and_style_are_ignored() {
        let src = "<script lang=\"ts\">const s = \"<Nope/>\";</script>\n\
                   <Real/>\n<style>.x { color: red; }</style>";
        let res = parse(src);
        let names: Vec<_> = res
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Instantiates)
            .map(|r| r.target_id.clone())
            .collect();
        assert_eq!(names, vec!["Real".to_string()]);
    }

    #[test]
    fn script_block_types_gain_uses_type_edges() {
        // The TS parser's UsesType post-pass (TS-001) applies to Svelte
        // script blocks for free, since they route through it.
        let src = "<script lang=\"ts\">\nfunction load(g: Graph): Config { return g.cfg; }\n</script>\n<p>hi</p>";
        let res = parse(src);
        let targets: Vec<_> = res
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::UsesType)
            .map(|r| r.target_id.as_str())
            .collect();
        assert!(targets.contains(&"Graph"), "{targets:?}");
        assert!(targets.contains(&"Config"), "{targets:?}");
    }

    /// Targets of the markup edges (SV-002), in scan order.
    fn markup_refs(res: &ParseResult) -> Vec<(String, u32)> {
        res.relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Interpolates)
            .map(|r| (r.target_id.clone(), r.weight))
            .collect()
    }

    #[test]
    fn an_imported_name_used_in_markup_gains_an_edge() {
        // The field-report case: two call sites, and before SV-002 the
        // helper had zero dependents in the graph.
        let src = "<script lang=\"ts\">\n\
                   import { definiteArticle } from './lib/article';\n\
                   export let word: string;\n\
                   </script>\n\
                   <Card label={definiteArticle(word)} />\n\
                   <p>{definiteArticle(word)}</p>";
        assert_eq!(markup_refs(&parse(src)), vec![("definiteArticle".into(), 2)]);
    }

    #[test]
    fn prose_that_happens_to_spell_the_name_is_not_a_reference() {
        // Why the scan is confined to `{…}`: markup text is words, not code.
        let src = "<script lang=\"ts\">import { definiteArticle } from './a';</script>\n\
                   <p>definiteArticle is the helper</p>";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn a_quoted_string_inside_an_expression_is_not_a_reference() {
        let src = "<script lang=\"ts\">import { definiteArticle } from './a';</script>\n\
                   <p>{\"definiteArticle\"}</p>";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn a_member_access_is_not_a_reference_to_the_member_name() {
        let src = "<script lang=\"ts\">import { fmt } from './a';</script>\n\
                   <p>{props.fmt}</p>";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn nested_braces_do_not_end_the_expression_early() {
        let src = "<script lang=\"ts\">import { fmt, tag } from './a';</script>\n\
                   <p>{fmt({ style: 'long' })} {tag}</p>";
        let refs = markup_refs(&parse(src));
        assert!(refs.contains(&("fmt".into(), 1)), "{refs:?}");
        assert!(refs.contains(&("tag".into(), 1)), "{refs:?}");
    }

    #[test]
    fn an_unbalanced_brace_does_not_swallow_later_expressions() {
        // A `{` that never closes is a scanning hazard, not a reference:
        // resuming at the wrong offset would silently drop every expression
        // in the rest of the file.
        let src = "<script lang=\"ts\">import { fmt } from './a';</script>\n\
                   <p title=\"a { brace\">{fmt}</p>";
        assert_eq!(markup_refs(&parse(src)), vec![("fmt".into(), 1)]);
    }

    #[test]
    fn block_syntax_reaches_the_names_it_uses_and_invents_none() {
        // `#each` and `:else` are not identifiers; `item` is a block-local
        // the script never declared; `items` is the real reference.
        let src = "<script lang=\"ts\">import { items } from './a';</script>\n\
                   {#each items as item}<span>{item}</span>{:else}<i>none</i>{/each}";
        assert_eq!(markup_refs(&parse(src)), vec![("items".into(), 1)]);
    }

    #[test]
    fn a_local_declaration_used_in_markup_stays_a_containment() {
        // Contains already ties a component to its own members; restating
        // it as a dependency would bury the edges that leave the file.
        let src = "<script lang=\"ts\">let word = 'x';</script>\n<p>{word}</p>";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn a_component_without_template_expressions_gains_nothing() {
        let src = "<script lang=\"ts\">import Foo from './Foo.svelte';</script>\n<Foo />";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn style_and_script_interiors_are_not_scanned() {
        let src = "<script lang=\"ts\">import { red } from './a';\nconst c = { red };</script>\n\
                   <style>.x { color: red; }</style>";
        assert!(markup_refs(&parse(src)).is_empty());
    }

    #[test]
    fn masking_preserves_line_numbers() {
        // The function sits on line 3 (0-indexed line 2); its span must
        // reflect the real file position, not a script-relative one.
        let src = "<div>hi</div>\n<script lang=\"ts\">\nfunction f() {}\n</script>";
        let res = parse(src);
        let f = res.entities.iter().find(|e| e.name == "f").unwrap();
        assert_eq!(f.span.start.line, 2);
    }
}
