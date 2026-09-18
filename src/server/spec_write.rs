//! Turning "document this file" into `.elv` source (SRV-022).
//!
//! Everything here is pure text: given a file's current contents and what the
//! reader typed, produce the contents it should have. The HTTP half lives in
//! [`super::spec_handler`], which resolves the parent out of the graph and
//! does the reading and writing. Splitting them is what lets the awkward part
//! — editing *inside* an existing definition somebody wrote by hand — be
//! asserted against real spec shapes rather than clicked at.
//!
//! # Why a stub is a finished thing
//!
//! `guide/elevator-language.md` opens on "sketch first, deepen where you
//! work": a body-less `f name` is valid and complete, and an orphan Feature
//! is a `--check` *hint*, not an error. So this module never invents prose to
//! fill a body it thinks looks empty. It writes exactly what the reader
//! supplied and stops, because the state it produces is one the language
//! already calls finished — the alternative being a plausible sentence nobody
//! wrote, in a file whose whole value is that every sentence was.
//!
//! # The one rule about editing what you did not write
//!
//! Attaching a Feature to a Category means inserting a child line into that
//! Category's body, which means editing a file whose author is not us. The
//! anchor for that edit is the parent's span, and a span is a claim about the
//! text *as the analyzer last read it* — the reader may have edited the file
//! since. So [`insert_child_ref`] verifies the block before it touches it: the
//! text at those lines has to still open with the keyword and name the graph
//! says it does. When it doesn't, the caller is told, and the definition is
//! appended unparented rather than spliced somewhere on a stale guess. An
//! orphan hint is a to-deepen note; a child line in the wrong Category is a
//! wrong map, and the spec's whole worth is that it isn't one.

/// The Elevator kinds this route can create.
///
/// Not UI Pages: `ui a.b.c` is a route name, not something a code path is
/// attached to, and every entry point here starts from a file, a folder or an
/// entity. Not Extensions: the top tier exists to bundle Categories for
/// separable products, which is a decision about the whole spec rather than
/// about the code under the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SpecKind {
    Category,
    Feature,
    Functionality,
    Concept,
}

impl SpecKind {
    /// Parse the wire value. Both the keyword and the spelled-out name are
    /// accepted, since the UI sends one and a curl user will reach for the
    /// other.
    pub(super) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "c" | "category" => Some(Self::Category),
            "f" | "feature" => Some(Self::Feature),
            "fu" | "functionality" => Some(Self::Functionality),
            "concept" => Some(Self::Concept),
            _ => None,
        }
    }

    /// The keyword this kind is written with, which is also the segment its
    /// entity id uses — `emit.rs::id_segment` keeps those the same.
    pub(super) fn keyword(self) -> &'static str {
        match self {
            Self::Category => "c",
            Self::Feature => "f",
            Self::Functionality => "fu",
            Self::Concept => "concept",
        }
    }

    /// What may contain this kind, or `None` for a kind that sits at the top.
    ///
    /// A Functionality's parent is not optional the way a Feature's is: its
    /// name is *qualified by* the Feature (`fu f.<feature>.<verb>`), so there
    /// is no such thing as an unparented one to write.
    pub(super) fn parent_kind(self) -> Option<SpecKind> {
        match self {
            Self::Feature => Some(Self::Category),
            Self::Functionality => Some(Self::Feature),
            Self::Category | Self::Concept => None,
        }
    }

    /// Whether a parent must be named for this kind to be writable at all.
    pub(super) fn requires_parent(self) -> bool {
        self == Self::Functionality
    }
}

/// What the reader asked for, validated and resolved.
pub(super) struct Draft {
    pub kind: SpecKind,
    /// Leaf name — `diff_streaming`, never `f.diff_streaming`.
    pub name: String,
    /// The parent's *qualified name* as the graph holds it (`visual_scopes`),
    /// absent when the entity stands alone.
    pub parent_qualname: Option<String>,
    pub description: Option<String>,
    /// Repo-relative paths, in the order the reader ticked them.
    pub code_refs: Vec<String>,
}

impl Draft {
    /// The qualified name the analyzer will give this entity, which is what
    /// its id is built from.
    pub(super) fn qualname(&self) -> String {
        match (self.kind, self.parent_qualname.as_deref()) {
            (SpecKind::Functionality, Some(parent)) => format!("{parent}.{}", self.name),
            _ => self.name.clone(),
        }
    }

    /// The id `elevator/emit.rs` will mint for it. Used to refuse a name that
    /// is already taken before anything is written.
    pub(super) fn entity_id(&self) -> String {
        format!("elevator::{}.{}", self.kind.keyword(), self.qualname())
    }

    /// How the definition heads its own line. A Functionality is written in
    /// the explicit `fu f.<feature>.<verb>` form this repo's spec uses; the
    /// parser normalizes the `f.` away, so the shorter form would parse to
    /// the same id and read as less obviously qualified.
    fn header(&self) -> String {
        match (self.kind, self.parent_qualname.as_deref()) {
            (SpecKind::Functionality, Some(parent)) => format!("fu f.{parent}.{}", self.name),
            _ => format!("{} {}", self.kind.keyword(), self.name),
        }
    }

    /// The line that goes *inside* the parent's body to declare containment.
    ///
    /// Bare, not qualified: inside `f X` a bare `fu Y` resolves to
    /// `elevator::fu.X.Y`, which is the id [`Self::entity_id`] promises.
    pub(super) fn child_ref(&self) -> String {
        format!("{} {}", self.kind.keyword(), self.name)
    }
}

/// Names the Elevator lexer will read back as one identifier.
///
/// The lexer's rule is `[A-Za-z_][A-Za-z0-9_]*` (`lexer.rs`). A name with a
/// dot in it is the mistake worth naming separately: it looks like the
/// qualified form and would silently define something else.
pub(super) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a name is required".to_string());
    }
    if name.contains('.') {
        return Err(format!(
            "`{name}` is qualified — give the leaf name only and name the parent separately"
        ));
    }
    let mut chars = name.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !head_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "`{name}` is not an identifier — letters, digits and underscore only, not starting \
             with a digit"
        ));
    }
    Ok(())
}

/// A description as an `.elv` string literal.
///
/// Newlines are folded to spaces rather than rejected: a textarea produces
/// them by accident and the language has no multi-line string, so folding is
/// what the author meant.
///
/// **A double quote is replaced, not escaped, because the language has no
/// escapes.** `lexer.rs::string` ends the literal at the first `"` it meets
/// and treats a backslash as an ordinary character — there is no sequence
/// that puts a `"` inside a description. Writing `\"` produces a file that
/// stops parsing mid-sentence, which is how this was found: the first
/// hand-written spec entry for this very feature quoted `cr: "ui/"` in its
/// own prose and took `elevator --check` down with four parse errors.
///
/// A single quote is the substitution because it is what an author writing
/// the line by hand would have reached for, and because the alternative —
/// refusing the description — throws away the sentence over a punctuation
/// mark. Backslashes are left exactly as typed: nothing consumes them, so
/// doubling one would put a character in the file the reader did not write.
fn quote(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .map(|c| if c == '"' { '\'' } else { c })
        .collect();
    format!(
        "\"{}\"",
        folded.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// Render the top-level definition, with no trailing newline.
///
/// A draft carrying neither a description nor a code ref renders body-less —
/// `f name` on its own line. `f name {}` would be equally valid and reads as
/// an empty room somebody furnished; the bare form reads as the sketch it is.
pub(super) fn render_definition(draft: &Draft) -> String {
    let mut body: Vec<String> = Vec::new();
    if let Some(text) = draft.description.as_deref().map(str::trim) {
        if !text.is_empty() {
            body.push(format!("    d: {}", quote(text)));
        }
    }
    let refs: Vec<String> = draft
        .code_refs
        .iter()
        .map(|p| format!("\"{}\"", p.replace('\\', "/")))
        .collect();
    if !refs.is_empty() {
        body.push(format!("    cr: {}", refs.join(", ")));
    }
    if body.is_empty() {
        return draft.header();
    }
    format!("{} {{\n{}\n}}", draft.header(), body.join("\n"))
}

/// Append a rendered definition to a file's text, returning the new text and
/// the 1-based line the definition starts on.
///
/// A blank line before it, always: `.elv` files separate top-level
/// definitions that way throughout, and a definition welded to the previous
/// one is the diff nobody wants to read.
pub(super) fn append_definition(source: &str, block: &str) -> (String, u32) {
    let mut text = source.trim_end().to_string();
    if text.is_empty() {
        let next = format!("{block}\n");
        return (next, 1);
    }
    let line = text.lines().count() as u32 + 2;
    text.push_str("\n\n");
    text.push_str(block);
    text.push('\n');
    (text, line)
}

/// Why a child line could not be inserted into a parent's body.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ParentEdit {
    /// The line was inserted; here is the new text.
    Inserted(String),
    /// The body already declares this child. Not an error — re-running the
    /// same create is how a reader recovers from a half-failed one.
    AlreadyPresent,
    /// The text at the parent's span is not the parent. The analysis is
    /// behind the file, and guessing where the body is would be a guess about
    /// somebody else's prose.
    Moved,
}

/// Insert `child` into the parent definition occupying `lines` (1-based,
/// inclusive) of `source`.
///
/// The insertion point is immediately before the block's final `}`, which
/// puts a new child at the end of an existing child list — the order the spec
/// is read in is the order it was written in, and sorting somebody's list on
/// their behalf is a diff they did not ask for. A parent with no body at all
/// gains one.
pub(super) fn insert_child_ref(
    source: &str,
    lines: (u32, u32),
    expected_header: (&str, &str),
    child: &str,
) -> ParentEdit {
    let all: Vec<&str> = source.split_inclusive('\n').collect();
    let (start, end) = (lines.0 as usize, lines.1 as usize);
    if start == 0 || end < start || end > all.len() {
        return ParentEdit::Moved;
    }
    let block: String = all[start - 1..end].concat();
    if !heads_definition(&block, expected_header) {
        return ParentEdit::Moved;
    }
    if declares_child(&block, child) {
        return ParentEdit::AlreadyPresent;
    }
    let indent = body_indent(&block);
    let Some(edited) = splice(&block, &format!("{indent}{child}\n")) else {
        return ParentEdit::Moved;
    };
    let mut text = all[..start - 1].concat();
    text.push_str(&edited);
    text.push_str(&all[end..].concat());
    ParentEdit::Inserted(text)
}

/// Does this text open with the definition the graph says lives here?
///
/// Keyword and name both, because a file edited since the last analysis
/// shifts every span below the edit and the line that lands under a stale one
/// is very often *another definition of the same kind*.
fn heads_definition(block: &str, expected: (&str, &str)) -> bool {
    let (keyword, name) = expected;
    let head = block.trim_start();
    let Some(rest) = head.strip_prefix(keyword) else {
        return false;
    };
    let rest = rest.trim_start();
    // `f.` for the explicit Functionality form, which names the same entity.
    let rest = rest.strip_prefix("f.").unwrap_or(rest);
    rest.strip_prefix(name)
        .is_some_and(|tail| !tail.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

/// Is `child` already one of this body's child references?
///
/// Compared token-wise rather than by substring: `f diff` must not match
/// `f diff_streaming`, and a line that merely mentions the name inside a `d:`
/// string is not a declaration.
fn declares_child(block: &str, child: &str) -> bool {
    block.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let head = (fields.next(), fields.next(), fields.next());
        let mut wanted = child.split_whitespace();
        head == (wanted.next(), wanted.next(), None)
    })
}

/// The indentation the parent's own children use, or four spaces when it has
/// none to copy.
fn body_indent(block: &str) -> String {
    block
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty() && !line.trim().starts_with('}'))
        .map(|line| line[..line.len() - line.trim_start().len()].to_string())
        .unwrap_or_else(|| "    ".to_string())
}

/// Put `line` inside the block, opening a body if there isn't one.
///
/// Returns `None` for a block that opens a body it never closes, which is a
/// file that does not parse — not something to append to.
fn splice(block: &str, line: &str) -> Option<String> {
    let trailing = &block[block.trim_end().len()..];
    let trimmed = block.trim_end();
    if !trimmed.contains('{') {
        // `c server` with no body at all.
        return Some(format!("{trimmed} {{\n{line}}}{trailing}"));
    }
    let close = trimmed.rfind('}')?;
    let (before, after) = trimmed.split_at(close);
    let separator = if before.ends_with('\n') { "" } else { "\n" };
    Some(format!("{before}{separator}{line}{after}{trailing}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: SpecKind, name: &str) -> Draft {
        Draft {
            kind,
            name: name.to_string(),
            parent_qualname: None,
            description: None,
            code_refs: Vec::new(),
        }
    }

    #[test]
    fn a_bare_sketch_renders_body_less() {
        let rendered = render_definition(&draft(SpecKind::Feature, "diff_streaming"));
        assert_eq!(rendered, "f diff_streaming");
    }

    #[test]
    fn a_description_and_refs_render_a_body() {
        let mut d = draft(SpecKind::Feature, "diff_streaming");
        d.description = Some("  Streams a diff.  ".to_string());
        d.code_refs = vec!["src/server/diff_handler.rs".to_string()];
        assert_eq!(
            render_definition(&d),
            "f diff_streaming {\n    d: \"Streams a diff.\"\n    cr: \"src/server/diff_handler.rs\"\n}"
        );
    }

    #[test]
    fn a_newline_in_the_description_folds_rather_than_breaking_the_file() {
        let mut d = draft(SpecKind::Feature, "x");
        d.description = Some("one\ntwo\t three".to_string());
        assert!(render_definition(&d).contains("d: \"one two three\""));
    }

    /// The language has no string escapes, so a `"` in the prose has to be
    /// replaced rather than escaped — `\"` would end the literal and leave
    /// the rest of the sentence being read as syntax.
    #[test]
    fn a_quote_in_the_prose_is_replaced_because_the_language_cannot_escape_one() {
        let mut d = draft(SpecKind::Feature, "x");
        d.description = Some(r#"the "cr:" field, or C:\ paths"#.to_string());
        let rendered = render_definition(&d);
        assert!(
            rendered.contains(r#"d: "the 'cr:' field, or C:\ paths""#),
            "{rendered}"
        );
        // Exactly two quote characters in the whole definition: the ones
        // opening and closing the literal.
        assert_eq!(rendered.matches('"').count(), 2, "{rendered}");
    }

    /// The guarantee the replacement exists for, checked against the real
    /// lexer rather than against a rule restated here.
    #[test]
    fn a_rendered_definition_parses_back_whatever_the_prose_contained() {
        use crate::parser::language_parser::LanguageParser;
        let mut d = draft(SpecKind::Feature, "spec_authoring");
        d.description = Some("Quotes a cr: \"ui/\" path, a \\ and a \" alone".to_string());
        d.code_refs = vec!["src/server/spec_write.rs".to_string()];
        let (text, _) = append_definition("", &render_definition(&d));

        let result = crate::parser::ElevatorParser
            .parse(std::path::Path::new("t.elv"), &text)
            .expect("the written file must parse");
        assert!(
            result.warnings.is_empty(),
            "written spec did not parse cleanly: {:?}\n{text}",
            result.warnings
        );
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].id, "elevator::f.spec_authoring");
    }

    #[test]
    fn a_functionality_is_qualified_by_its_feature() {
        let mut d = draft(SpecKind::Functionality, "shape");
        d.parent_qualname = Some("visual_scopes".to_string());
        assert_eq!(d.qualname(), "visual_scopes.shape");
        assert_eq!(d.entity_id(), "elevator::fu.visual_scopes.shape");
        assert_eq!(render_definition(&d), "fu f.visual_scopes.shape");
        // Bare inside the parent's body, where it resolves to the same id.
        assert_eq!(d.child_ref(), "fu shape");
    }

    #[test]
    fn names_that_would_not_lex_are_refused() {
        assert!(validate_name("diff_streaming").is_ok());
        assert!(validate_name("_private2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("2fast").is_err());
        assert!(validate_name("has-dash").is_err());
        assert!(validate_name("has space").is_err());
        let qualified = validate_name("f.thing").unwrap_err();
        assert!(qualified.contains("qualified"), "{qualified}");
    }

    const CATEGORY: &str = "# A spec.\n\nc server {\n    d: \"The engine.\"\n    f engine_endpoint\n}\n\nf engine_endpoint {\n    d: \"Where.\"\n}\n";

    #[test]
    fn a_child_lands_at_the_end_of_the_list() {
        let edit = insert_child_ref(CATEGORY, (3, 6), ("c", "server"), "f diff_streaming");
        let ParentEdit::Inserted(text) = edit else {
            panic!("expected an insert, got {edit:?}");
        };
        assert_eq!(
            text,
            "# A spec.\n\nc server {\n    d: \"The engine.\"\n    f engine_endpoint\n    f diff_streaming\n}\n\nf engine_endpoint {\n    d: \"Where.\"\n}\n"
        );
    }

    #[test]
    fn the_indentation_is_the_parents_own() {
        let source = "c server {\n\tf a\n}\n";
        let ParentEdit::Inserted(text) = insert_child_ref(source, (1, 3), ("c", "server"), "f b")
        else {
            panic!("expected an insert");
        };
        assert_eq!(text, "c server {\n\tf a\n\tf b\n}\n");
    }

    #[test]
    fn a_parent_with_no_body_gains_one() {
        let source = "c server\n\nf other\n";
        let ParentEdit::Inserted(text) = insert_child_ref(source, (1, 1), ("c", "server"), "f a")
        else {
            panic!("expected an insert");
        };
        assert_eq!(text, "c server {\n    f a\n}\n\nf other\n");
    }

    #[test]
    fn an_empty_body_gains_a_line() {
        let source = "c server {}\n";
        let ParentEdit::Inserted(text) = insert_child_ref(source, (1, 1), ("c", "server"), "f a")
        else {
            panic!("expected an insert");
        };
        assert_eq!(text, "c server {\n    f a\n}\n");
    }

    #[test]
    fn a_child_already_declared_is_left_alone() {
        let edit = insert_child_ref(CATEGORY, (3, 6), ("c", "server"), "f engine_endpoint");
        assert_eq!(edit, ParentEdit::AlreadyPresent);
    }

    #[test]
    fn a_prefix_of_an_existing_child_is_not_already_present() {
        let edit = insert_child_ref(CATEGORY, (3, 6), ("c", "server"), "f engine");
        assert!(matches!(edit, ParentEdit::Inserted(_)), "{edit:?}");
    }

    #[test]
    fn a_span_pointing_at_the_wrong_definition_refuses_to_edit() {
        // Lines 8-10 are `f engine_endpoint`, not `c server`: what a file
        // edited since the last analysis looks like.
        let edit = insert_child_ref(CATEGORY, (8, 10), ("c", "server"), "f x");
        assert_eq!(edit, ParentEdit::Moved);
        // And a span running off the end of the file.
        assert_eq!(
            insert_child_ref(CATEGORY, (40, 44), ("c", "server"), "f x"),
            ParentEdit::Moved
        );
    }

    #[test]
    fn a_functionality_parent_is_recognised_in_its_explicit_form() {
        let source = "fu f.visual_scopes.shape {\n    d: \"x\"\n}\n";
        let edit = insert_child_ref(source, (1, 3), ("fu", "visual_scopes.shape"), "fu y");
        assert!(matches!(edit, ParentEdit::Inserted(_)), "{edit:?}");
    }

    #[test]
    fn an_appended_definition_is_separated_by_a_blank_line() {
        let (text, line) = append_definition("c server {\n    f a\n}\n", "f b");
        assert_eq!(text, "c server {\n    f a\n}\n\nf b\n");
        assert_eq!(line, 5);
        assert_eq!(text.lines().nth(line as usize - 1), Some("f b"));
    }

    #[test]
    fn an_empty_file_takes_the_definition_at_line_one() {
        let (text, line) = append_definition("", "c server");
        assert_eq!(text, "c server\n");
        assert_eq!(line, 1);
    }

    #[test]
    fn kinds_round_trip_through_the_wire_form() {
        assert_eq!(SpecKind::parse("f"), Some(SpecKind::Feature));
        assert_eq!(SpecKind::parse("Feature"), Some(SpecKind::Feature));
        assert_eq!(SpecKind::parse("fu"), Some(SpecKind::Functionality));
        assert_eq!(SpecKind::parse("concept"), Some(SpecKind::Concept));
        assert_eq!(SpecKind::parse("ui"), None);
        assert_eq!(SpecKind::Feature.parent_kind(), Some(SpecKind::Category));
        assert!(SpecKind::Functionality.requires_parent());
        assert!(!SpecKind::Feature.requires_parent());
    }
}
