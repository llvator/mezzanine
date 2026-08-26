//! Statement splitting for SQL files.
//!
//! `sqlparser` is all-or-nothing per parse: one unsupported statement in a
//! file costs the whole file. Every other parser in Mezzanine is error-tolerant
//! because tree-sitter is, so the SQL parser has to buy that tolerance
//! itself — split the file into statements first, then parse each one and
//! warn on the ones that fail.
//!
//! Splitting on `;` is not enough. A PostgreSQL file can contain semicolons
//! inside string literals, inside `--` and `/* */` comments, and inside
//! dollar-quoted bodies (`$$ … $$`, `$tag$ … $tag$`) that routinely hold
//! whole procedural programs. This scanner skips all four.

/// Split `sql` into individual statements, dropping the separating `;`.
///
/// Blank chunks are omitted. A trailing statement with no `;` is included.
pub fn split_statements(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match skip_opaque(sql, i) {
            Some(next) => i = next,
            None => {
                if bytes[i] == b';' {
                    push_trimmed(&mut out, &sql[start..i]);
                    start = i + 1;
                }
                i += 1;
            }
        }
    }
    push_trimmed(&mut out, &sql[start..]);
    out
}

fn push_trimmed<'a>(out: &mut Vec<&'a str>, chunk: &'a str) {
    let trimmed = trim_leading_noise(chunk).trim_end();
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
}

/// Drop leading whitespace and comments from a statement.
///
/// Migration files open with banner comments, and a chunk runs from the
/// previous `;` — so without this the first statement in a file starts at
/// line 0 and drags the banner along. That would put every entity's span on
/// the banner rather than on its `CREATE TABLE`, and would make a parse
/// warning cite the wrong line, which is worse: the whole point of the
/// warning is to send someone to the statement that failed.
fn trim_leading_noise(chunk: &str) -> &str {
    let mut rest = chunk.trim_start();
    while let Some(after) = strip_leading_comment(rest) {
        rest = after.trim_start();
    }
    rest
}

/// Strip one leading comment, returning the text after it. `None` when
/// `rest` does not start with a comment. An unterminated block comment
/// consumes the remainder.
fn strip_leading_comment(rest: &str) -> Option<&str> {
    if let Some(after) = rest.strip_prefix("--") {
        return Some(after.find('\n').map_or("", |i| &after[i + 1..]));
    }
    let after = rest.strip_prefix("/*")?;
    Some(after.find("*/").map_or("", |i| &after[i + 2..]))
}

/// If a region that must not be scanned for `;` starts at `i`, return the
/// index just past it. Returns `None` when `i` is ordinary SQL text.
///
/// Shared with the nested-DDL recovery pass (SQL-003), which has to skip the
/// same four regions for a different reason.
pub(crate) fn skip_opaque(sql: &str, i: usize) -> Option<usize> {
    let b = sql.as_bytes();
    match b[i] {
        b'-' if b.get(i + 1) == Some(&b'-') => Some(skip_line_comment(b, i)),
        b'/' if b.get(i + 1) == Some(&b'*') => Some(skip_block_comment(b, i)),
        b'\'' => Some(skip_single_quoted(b, i)),
        b'$' => dollar_quote_end(sql, i),
        _ => None,
    }
}

fn skip_line_comment(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    i
}

fn skip_block_comment(b: &[u8], i: usize) -> usize {
    let mut j = i + 2;
    while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
        j += 1;
    }
    (j + 2).min(b.len())
}

/// Skip `'…'`, honouring the `''` escape for a literal apostrophe.
fn skip_single_quoted(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if b[j] != b'\'' {
            j += 1;
            continue;
        }
        if b.get(j + 1) == Some(&b'\'') {
            j += 2;
            continue;
        }
        return j + 1;
    }
    b.len()
}

/// If a dollar-quoted string opens at `i`, return the index just past its
/// closing tag. A `$` that is not a valid opening tag (`$1`, a stray `$`)
/// returns `None` so the caller treats it as ordinary text.
fn dollar_quote_end(sql: &str, i: usize) -> Option<usize> {
    let (tag, body_start) = dollar_tag(sql, i)?;
    let close = sql[body_start..].find(tag)?;
    Some(body_start + close + tag.len())
}

/// Read the `$tag$` opening at `i`, returning the tag text and the index of
/// the body that follows it. The tag may be empty (`$$`); anything between
/// the dollars must be alphanumeric or `_`.
pub(crate) fn dollar_tag(sql: &str, i: usize) -> Option<(&str, usize)> {
    let rest = sql.get(i + 1..)?;
    let close_offset = rest.find('$')?;
    let tag = &sql[i..i + 1 + close_offset + 1];
    let inner = &tag[1..tag.len() - 1];
    if inner.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some((tag, i + tag.len()))
    } else {
        None
    }
}
