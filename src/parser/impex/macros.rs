//! IM-004 — Impex macro definitions and substitution.
//!
//! Impex files use `$<name> = <value>` lines to define macros, then
//! reference them inside header / column / modifier values as
//! `$<name>`. The substitution is **definition-order sensitive**: a
//! reference appearing before its definition resolves to the literal
//! `$<name>` string and emits a parse warning. References inside macro
//! values themselves resolve recursively, with a depth cap to bottom
//! out cycles (`$a=$b\n$b=$a`) without panicking.
//!
//! Hybris reserves a handful of `$START_*` / `$END_*` directives that
//! aren't macros (block markers like `$START_USERRIGHTS`); the line
//! dispatcher in `mod.rs` filters them out before reaching this
//! module so the macro table only sees real `$x = value` definitions.

use std::collections::HashMap;

/// Maximum levels of macro-reference resolution before we declare a
/// cycle and bottom out. Sixteen is comfortably above any
/// real-world Hybris config (typical depth is 1-2) while still
/// guaranteeing termination.
const MAX_RECURSION: usize = 16;

#[derive(Debug, Default)]
pub(super) struct MacroTable {
    bindings: HashMap<String, String>,
    /// Names referenced before they were defined (or never defined).
    /// Surface as warnings on the file's `ParseResult` so the
    /// downstream user can see what went unresolved.
    pub(super) unresolved_refs: Vec<String>,
    pub(super) cycle_warnings: Vec<String>,
}

impl MacroTable {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Parse a `$<name> = <value>` definition. Returns `None` if the
    /// line doesn't match the macro shape — the dispatcher uses this
    /// as the test for "is this a macro definition vs a `$START_*`
    /// directive vs a header that happens to start with `$`."
    pub(super) fn parse_definition(line: &str) -> Option<(String, String)> {
        let rest = line.strip_prefix('$')?;
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let value = rest[eq + 1..].trim();
        if name.is_empty() || !is_macro_name(name) {
            return None;
        }
        Some((name.to_string(), value.to_string()))
    }

    /// Insert a binding. Subsequent references to `$<name>` resolve
    /// to `value` (with macro references in `value` itself substituted
    /// at lookup time, not at define time — matches Hybris's lazy
    /// evaluation).
    pub(super) fn define(&mut self, name: String, value: String) {
        self.bindings.insert(name, value);
    }

    /// Substitute every `$<name>` reference in `text` against the
    /// current bindings, recursively up to `MAX_RECURSION`. References
    /// to undefined names pass through verbatim and get logged.
    pub(super) fn substitute(&mut self, text: &str) -> String {
        let mut out = String::new();
        self.substitute_inner(text, 0, &mut out);
        out
    }

    fn substitute_inner(&mut self, text: &str, depth: usize, out: &mut String) {
        if depth >= MAX_RECURSION {
            self.cycle_warnings.push(format!(
                "macro substitution depth limit reached at fragment: {}",
                text
            ));
            out.push_str(text);
            return;
        }
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let name_start = i + 1;
                let mut name_end = name_start;
                while name_end < bytes.len() && is_macro_name_char(bytes[name_end]) {
                    name_end += 1;
                }
                if name_end > name_start {
                    let name = &text[name_start..name_end];
                    if let Some(value) = self.bindings.get(name).cloned() {
                        self.substitute_inner(&value, depth + 1, out);
                        i = name_end;
                        continue;
                    } else {
                        self.unresolved_refs.push(name.to_string());
                        // Pass through the raw `$name` so the user can
                        // see what was looked for in the rendered
                        // output / detail panel.
                        out.push_str(&text[i..name_end]);
                        i = name_end;
                        continue;
                    }
                }
            }
            out.push(text.as_bytes()[i] as char);
            i += 1;
        }
    }
}

/// True when `name` is a syntactically valid macro identifier — at
/// least one character, all of which are alphanumerics or `_`. The
/// same predicate gates the inline-reference walk in
/// `substitute_inner`.
fn is_macro_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(is_macro_name_char)
}

fn is_macro_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_definition() {
        let parsed = MacroTable::parse_definition("$catalog = apparel-deContentCatalog");
        assert_eq!(
            parsed,
            Some((
                "catalog".to_string(),
                "apparel-deContentCatalog".to_string()
            ))
        );
    }

    #[test]
    fn parse_definition_no_equals_returns_none() {
        assert_eq!(MacroTable::parse_definition("$START_USERRIGHTS"), None);
    }

    #[test]
    fn substitute_simple() {
        let mut t = MacroTable::new();
        t.define("cat".into(), "foo".into());
        assert_eq!(t.substitute("[catalog=$cat]"), "[catalog=foo]");
    }

    #[test]
    fn substitute_recursive() {
        let mut t = MacroTable::new();
        t.define("inner".into(), "value".into());
        t.define("outer".into(), "[k=$inner]".into());
        assert_eq!(t.substitute("$outer"), "[k=value]");
    }

    #[test]
    fn substitute_undefined_passes_through_and_warns() {
        let mut t = MacroTable::new();
        let result = t.substitute("[k=$missing]");
        assert_eq!(result, "[k=$missing]");
        assert_eq!(t.unresolved_refs, vec!["missing".to_string()]);
    }

    #[test]
    fn substitute_cycle_bottoms_out_with_warning() {
        let mut t = MacroTable::new();
        t.define("a".into(), "$b".into());
        t.define("b".into(), "$a".into());
        // Should not panic / loop forever.
        let _ = t.substitute("$a");
        assert!(
            !t.cycle_warnings.is_empty(),
            "cycle should produce a warning"
        );
    }
}
