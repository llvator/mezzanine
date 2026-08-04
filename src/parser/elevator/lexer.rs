//! Tokenizer for the Elevator (`.elv`) spec language.
//!
//! Tokens are minimal: braces, colons, commas, dots, identifiers, and
//! double-quoted strings. Whitespace and `#` line-comments are skipped.
//!
//! Every token carries a byte-accurate half-open range plus its
//! line/column, which is what lets the parser build real spans (and
//! slice `source_code`) rather than pointing every entity at column 0
//! of its first line. No token spans a newline, so `end_column` is
//! derivable and isn't stored.
//!
//! **Lexing never fails.** A stray character or an unterminated string
//! produces a diagnostic and the lexer resynchronises, because a `.elv`
//! file is hand-authored prose-adjacent text and one typo used to zero
//! out the whole file's entities — the exact silent-miss failure mode
//! the spec's own `f.parsers.parse` warns about.

/// A lexical diagnostic, already carrying its own location.
pub(super) struct LexDiag {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Token {
    LBrace,
    RBrace,
    Colon,
    Comma,
    Dot,
    Ident(String),
    StringLit(String),
}

impl Token {
    /// Author-facing name used in "expected X, got Y" diagnostics.
    /// `{:?}` leaks Rust variant syntax at the user, which is noise in
    /// a spec-authoring error message.
    pub(super) fn describe(&self) -> String {
        match self {
            Token::LBrace => "`{`".to_string(),
            Token::RBrace => "`}`".to_string(),
            Token::Colon => "`:`".to_string(),
            Token::Comma => "`,`".to_string(),
            Token::Dot => "`.`".to_string(),
            Token::Ident(s) => format!("identifier `{}`", s),
            Token::StringLit(_) => "string literal".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpannedToken {
    /// The delimiting syntax element, e.g. `f`, `{`, `:`.
    pub token: Token,
    /// 0-based line number.
    pub line: usize,
    /// 0-based column of the token's first byte.
    pub column: usize,
    /// Byte offset of the token's first byte.
    pub offset: usize,
    /// Byte offset one past the token's last byte. For a string
    /// literal this includes the quotes, so slicing `[offset..end]`
    /// round-trips the source text.
    pub end: usize,
}

impl SpannedToken {
    /// 0-based column one past the token's last byte. Valid because no
    /// token contains a newline.
    pub(super) fn end_column(&self) -> usize {
        self.column + (self.end - self.offset)
    }
}

/// Tokenize `src`. Returns every token it could recognise plus one
/// diagnostic per skipped-over defect; the token stream is always
/// usable.
pub(super) fn tokenize(src: &str) -> (Vec<SpannedToken>, Vec<LexDiag>) {
    let mut lx = Lexer {
        out: Vec::new(),
        diags: Vec::new(),
        line: 0,
        line_start: 0,
    };
    // `char_indices` gives byte offsets directly, so spans stay valid
    // for non-ASCII descriptions without a separate counter.
    let mut iter = src.char_indices().peekable();

    while let Some(&(i, c)) = iter.peek() {
        match c {
            ' ' | '\t' | '\r' => {
                iter.next();
            }
            '\n' => {
                iter.next();
                lx.line += 1;
                lx.line_start = i + 1;
            }
            '#' => skip_line_comment(&mut iter),
            '{' | '}' | ':' | ',' | '.' => lx.delimiter(i, c, &mut iter),
            '"' => lx.string(i, &mut iter),
            ch if is_ident_start(ch) => lx.ident(i, &mut iter),
            other => {
                lx.diag_at(
                    lx.line,
                    i - lx.line_start,
                    format!("unexpected character `{}`", other),
                );
                iter.next();
            }
        }
    }

    (lx.out, lx.diags)
}

type Chars<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn skip_line_comment(iter: &mut Chars<'_>) {
    while let Some(&(_, ch)) = iter.peek() {
        if ch == '\n' {
            return;
        }
        iter.next();
    }
}

struct Lexer {
    out: Vec<SpannedToken>,
    diags: Vec<LexDiag>,
    line: usize,
    line_start: usize,
}

impl Lexer {
    fn push(&mut self, token: Token, line: usize, offset: usize, end: usize, column: usize) {
        self.out.push(SpannedToken {
            token,
            line,
            column,
            offset,
            end,
        });
    }

    fn diag_at(&mut self, line: usize, column: usize, message: String) {
        self.diags.push(LexDiag {
            message: format!("{} at line {} column {}", message, line + 1, column + 1),
        });
    }

    fn delimiter(&mut self, i: usize, c: char, iter: &mut Chars<'_>) {
        let tok = match c {
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            ':' => Token::Colon,
            ',' => Token::Comma,
            '.' => Token::Dot,
            _ => unreachable!("delimiter called with `{}`", c),
        };
        iter.next();
        let (line, col) = (self.line, i - self.line_start);
        self.push(tok, line, i, i + c.len_utf8(), col);
    }

    /// Lex a double-quoted string. An unterminated one is recovered by
    /// closing it at the end of the line: the description still lands
    /// on its entity and the rest of the file keeps parsing.
    fn string(&mut self, i: usize, iter: &mut Chars<'_>) {
        let (start_line, start_col) = (self.line, i - self.line_start);
        iter.next();
        let mut s = String::new();
        let mut end = i + 1;
        let mut closed = false;
        while let Some(&(j, ch)) = iter.peek() {
            if ch == '"' {
                iter.next();
                end = j + 1;
                closed = true;
                break;
            }
            if ch == '\n' {
                end = j;
                break;
            }
            s.push(ch);
            end = j + ch.len_utf8();
            iter.next();
        }
        if !closed {
            self.diag_at(start_line, start_col, "unterminated string".to_string());
        }
        self.push(Token::StringLit(s), start_line, i, end, start_col);
    }

    fn ident(&mut self, i: usize, iter: &mut Chars<'_>) {
        let start_col = i - self.line_start;
        let mut s = String::new();
        let mut end = i;
        while let Some(&(j, ch)) = iter.peek() {
            if !is_ident_continue(ch) {
                break;
            }
            s.push(ch);
            end = j + ch.len_utf8();
            iter.next();
        }
        let line = self.line;
        self.push(Token::Ident(s), line, i, end, start_col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Token> {
        let (tokens, diags) = tokenize(src);
        assert!(diags.is_empty(), "unexpected diagnostics");
        tokens.into_iter().map(|t| t.token).collect()
    }

    #[test]
    fn lex_simple_feature() {
        assert_eq!(
            kinds("f library { f protocol }"),
            vec![
                Token::Ident("f".to_string()),
                Token::Ident("library".to_string()),
                Token::LBrace,
                Token::Ident("f".to_string()),
                Token::Ident("protocol".to_string()),
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lex_qualified_name_with_dots() {
        assert_eq!(
            kinds("ui.library.protocol"),
            vec![
                Token::Ident("ui".to_string()),
                Token::Dot,
                Token::Ident("library".to_string()),
                Token::Dot,
                Token::Ident("protocol".to_string()),
            ]
        );
    }

    #[test]
    fn lex_string_literal() {
        let (tokens, _) = tokenize(r#"d: "hello world""#);
        assert!(
            matches!(tokens.last().unwrap().token, Token::StringLit(ref s) if s == "hello world")
        );
    }

    #[test]
    fn lex_skips_line_comments() {
        let idents: Vec<String> = kinds("f x  # this is a comment\n  f y")
            .into_iter()
            .filter_map(|t| match t {
                Token::Ident(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(idents, vec!["f", "x", "f", "y"]);
    }

    #[test]
    fn token_offsets_slice_back_to_the_source() {
        let src = "f library { d: \"hi\" }";
        let (tokens, _) = tokenize(src);
        for t in &tokens {
            let text = &src[t.offset..t.end];
            match &t.token {
                Token::Ident(s) => assert_eq!(text, s),
                Token::StringLit(s) => assert_eq!(text, format!("\"{}\"", s)),
                _ => assert_eq!(text.len(), 1),
            }
        }
    }

    #[test]
    fn columns_are_relative_to_the_line() {
        let (tokens, _) = tokenize("f a\n  f b");
        let last = tokens.last().unwrap();
        assert_eq!((last.line, last.column), (1, 4));
    }

    #[test]
    fn unterminated_string_recovers_at_end_of_line() {
        let (tokens, diags) = tokenize("f a { d: \"no closing quote\nf b");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unterminated string at line 1"));
        // The `f b` after the broken line still lexes.
        let idents: Vec<&str> = tokens
            .iter()
            .filter_map(|t| match &t.token {
                Token::Ident(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(idents, vec!["f", "a", "d", "f", "b"]);
    }

    #[test]
    fn stray_character_is_skipped_not_fatal() {
        let (tokens, diags) = tokenize("f a @ f b");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unexpected character `@`"));
        let idents: Vec<&str> = tokens
            .iter()
            .filter_map(|t| match &t.token {
                Token::Ident(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(idents, vec!["f", "a", "f", "b"]);
    }

    #[test]
    fn non_ascii_description_keeps_offsets_valid() {
        let src = "f a { d: \"café ☕\" }";
        let (tokens, diags) = tokenize(src);
        assert!(diags.is_empty());
        let lit = tokens
            .iter()
            .find(|t| matches!(t.token, Token::StringLit(_)))
            .unwrap();
        assert_eq!(&src[lit.offset..lit.end], "\"café ☕\"");
    }
}
