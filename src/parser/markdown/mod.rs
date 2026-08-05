//! Markdown (`.md`) — a documentation *link* language.
//!
//! Recovers the shape Obsidian's graph view shows (notes and the links
//! between them) plus the one it structurally cannot: the links that leave
//! the docs and land on source code. Nao's own checkout has 670 note→note
//! links and 1 549 note→source links, so the second kind is the majority of
//! the graph and the reason this parser exists.
//!
//! No tree-sitter. Markdown's link syntax is regular enough to scan line by
//! line, and the interesting content — where a link points — needs no tree.
//! Complexity metrics stay empty, for the same reason they do for SQL and
//! ansible-deploy: prose has no control flow (ADR 0003).
//!
//! ## Entity model
//!
//! One [`EntityKind::Note`] per file. The whole document is a single node
//! because that is the unit a link addresses — headings are not entities.
//!
//! - **Note** — ID `md::note.<absolute path>`. Absolute so that a relative
//!   link resolved against the linking note's directory produces the exact
//!   ID the target file will emit for itself, and the two halves meet
//!   post-merge without a resolution pass. Same trick as the ansible
//!   parser's `ansible::tpl.<rel>`.
//!
//! The note's `name` is its title: frontmatter `title:`, else the first H1,
//! else the filename stem. The stem is kept in `attributes` as `stem:` so
//! the analyzer's wikilink pass can match on either.
//!
//! ## Link model
//!
//! Every outbound link is a `References` edge carrying `link:<form>`
//! metadata (`inline`, `wiki`, `embed`), and `anchor:<frag>` when the link
//! addressed a section.
//!
//! | Written              | Target ID                     | Resolved |
//! |----------------------|-------------------------------|----------|
//! | `[t](../other.md)`   | `md::note.<abs>`              | natively, post-merge |
//! | `[[Other Note]]`     | `md::wiki.<lowercased name>`  | by `analyzer::markdown_links` |
//! | `![[Other Note]]`    | `md::wiki.<lowercased name>`  | ditto, `link:embed` |
//! | `[t](../../src/x.rs)`| — not an edge —               | becomes a `cr:` code ref |
//!
//! A wikilink names a note without saying where it lives, so it cannot be
//! resolved from inside one file; that is the analyzer pass's job. A link
//! that resolves to nothing becomes an `unresolved` Note stub — Obsidian's
//! ghost node, via the same `synthesise_unresolved_stubs` that already
//! serves Elevator.
//!
//! ## Code references are refs, not edges
//!
//! A link to a source file is recorded as an `mdref:<absolute path>`
//! attribute, which `analyzer::markdown_links` rewrites to the `cr:` form
//! Elevator uses. That is deliberate: ADR 0005 decided doc↔code pairing is
//! expressed as **scope**, not as a visual channel, and `cr:` already
//! carries the whole mechanism — the detail panel listing, selecting a note
//! to re-scope the canvas onto the code it describes, the reverse "which
//! notes claim this file" lookup, and drift reporting for a ref that points
//! at nothing. Emitting edges instead would build a second, quieter
//! mechanism next to a working one.
//!
//! ## What is deliberately not read
//!
//! Headings, tags, frontmatter fields other than `title`, tables, and
//! footnotes. Links inside fenced or inline code are masked out before
//! scanning — a fenced example of a link is a code sample, not a claim
//! about the document graph.

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, Position, Relationship, RelationshipKind, Span, Visibility};
use anyhow::Result;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
mod tests;

/// How a link was written. Kept distinct because a reader treats them
/// differently: an embed pulls the target's content into this document,
/// while a link merely points at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkForm {
    /// `[text](target)`
    Inline,
    /// `[[Target]]`
    Wiki,
    /// `![[Target]]` — transclusion.
    Embed,
}

impl LinkForm {
    fn label(self) -> &'static str {
        match self {
            LinkForm::Inline => "inline",
            LinkForm::Wiki => "wiki",
            LinkForm::Embed => "embed",
        }
    }
}

/// One link found in a document, before it is known what it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    form: LinkForm,
    /// The target as written, with any `#anchor` and `|alias` removed.
    target: String,
    /// The `#section` part, if the link addressed one.
    anchor: Option<String>,
    /// 0-indexed line the link was written on.
    line: usize,
}

pub struct MarkdownParser;

impl MarkdownParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for MarkdownParser {
    fn language(&self) -> Language {
        Language::Markdown
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();

        let body = mask_code(content);
        let front = frontmatter(&body);
        let note_id = note_id_for(path);

        let mut note = CodeEntity::new(
            title_of(&body, front, path),
            EntityKind::Note,
            path,
            Span::new(
                Position::new(0, 0, 0),
                Position::new(content.lines().count().saturating_sub(1), 0, content.len()),
            ),
        );
        note.id = note_id.clone();
        note.qualified_name = path.to_string_lossy().to_string();
        note.visibility = Visibility::Public;
        note.tags.insert("markdown".to_string());
        note.documentation = summary(&body, front);
        note.metrics.loc = content.lines().count() as u32;
        // The filename without its extension. A wikilink usually names the
        // file rather than the H1 title, so the analyzer needs both keys.
        note.attributes.push(format!("stem:{}", stem_of(path)));

        for link in scan_links(&body, front) {
            match classify(&link, path) {
                Target::Note(target_id) => {
                    result.add_relationship(edge(&note_id, &target_id, &link));
                }
                Target::Wiki(key) => {
                    result.add_relationship(edge(&note_id, &format!("md::wiki.{}", key), &link));
                }
                // Not an edge by design — see the module header. The
                // analyzer turns this into `cr:<repo-relative>` once it
                // knows the root the path should be relative to.
                Target::Code(target) => note
                    .attributes
                    .push(format!("mdref:{}", target.to_string_lossy())),
                Target::Ignored => {}
            }
        }

        result.add_entity(note);
        Ok(result)
    }
}

/// The stable ID a markdown file emits for itself. Any note linking to this
/// path computes the same string, so the edge connects post-merge.
///
/// The path is normalized first, and that is load-bearing rather than
/// cosmetic. The walker hands out `./CLAUDE.md` when the analysis root is
/// relative, while a link resolved against a note's directory comes back as
/// `CLAUDE.md` — the same file under two spellings, which silently turned
/// every inbound link into a ghost. Both sides go through here, so both get
/// the same string.
pub(crate) fn note_id_for(path: &Path) -> String {
    let normalized = normalize(path).unwrap_or_else(|| path.to_path_buf());
    format!("md::note.{}", normalized.to_string_lossy())
}

/// The wikilink lookup key for a name. Obsidian matches case-insensitively,
/// so the key is lowercased and trimmed.
pub(crate) fn wiki_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn edge(source: &str, target: &str, link: &Link) -> Relationship {
    let mut rel = Relationship::new(source, target, RelationshipKind::References)
        .with_metadata("link", link.form.label());
    if let Some(anchor) = &link.anchor {
        rel = rel.with_metadata("anchor", anchor.clone());
    }
    rel
}

/// What a link turned out to point at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// Another markdown file, addressed by a resolvable path.
    Note(String),
    /// A note addressed by name — needs the whole corpus to resolve.
    Wiki(String),
    /// A source file. Becomes a code ref, not an edge.
    Code(PathBuf),
    /// External URL, pure anchor, or a path that escapes the filesystem.
    Ignored,
}

fn classify(link: &Link, from: &Path) -> Target {
    if link.form != LinkForm::Inline {
        // A wikilink may still name a path (`[[docs/setup]]`); the analyzer
        // indexes notes by stem *and* title, so the last segment is the key.
        let name = link.target.rsplit('/').next().unwrap_or(&link.target);
        let name = name.strip_suffix(".md").unwrap_or(name);
        let key = wiki_key(name);
        return if key.is_empty() { Target::Ignored } else { Target::Wiki(key) };
    }

    if is_external(&link.target) {
        return Target::Ignored;
    }

    let Some(dir) = from.parent() else {
        return Target::Ignored;
    };
    let Some(abs) = normalize(&dir.join(&link.target)) else {
        return Target::Ignored;
    };

    if is_markdown(&abs) {
        Target::Note(note_id_for(&abs))
    } else {
        Target::Code(abs)
    }
}

/// Links nao has nothing to say about: other protocols, page-local
/// anchors, and protocol-relative URLs.
fn is_external(target: &str) -> bool {
    target.starts_with('#')
        || target.starts_with("//")
        || target
            .split_once("://")
            .is_some_and(|(scheme, _)| !scheme.is_empty())
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
}

fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(),
        Some("md") | Some("markdown")
    )
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

/// Resolve `.` and `..` lexically. Not `canonicalize` — that touches the
/// filesystem and fails for a link whose target does not exist, and a link
/// to a missing file is exactly the case that has to survive as a ghost.
/// Returns `None` if the path climbs above its own root.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

// =====================================================================
// Scanning
// =====================================================================

/// Blank out fenced blocks and inline spans, preserving every byte
/// position and line break so spans stay honest. A link written inside a
/// code sample documents syntax; it is not a claim about the graph.
fn mask_code(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut fence: Option<String> = None;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let opener = fence_marker(trimmed);

        match (&fence, opener) {
            // Inside a fence: only the matching marker closes it, so a ```
            // nested in a ~~~~ block does not end it early.
            (Some(open), Some(marker)) if marker.starts_with(open.as_str()) => {
                fence = None;
                out.push_str(&blank(line));
            }
            (Some(_), _) => out.push_str(&blank(line)),
            (None, Some(marker)) => {
                fence = Some(marker);
                out.push_str(&blank(line));
            }
            (None, None) => out.push_str(&mask_inline_code(line)),
        }
    }
    out
}

/// The ``` or ~~~ run opening or closing a fence, if this line is one.
fn fence_marker(trimmed: &str) -> Option<String> {
    for ch in ['`', '~'] {
        let run: String = trimmed.chars().take_while(|c| *c == ch).collect();
        if run.len() >= 3 {
            return Some(run);
        }
    }
    None
}

/// Replace `` `code` `` spans with spaces, keeping the line's length.
fn mask_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut inside = false;
    for ch in line.chars() {
        if ch == '`' {
            inside = !inside;
            out.push(' ');
        } else if inside && ch != '\n' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Replace every character except newlines with a space.
fn blank(line: &str) -> String {
    line.chars().map(|c| if c == '\n' { '\n' } else { ' ' }).collect()
}

/// Line count of a leading `---` frontmatter block, or 0 when there is
/// none. Returned as a line offset so every later scan can skip it without
/// re-detecting it.
fn frontmatter(body: &str) -> usize {
    let mut lines = body.lines();
    if lines.next().map(str::trim) != Some("---") {
        return 0;
    }
    // +2 for the opening delimiter and the closing one.
    lines
        .position(|l| l.trim() == "---")
        .map(|i| i + 2)
        .unwrap_or(0)
}

/// Frontmatter `title:`, else the first H1, else the filename stem.
fn title_of(body: &str, front: usize, path: &Path) -> String {
    let lines: Vec<&str> = body.lines().collect();

    for line in lines.iter().take(front) {
        if let Some(value) = line.trim().strip_prefix("title:") {
            let value = value.trim().trim_matches(['"', '\'']).trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }

    for line in lines.iter().skip(front) {
        if let Some(heading) = line.strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return heading.to_string();
            }
        }
    }

    stem_of(path)
}

/// The first prose paragraph, for the detail panel. Skips frontmatter,
/// headings and blank lines; stops at the first blank line after prose.
fn summary(body: &str, front: usize) -> Option<String> {
    let mut collected: Vec<&str> = Vec::new();
    for line in body.lines().skip(front) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if collected.is_empty() {
                continue;
            }
            break;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("---") {
            if collected.is_empty() {
                continue;
            }
            break;
        }
        collected.push(trimmed);
    }
    if collected.is_empty() {
        return None;
    }
    let joined = collected.join(" ");
    Some(if joined.chars().count() > 280 {
        format!("{}…", joined.chars().take(279).collect::<String>())
    } else {
        joined
    })
}

/// Every link in the masked body, in source order.
fn scan_links(body: &str, front: usize) -> Vec<Link> {
    let mut links = Vec::new();
    for (offset, line) in body.lines().enumerate().skip(front) {
        scan_wiki_links(line, offset, &mut links);
        scan_inline_links(line, offset, &mut links);
    }
    links
}

/// `[[Target]]`, `[[Target|alias]]`, `![[Target]]`.
fn scan_wiki_links(line: &str, offset: usize, links: &mut Vec<Link>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(rel) = line[i..].find("[[") {
        let open = i + rel;
        let Some(close_rel) = line[open + 2..].find("]]") else {
            break;
        };
        let close = open + 2 + close_rel;
        let embed = open > 0 && bytes[open - 1] == b'!';
        let inner = &line[open + 2..close];
        // An alias changes only what the reader sees, never what is linked.
        let target = inner.split('|').next().unwrap_or(inner);
        let (target, anchor) = split_anchor(target);
        if !target.trim().is_empty() {
            links.push(Link {
                form: if embed { LinkForm::Embed } else { LinkForm::Wiki },
                target: target.trim().to_string(),
                anchor,
                line: offset,
            });
        }
        i = close + 2;
    }
}

/// `[text](target)` and `![alt](target)`. Wikilinks are skipped here — they
/// were already taken by `scan_wiki_links`, and `[[x]]` would otherwise
/// read as a `[text]` with no destination.
fn scan_inline_links(line: &str, offset: usize, links: &mut Vec<Link>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = line[i..].find("](") else {
            break;
        };
        let bracket = i + rel;
        let Some(close_rel) = line[bracket + 2..].find(')') else {
            break;
        };
        let close = bracket + 2 + close_rel;

        // `[[wiki]](…)` is not an inline link with a wiki label; the `]]`
        // belongs to the wikilink form handled above.
        let is_wiki_tail = bracket > 0 && bytes[bracket - 1] == b']';
        let raw = &line[bracket + 2..close];
        if !is_wiki_tail {
            // A title (`](path "Title")`) is not part of the destination.
            let dest = raw.split_whitespace().next().unwrap_or("");
            let dest = dest.trim_start_matches('<').trim_end_matches('>');
            let (target, anchor) = split_anchor(dest);
            if !target.trim().is_empty() {
                links.push(Link {
                    form: LinkForm::Inline,
                    target: percent_decode(target.trim()),
                    anchor,
                    line: offset,
                });
            }
        }
        i = close + 1;
    }
}

/// Split a `path#section` into its two halves. A leading `#` is a
/// page-local anchor and stays with the target so `is_external` can reject
/// the whole link.
fn split_anchor(target: &str) -> (&str, Option<String>) {
    match target.find('#') {
        Some(0) | None => (target, None),
        Some(at) => (&target[..at], Some(target[at + 1..].to_string())),
    }
}

/// Decode the `%20` escapes a path with spaces picks up. Only the escapes
/// that appear in file paths — this is not a URL decoder.
fn percent_decode(target: &str) -> String {
    if !target.contains('%') {
        return target.to_string();
    }
    let bytes = target.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| target.to_string())
}
