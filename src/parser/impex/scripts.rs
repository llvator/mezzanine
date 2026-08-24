//! IM-003 — `#%` script-line dispatch.
//!
//! Three forms recognised by the line dispatcher:
//!
//! 1. **Statement.** `#% <expr>` — one Groovy/BeanShell statement.
//!    Handed to the Groovy parser via
//!    [`crate::parser::groovy::GroovyParser::parse_expression`] and
//!    the resulting relationships attribute to the file's module
//!    entity (or to the surrounding `if:` Branch).
//! 2. **Quoted statement.** `"#% ...;"` — an entire `#%` declaration
//!    wrapped in double quotes (per Impex's standard quoting). Strip
//!    the wrapping quotes, then treat as form 1.
//! 3. **Per-row hooks.** `#% beforeeach:` / `#% aftereach:` — the
//!    body after the colon is a Groovy expression run for each data
//!    row of the surrounding header. Treated as form 1 against the
//!    body, not the keyword.
//!
//! Conditional gating (`#% if: <cond>` / `#% endif:`) is handled in
//! `mod.rs` because it spans multiple lines; this module just parses
//! the cell shape.

/// What kind of `#%` line was encountered. Used by the dispatcher to
/// route to the right handler — statement bodies go through the
/// Groovy bridge, conditionals open / close a Branch arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScriptLine<'a> {
    /// `#% <body>` or `"#% <body>"` — Groovy expression to parse.
    Statement(&'a str),
    /// `#% if: <condition>` — opens a conditional Branch arm.
    IfStart(&'a str),
    /// `#% endif:` — closes the most recent conditional Branch.
    IfEnd,
    /// `#% beforeeach: <body>` / `#% aftereach: <body>` — per-row hook
    /// whose body is a Groovy expression. Same dispatch as
    /// `Statement` against the body.
    Hook { kind: HookKind, body: &'a str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HookKind {
    BeforeEach,
    AfterEach,
}

impl HookKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            HookKind::BeforeEach => "beforeeach",
            HookKind::AfterEach => "aftereach",
        }
    }
}

/// Try to parse a line as a `#%` script form. Returns `None` if the
/// line doesn't begin with `#%` (after optional surrounding double
/// quotes and trailing semicolon stripping).
///
/// Quoted form: Impex wraps multi-token script declarations in double
/// quotes so `;` inside the body doesn't get split as a cell
/// boundary. We strip the quotes and the optional trailing `;` before
/// dispatch — the body the Groovy parser sees should never carry the
/// Impex-level decoration.
pub(super) fn parse_script_line(line: &str) -> Option<ScriptLine<'_>> {
    let body = strip_decoration(line)?;

    if let Some(rest) = body.strip_prefix("if:") {
        return Some(ScriptLine::IfStart(rest.trim()));
    }
    if body == "endif:" || body.starts_with("endif:") {
        return Some(ScriptLine::IfEnd);
    }
    if let Some(rest) = body.strip_prefix("beforeeach:") {
        return Some(ScriptLine::Hook {
            kind: HookKind::BeforeEach,
            body: rest.trim(),
        });
    }
    if let Some(rest) = body.strip_prefix("aftereach:") {
        return Some(ScriptLine::Hook {
            kind: HookKind::AfterEach,
            body: rest.trim(),
        });
    }

    Some(ScriptLine::Statement(body))
}

/// Strip the `#%` prefix plus any wrapping double quotes / trailing
/// `;` so the returned slice is the bare statement body. Handles all
/// four shapes Impex uses in practice:
///
/// * `#% expr`
/// * `#% expr;`
/// * `"#% expr;"`
/// * `"#% expr"` (no trailing `;`, rare but seen)
fn strip_decoration(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = if let Some(stripped) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
    {
        stripped
    } else {
        trimmed
    };
    let body = inner.strip_prefix("#%")?.trim();
    let body = body.strip_suffix(';').map(str::trim).unwrap_or(body);
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_form() {
        let line = parse_script_line("#% doIt()").unwrap();
        assert_eq!(line, ScriptLine::Statement("doIt()"));
    }

    #[test]
    fn quoted_statement() {
        let line = parse_script_line("\"#% doIt();\"").unwrap();
        assert_eq!(line, ScriptLine::Statement("doIt()"));
    }

    #[test]
    fn if_start() {
        let line =
            parse_script_line("#% if: platformInfoService.isExtensionAvailable(\"catalogtest\")")
                .unwrap();
        assert_eq!(
            line,
            ScriptLine::IfStart("platformInfoService.isExtensionAvailable(\"catalogtest\")")
        );
    }

    #[test]
    fn if_end() {
        assert_eq!(parse_script_line("#% endif:"), Some(ScriptLine::IfEnd));
    }

    #[test]
    fn hooks() {
        let before = parse_script_line("#% beforeeach: prepareRow()").unwrap();
        assert_eq!(
            before,
            ScriptLine::Hook {
                kind: HookKind::BeforeEach,
                body: "prepareRow()"
            }
        );
        let after = parse_script_line("#% aftereach: cleanupRow()").unwrap();
        assert_eq!(
            after,
            ScriptLine::Hook {
                kind: HookKind::AfterEach,
                body: "cleanupRow()"
            }
        );
    }

    #[test]
    fn non_script_line() {
        assert_eq!(parse_script_line("# a comment"), None);
        assert_eq!(parse_script_line("INSERT_UPDATE Foo; code"), None);
    }
}
