//! Phase 1 — recursive descent over the token stream into [`DefStmt`]s.
//!
//! Nothing here resolves a reference; bodies are captured verbatim and
//! handed to [`super::emit`]. The parser's other job is diagnostics:
//! every warning carries a `line L column C`, because for a
//! hand-authored spec language the parser *is* the author's feedback
//! loop — a location-free "expected identifier inside body" is not
//! actionable in a 200-line file.

use super::ast::{qualify_child, ChildRef, DefKind, DefStmt, EdgeRef, Loc};
use super::lexer::{tokenize, SpannedToken, Token};

/// Every keyword that opens a definition, longest-prefix first so
/// `fu`/`concept` are matched before `f`/`c` when used as an id prefix.
const ALL_KINDS: [DefKind; 6] = [
    DefKind::Functionality,
    DefKind::Concept,
    DefKind::Extension,
    DefKind::Feature,
    DefKind::Category,
    DefKind::UiPage,
];

/// The kind a definition keyword opens, or `None` for a word that
/// isn't one.
fn kind_from_keyword(kw: &str) -> Option<DefKind> {
    ALL_KINDS.into_iter().find(|k| k.keyword() == kw)
}

/// True for kinds whose names must be flat (no dots). Only
/// Functionalities are qualified; UI pages accept any path.
fn requires_bare_name(kind: DefKind) -> bool {
    matches!(
        kind,
        DefKind::Extension | DefKind::Category | DefKind::Feature | DefKind::Concept
    )
}

/// Split a leading kind prefix (`fu.`, `concept.`, `e.`, `f.`, `c.`,
/// `ui.`) off a name, returning the kind it named and the remainder.
/// Lets authors write `f protocol` and `f f.protocol` interchangeably.
fn split_kind_prefix(qualname: &str) -> (Option<DefKind>, String) {
    for kind in ALL_KINDS {
        let prefix = format!("{}.", kind.keyword());
        if let Some(rest) = qualname.strip_prefix(&prefix) {
            return (Some(kind), rest.to_string());
        }
    }
    (None, qualname.to_string())
}

/// Drop a leading kind prefix, keeping only the name.
fn strip_kind_prefix(qualname: &str) -> String {
    split_kind_prefix(qualname).1
}

/// An import statement plus its source range, so the `ImportInfo` the
/// analyzer sees can point at the line the author wrote.
pub(super) struct ImportStmt {
    pub path: String,
    pub start: Loc,
    pub end: Loc,
}

#[derive(Default)]
pub(super) struct ParseOutput {
    pub defs: Vec<DefStmt>,
    pub imports: Vec<ImportStmt>,
    pub warnings: Vec<String>,
}

/// Parse a whole `.elv` source: tokenize it, then recover a flat list
/// of imports and definitions.
///
/// The token stream never leaves this module. Phase 1 is the only
/// reader of a token, so it owns producing one — a caller that had to
/// lex first would also have to remember to drain the lexer's
/// diagnostics, and forgetting is silent. They arrive here in
/// `warnings` alongside the parser's own, in source order.
pub(super) fn parse_source(src: &str) -> ParseOutput {
    let (tokens, lex_diags) = tokenize(src);
    let mut out = Parser::new(&tokens).parse_file();
    // Lexing ran first, so its complaints read first.
    out.warnings.splice(
        0..0,
        lex_diags
            .into_iter()
            .map(|d| format!("elevator: {}", d.message)),
    );
    out
}

/// How deep a chain of (illegal) nested definitions we'll unwrap before
/// giving up. Recovery recurses, so this is what keeps a file of
/// nothing but `{` from blowing the stack.
const MAX_NESTING: usize = 16;

struct Parser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    depth: usize,
    out: ParseOutput,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [SpannedToken]) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            out: ParseOutput::default(),
        }
    }

    fn parse_file(mut self) -> ParseOutput {
        self.parse_import_prelude();
        while self.peek().is_some() {
            self.skip_separators();
            if self.peek().is_none() {
                break;
            }
            if self.peek_ident() == Some("import") {
                self.warn_here("`import` statements must precede every definition");
                self.pos += 1;
                continue;
            }
            if !self.parse_top_def() {
                self.pos += 1;
            }
        }
        self.out
    }

    /// Imports must precede definitions. Commas between them are
    /// tolerated; an `import` after any definition is rejected by the
    /// main loop — that catches "imports added in the middle of the
    /// file" before it becomes a maintenance hazard.
    fn parse_import_prelude(&mut self) {
        loop {
            self.skip_separators();
            if self.peek_ident() != Some("import") {
                return;
            }
            let start = self.loc_here();
            self.advance();
            match self.take_string() {
                Some((path, end)) => self.out.imports.push(ImportStmt { path, start, end }),
                None => self.warn_expected("string after `import`"),
            }
        }
    }

    /// Parse one top-level definition. Returns false on syntax we
    /// can't recognise (caller advances by one to recover).
    fn parse_top_def(&mut self) -> bool {
        let Some(kw) = self.peek_ident().map(str::to_string) else {
            return false;
        };
        let Some(kind) = kind_from_keyword(&kw) else {
            self.warn_here(&format!(
                "unexpected top-level keyword `{}` (expected `import`, `e`, `c`, `f`, `fu`, `concept`, or `ui`)",
                kw
            ));
            return false;
        };
        let start = self.loc_here();
        self.advance();

        let Some((raw, name_end)) = self.read_qualname() else {
            self.warn_expected(&format!("name after `{}`", kind.keyword()));
            return false;
        };
        let qualname = strip_kind_prefix(&raw);
        self.check_name_shape(kind, &qualname, start);
        self.finish_def(kind, qualname, start, name_end);
        true
    }

    /// Parse the optional `{ ... }` of a definition whose keyword and
    /// name have already been consumed, and record it. Shared by
    /// top-level parsing and by nested-definition recovery.
    fn finish_def(&mut self, kind: DefKind, qualname: String, start: Loc, name_end: Loc) {
        let mut def = DefStmt {
            kind,
            qualname,
            start,
            end: name_end,
            description: None,
            where_targets: Vec::new(),
            references: Vec::new(),
            used_by: Vec::new(),
            code_refs: Vec::new(),
            children: Vec::new(),
        };

        if self.peek_token() == Some(&Token::LBrace) {
            let open = self.loc_here();
            self.advance();
            def.end = self.parse_body(&mut def, open);
        }

        self.out.defs.push(def);
    }

    /// Functionalities must be qualified (contain a dot); flat-kind
    /// names must not.
    fn check_name_shape(&mut self, kind: DefKind, qualname: &str, at: Loc) {
        if kind == DefKind::Functionality && !qualname.contains('.') {
            self.warn_at(
                at,
                &format!(
                    "`fu {}` must be qualified (e.g. `fu f.<feature>.<verb>`)",
                    qualname
                ),
            );
        } else if requires_bare_name(kind) && qualname.contains('.') {
            self.warn_at(
                at,
                &format!(
                    "`{} {}` must be a bare name — dotted names are reserved for Functionalities",
                    kind.keyword(),
                    qualname
                ),
            );
        }
    }

    /// Parse a `{ ... }` body into `def`. Returns the location just
    /// past the closing brace, which becomes the definition's span end.
    fn parse_body(&mut self, def: &mut DefStmt, open: Loc) -> Loc {
        loop {
            self.skip_separators();
            match self.peek_token() {
                Some(Token::RBrace) => {
                    let end = self.loc_end_here();
                    self.advance();
                    return end;
                }
                None => {
                    // Located at the `{`, not at EOF — the missing `}`
                    // is a hundred lines from where the author needs
                    // to look.
                    self.warn_at(open, "unexpected end of file inside body opened here");
                    return self.loc_last_end();
                }
                _ => {}
            }
            let Some(kw) = self.peek_ident().map(str::to_string) else {
                self.warn_expected("identifier inside body");
                self.pos += 1;
                continue;
            };
            self.parse_body_item(def, &kw);
        }
    }

    /// Dispatch one `<keyword> ...` item inside a body. Split out of
    /// `parse_body` so neither function carries the whole grammar's
    /// branching weight.
    fn parse_body_item(&mut self, def: &mut DefStmt, kw: &str) {
        match kw {
            "d" => self.parse_description(def),
            "where" => {
                self.advance();
                let list = self.expect_reflist();
                def.where_targets.extend(list);
            }
            "references" => {
                self.advance();
                let list = self.expect_reflist();
                def.references.extend(list);
            }
            "used_by" => {
                self.advance();
                let list = self.expect_reflist();
                def.used_by.extend(list);
            }
            "cr" => self.parse_code_refs(def),
            _ => match kind_from_keyword(kw) {
                Some(child_kind) => self.parse_child_ref(def, child_kind),
                None => {
                    self.warn_here(&format!("unexpected keyword `{}` inside body", kw));
                    self.pos += 1;
                }
            },
        }
    }

    fn parse_description(&mut self, def: &mut DefStmt) {
        let at = self.loc_here();
        self.advance();
        if !self.expect_colon("d") {
            return;
        }
        let Some((text, _)) = self.take_string() else {
            self.warn_expected("string after `d:`");
            return;
        };
        if def.description.is_some() {
            self.warn_at(
                at,
                &format!(
                    "duplicate `d:` on `{} {}` — the later description wins",
                    def.kind.keyword(),
                    def.qualname
                ),
            );
        }
        def.description = Some(text);
    }

    /// `cr:` / `cr.<tag>:` — one or more quoted paths. Distinct from
    /// `expect_reflist` because paths contain `/` and `.` which the
    /// qualname lexer treats as separators, so values must be quoted.
    fn parse_code_refs(&mut self, def: &mut DefStmt) {
        self.advance();
        // Optional tag partitioning refs by layer. Free-form — the
        // parser doesn't enforce which tags exist; conventional ones
        // are `fe` (frontend), `be` (backend), `test`.
        let tag = if self.peek_token() == Some(&Token::Dot) {
            self.advance();
            match self.peek_ident().map(str::to_string) {
                Some(s) => {
                    self.advance();
                    s
                }
                None => {
                    self.warn_expected("tag after `cr.`");
                    String::new()
                }
            }
        } else {
            String::new()
        };
        if !self.expect_colon("cr") {
            return;
        }
        loop {
            let Some((path, _)) = self.take_string() else {
                self.warn_expected("string literal after `cr:`");
                return;
            };
            def.code_refs.push((tag.clone(), path));
            if self.peek_token() == Some(&Token::Comma) {
                self.advance();
                continue;
            }
            return;
        }
    }

    fn parse_child_ref(&mut self, def: &mut DefStmt, child_kind: DefKind) {
        let at = self.loc_here();
        self.advance();
        let Some((name, name_end)) = self.read_qualname() else {
            self.warn_expected(&format!("name after child `{}`", child_kind.keyword()));
            return;
        };
        let raw = strip_kind_prefix(&name);
        // A bare `fu` child is qualified with its parent's name, which
        // only produces a real id when that parent is a Feature.
        if child_kind == DefKind::Functionality
            && !raw.contains('.')
            && def.kind != DefKind::Feature
        {
            self.warn_at(
                at,
                &format!(
                    "bare `fu {}` inside `{} {}` — a Functionality is qualified by its Feature, so write `fu f.<feature>.{}`",
                    raw,
                    def.kind.keyword(),
                    def.qualname,
                    raw
                ),
            );
        }
        let qualified = qualify_child(def, child_kind, &raw);
        def.children.push(ChildRef {
            kind: child_kind,
            raw,
        });
        if self.peek_token() == Some(&Token::LBrace) {
            self.recover_nested_def(child_kind, qualified, at, name_end);
        }
    }

    /// A `{` after a child reference means the author nested a
    /// definition. The language forbids that — a name in a body is a
    /// *use* — and the old parser recovered by skipping the brace,
    /// which quietly merged the inner body's fields into the *parent*
    /// (`c library { f protocol { d: "…" } }` put the description on
    /// `library`). We instead parse the inner block as the definition
    /// it was clearly meant to be; the containment the author wanted
    /// is already recorded as the child reference.
    fn recover_nested_def(&mut self, kind: DefKind, qualname: String, at: Loc, name_end: Loc) {
        self.warn_at(
            at,
            &format!(
                "definitions never nest — `{} {}` inside a body is a reference, not a definition; \
                 define it at the top level (parsed as a top-level `{} {}` so its body isn't lost)",
                kind.keyword(),
                qualname,
                kind.keyword(),
                qualname
            ),
        );
        if self.depth >= MAX_NESTING {
            self.warn_at(at, "nesting too deep — skipping this body");
            return;
        }
        self.depth += 1;
        self.finish_def(kind, qualname, at, name_end);
        self.depth -= 1;
    }

    fn expect_colon(&mut self, after: &str) -> bool {
        if self.peek_token() != Some(&Token::Colon) {
            self.warn_expected(&format!("`:` after `{}`", after));
            return false;
        }
        self.advance();
        true
    }

    /// A colon followed by one or more comma-separated qualified names.
    fn expect_reflist(&mut self) -> Vec<EdgeRef> {
        if !self.expect_colon("edge keyword") {
            return Vec::new();
        }
        let mut out = Vec::new();
        loop {
            let at = self.loc_here();
            let Some((raw, _)) = self.read_qualname() else {
                if out.is_empty() {
                    self.warn_expected("at least one reference after `:`");
                }
                return out;
            };
            let (kind, name) = split_kind_prefix(&raw);
            out.push(EdgeRef { kind, name, at });
            if self.peek_token() == Some(&Token::Comma) {
                self.advance();
                continue;
            }
            return out;
        }
    }

    /// Read `ident (. ident)*`, returning the joined name and the
    /// location just past its last token.
    fn read_qualname(&mut self) -> Option<(String, Loc)> {
        let mut parts: Vec<String> = Vec::new();
        let mut end = Loc::default();
        while let Some(s) = self.peek_ident().map(str::to_string) {
            parts.push(s);
            end = self.loc_end_here();
            self.advance();
            if self.peek_token() != Some(&Token::Dot) {
                break;
            }
            self.advance();
        }
        if parts.is_empty() {
            None
        } else {
            Some((parts.join("."), end))
        }
    }

    /// Consume a string literal, returning its value and end location.
    fn take_string(&mut self) -> Option<(String, Loc)> {
        let s = match self.peek_token() {
            Some(Token::StringLit(s)) => s.clone(),
            _ => return None,
        };
        let end = self.loc_end_here();
        self.advance();
        Some((s, end))
    }

    fn skip_separators(&mut self) {
        while self.peek_token() == Some(&Token::Comma) {
            self.advance();
        }
    }

    fn peek(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.peek().map(|t| &t.token)
    }

    fn peek_ident(&self) -> Option<&str> {
        match self.peek().map(|t| &t.token) {
            Some(Token::Ident(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    /// Start location of the current token, or of end-of-file.
    fn loc_here(&self) -> Loc {
        match self.peek() {
            Some(t) => Loc {
                line: t.line,
                column: t.column,
                offset: t.offset,
            },
            None => self.loc_last_end(),
        }
    }

    /// Location one past the current token's last byte.
    fn loc_end_here(&self) -> Loc {
        match self.peek() {
            Some(t) => Loc {
                line: t.line,
                column: t.end_column(),
                offset: t.end,
            },
            None => self.loc_last_end(),
        }
    }

    /// Location one past the final token in the file — the fallback
    /// span end when a definition runs off the end of the input.
    fn loc_last_end(&self) -> Loc {
        match self.tokens.last() {
            Some(t) => Loc {
                line: t.line,
                column: t.end_column(),
                offset: t.end,
            },
            None => Loc::default(),
        }
    }

    fn warn_at(&mut self, at: Loc, msg: &str) {
        self.out
            .warnings
            .push(format!("elevator: {} at {}", msg, at.describe()));
    }

    fn warn_here(&mut self, msg: &str) {
        let at = self.loc_here();
        self.warn_at(at, msg);
    }

    /// "expected X, got Y" against the current token — the shape most
    /// of the grammar's diagnostics take.
    fn warn_expected(&mut self, what: &str) {
        let got = match self.peek_token() {
            Some(t) => t.describe(),
            None => "end of file".to_string(),
        };
        self.warn_here(&format!("expected {}, got {}", what, got));
    }
}
