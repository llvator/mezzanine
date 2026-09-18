//! The text rendering of a tool answer, dressed for a person at a terminal.
//!
//! ADR 0035 says a tool answer is a value with two renderings: the prose
//! and the JSON. The prose has two *readers*, and they are not alike. An
//! agent over stdio gets markdown because markdown is what it reads best;
//! a person gets the same markdown scrolling past in a terminal, where
//! `## Top 10 by refactor pressure` is a line of body text like any other
//! and the sections it separates run together.
//!
//! So this is not a third rendering — nothing here changes what the answer
//! says. It is the terminal's share of the one text rendering, applied at
//! the CLI's print boundary and nowhere else. The MCP server hands its
//! callers the prose untouched: an escape sequence in a `tools/call`
//! response is noise in a transcript, not emphasis.
//!
//! Colour switches itself off when nobody is watching. [`colored`] resolves
//! `CLICOLOR_FORCE`, then `NO_COLOR`, then `CLICOLOR` against whether
//! stdout is a terminal, and its `Display` writes the bare string when the
//! answer is no — so `mezz quality > report.md` gets the same bytes it got
//! before this module existed.

use colored::Colorize;

/// The deepest ATX heading markdown recognises, and the widest run of `#`
/// this will treat as one.
const MAX_HEADING_DEPTH: usize = 6;

/// One tool answer's prose, with its headings styled for a terminal.
///
/// Every other line is returned verbatim. The markers stay in the output
/// rather than being stripped: they are what makes the answer markdown,
/// which is what a caller redirecting it into a file or a PR comment is
/// counting on. Dimming them just moves the eye off them and onto the
/// words, which is the whole ask.
pub fn style_headings(prose: &str) -> String {
    let mut out = String::with_capacity(prose.len());
    let mut fenced = false;
    for (i, line) in prose.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        match heading_depth(line).filter(|_| !fenced) {
            Some(depth) => {
                let (markers, text) = line.split_at(depth);
                out.push_str(&format!("{}{}", markers.dimmed(), text.cyan().bold()));
            }
            None => out.push_str(line),
        }
    }
    // `lines()` drops a trailing newline, and a caller that redirects this
    // into a file should not find the last one missing.
    if prose.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// How many `#` open this line as a heading, or `None` if it is body text.
///
/// The trailing space is required, which is what keeps a fenced-out
/// `#[derive(Debug)]` — or a `#!/bin/sh`, or a Python comment — from being
/// read as a heading. The fence check in [`style_headings`] is the first
/// line of defence and this is the second, because `context` prints
/// captured source and source is full of lines that start with `#`.
fn heading_depth(line: &str) -> Option<usize> {
    let depth = line.chars().take_while(|c| *c == '#').count();
    (1..=MAX_HEADING_DEPTH)
        .contains(&depth)
        .then_some(depth)
        .filter(|d| line[*d..].starts_with(' '))
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    /// [`colored`]'s override is process-global, and the tests below want
    /// it in both positions, so they take turns rather than racing.
    static OVERRIDE: Mutex<()> = Mutex::new(());

    /// Forces colour on or off for the duration of one test.
    fn forcing(colour: bool) -> MutexGuard<'static, ()> {
        let guard = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
        colored::control::set_override(colour);
        guard
    }

    /// The styling is additive: it wraps lines, it never rewrites them. A
    /// caller redirecting prose into a file must get the same document,
    /// which is also why every assertion below strips the escapes rather
    /// than matching on them.
    fn plain(styled: &str) -> String {
        let mut out = String::new();
        let mut rest = styled;
        while let Some(start) = rest.find('\u{1b}') {
            out.push_str(&rest[..start]);
            match rest[start..].find('m') {
                Some(end) => rest = &rest[start + end + 1..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn styling_headings_leaves_the_document_it_was_given() {
        let _colour = forcing(true);
        let prose = "# Quality of src/mcp\n\n## Smells (25)\n- function `trace` — tools.rs:2508\n\n_scope 8e5566 · mezz 1.4.0_";
        assert_eq!(plain(&style_headings(prose)), prose);
    }

    /// `context` fences the target's own source, and Rust source is full of
    /// attributes. A line-prefix rule with no fence tracking colours
    /// `#[derive(Debug)]` as a section heading, in the middle of code the
    /// caller is about to quote or edit.
    ///
    /// The override is what makes this assertable at all: a test process
    /// has no terminal, so [`colored`] is off by default here — which is
    /// the behaviour the next test is about.
    #[test]
    fn an_attribute_inside_a_fence_is_not_a_heading() {
        let _colour = forcing(true);
        let prose = "## Source\n```\n#[derive(Debug)]\nstruct S;\n```\n## Uses (1)";
        let styled = style_headings(prose);

        for line in styled.lines() {
            let is_heading = line.contains("Source") || line.contains("Uses");
            assert_eq!(
                line.contains('\u{1b}'),
                is_heading,
                "only the two headings carry escapes; this line does not qualify:\n{line}"
            );
        }
        assert_eq!(plain(&styled), prose);
    }

    /// `mezz quality > report.md` must produce the file it produced before
    /// this module existed — not markdown with escapes wedged into its
    /// headings. Redirected output is byte-identical, trailing newline and
    /// all.
    #[test]
    fn a_redirected_answer_is_byte_identical_to_the_prose() {
        let _colour = forcing(false);
        let prose = "# Map of src/output\n\n## Smells (2)\n- one\n";

        assert_eq!(style_headings(prose), prose);
    }

    /// A `#` is a heading when it opens a line *and* is followed by a
    /// space. Without the second half, the `#5` in a prose sentence and a
    /// shell shebang both read as headings.
    #[test]
    fn a_hash_is_only_a_heading_when_a_space_follows_it() {
        assert_eq!(heading_depth("# Quality of src"), Some(1));
        assert_eq!(heading_depth("###### Deepest"), Some(6));
        assert_eq!(heading_depth("####### Too deep"), None);
        assert_eq!(heading_depth("#!/bin/sh"), None);
        assert_eq!(heading_depth("#[derive(Debug)]"), None);
        assert_eq!(heading_depth("- ranked #3 by pressure"), None);
        assert_eq!(heading_depth(""), None);
    }

    /// An unterminated fence — a truncated answer, a tool that forgot to
    /// close one — must not leave every heading after it unstyled *and*
    /// must not panic. It simply stops styling, which is the safe half of
    /// the choice.
    #[test]
    fn an_unclosed_fence_swallows_the_rest_without_panicking() {
        let styled = style_headings("# Title\n```\n# not a heading");
        assert_eq!(plain(&styled), "# Title\n```\n# not a heading");
    }
}
