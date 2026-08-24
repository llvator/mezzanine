//! Reducing written-out Rust type text to the bare name the entity index is
//! keyed by.
//!
//! Pure string work — it closes over no tree, no cursor, no parse context —
//! and two unrelated callers need it: the Rust parser's inference walk, and
//! the analyzer's cross-file field index (AN-012), which normalises the same
//! declared-type text read off entities rather than off the tree.
//!
//! It sits here, beside [`super::rust`] rather than inside it, for that
//! reason. It grew in [`super::rust::bodies::inference`], but that module
//! walks a tree and promises to report only what a body yields, and the
//! analyzer wants neither the tree nor the walk. Left inside the parser
//! folder it was a second way in, so the analyzer reached past the parser's
//! door into its interior for something the parser does not own. Being pure,
//! it could move out; anything that closed over the walk could not have.
//! Now `src/parser/rust` has exactly one entrance, [`super::RustParser`].

/// Strip angle-bracketed generic arguments from a path string while preserving
/// `::` separators. `Foo<T>::bar<U>` → `Foo::bar`, `Self::new` → `Self::new`.
pub(super) fn strip_generics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth: i32 = 0;
    for ch in s.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Reduce a written-out type to the bare name the entity index is keyed by.
///
/// `&mut LspClient` → `LspClient`, `Vec<Foo>` → `Vec`,
/// `crate::parser::RustParser` → `RustParser`. Returns `None` for anything
/// that doesn't resolve to a type-shaped name (`&str`, `usize`, a bare
/// lifetime) — the uppercase test is the same one the inference walk uses to
/// avoid binding a variable to a primitive.
///
/// Visible to the crate because the analyzer's cross-file field index
/// (AN-012) normalises the same declared-type text, read off entities
/// rather than off the tree; two normalisers would drift.
pub(crate) fn base_type_name(text: &str) -> Option<String> {
    let stripped = strip_generics(text);
    let mut rest = stripped.trim();
    loop {
        let trimmed = rest.trim_start();
        if let Some(tail) = trimmed.strip_prefix('&') {
            rest = tail;
        } else if let Some(tail) = trimmed.strip_prefix("mut ") {
            rest = tail;
        } else if let Some(tail) = trimmed.strip_prefix("dyn ") {
            rest = tail;
        } else if let Some(tail) = trimmed.strip_prefix("impl ") {
            rest = tail;
        } else if trimmed.starts_with('\'') {
            // A lifetime (`&'a mut Foo`); drop it and keep unwrapping.
            rest = trimmed.trim_start_matches(|c: char| c != ' ');
        } else {
            rest = trimmed;
            break;
        }
    }
    let last = rest.rsplit("::").next()?.trim();
    let name: String = last
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    name.chars().next()?.is_uppercase().then_some(name)
}
