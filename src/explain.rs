//! What the abbreviations in an entity row stand for.
//!
//! `map`, `quality`, `hotspots`, `impact`, `context` and `assess_change` all
//! annotate an entity with the same parenthesised suffix — `(L44, loc 89,
//! cx 11, cog 14, ws 16, out 5, ⚠ Overfull Head)`, built by
//! [`crate::mcp::tools::metric_suffix`]. Nothing beside it says what the
//! tokens mean, and the only written key lived in one tool's documentation
//! page, where it fell a token behind the code without anyone noticing: `ws`
//! shipped, the table was never told, and a reader met an abbreviation
//! nothing defined.
//!
//! So the key lives here as data rather than there as prose. `mezz explain`
//! prints it, and `every_emitted_token_is_explained` renders a row with every
//! metric populated and demands an entry for each token in it — the check
//! that would have caught `ws` the day it shipped.

use crate::models::entity::SmellKind;

/// One token of the metric suffix, and the two lengths of answer about it.
///
/// `gloss` is the table cell — one line, read in passing next to the sample.
/// `detail` is what `mezz explain <key>` adds when a reader stops to ask,
/// and is the only place a threshold or a caveat belongs: a key whose every
/// cell carried its caveats would be unreadable as a key.
pub struct Token {
    /// The name to look it up by: `mezz explain ws`.
    pub key: &'static str,
    /// Other spellings that should find it — the metric's full name, the
    /// hyphenated form, whatever a reader would reasonably type.
    pub aliases: &'static [&'static str],
    /// The token as it appears in output, with a value.
    pub sample: &'static str,
    /// One line. What the token counts.
    pub gloss: &'static str,
    /// The paragraph underneath: how to read the number, and what it hides.
    pub detail: &'static str,
}

impl Token {
    /// Whether `query` names this token.
    ///
    /// Matched against the key and every alias, both verbatim and stripped to
    /// letters and digits, so `cx`, `CX`, `cyclomatic`, `fan-in`, `fan in`
    /// and `overfull-head` all land. The verbatim arm is what lets `⚠` be
    /// looked up at all: stripping it leaves nothing.
    fn names(&self, query: &str) -> bool {
        let normalized = normalize(query);
        std::iter::once(self.key)
            .chain(self.aliases.iter().copied())
            .any(|name| {
                name.eq_ignore_ascii_case(query)
                    || (!normalized.is_empty() && normalize(name) == normalized)
            })
    }
}

/// Lowercased, with everything that is not a letter or a digit dropped, so
/// `fan-in`, `fan in` and `FanIn` are one query.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The suffix tokens, in the order [`crate::mcp::tools::metric_suffix`]
/// emits them, followed by the two that other renderers append to it.
///
/// Reading order is emission order on purpose: the key and the row it
/// explains can be read side by side, and a token added to the middle of the
/// suffix has an obvious place to go here.
pub const TOKENS: &[Token] = &[
    Token {
        key: "L",
        aliases: &["line"],
        sample: "L44",
        gloss: "1-based line the entity starts on",
        detail: "The line the declaration opens on, counted from 1 so it matches the editor \
                 gutter. Tools that take `--line` accept any line inside the entity, this one \
                 included, so a row can be pasted straight back: `mezz context --path <file> \
                 --line 44`.",
    },
    Token {
        key: "loc",
        aliases: &["lines", "lines of code"],
        sample: "loc 89",
        gloss: "lines the entity spans, first to last inclusive",
        detail: "A span, not a statement count — blank lines, comments and nested declarations \
                 inside the entity are all in it. Reported for every entity, which makes it the \
                 one number a container and a callable can be compared on.",
    },
    Token {
        key: "cx",
        aliases: &["cyclomatic", "cyclomatic complexity"],
        sample: "cx 11",
        gloss: "cyclomatic complexity — independent paths through the body",
        detail: "One, plus one for every branch point: each `if`, loop, `match` arm, `&&`, `||` \
                 and `?`. Callables only — a container has no body to branch and prints no `cx`. \
                 This repository's ceiling for newly added code is 15.",
    },
    Token {
        key: "cog",
        aliases: &["cognitive", "cognitive complexity"],
        sample: "cog 14",
        gloss: "cognitive complexity — branching weighted by how deeply it nests",
        detail: "The same branch points as `cx`, except one buried three levels deep costs more \
                 than one at the top. Usually the better answer to \"why is this hard to \
                 follow\" — the ceiling here is 22, and a `cog` far above its `cx` says the \
                 problem is nesting depth rather than branch count.",
    },
    Token {
        key: "ws",
        aliases: &["working set"],
        sample: "ws 16",
        gloss: "working set — distinct names the body holds in view at once",
        detail: "Parameters + locals + the instance fields it reaches for through an explicit \
                 `self`/`this`, counted once each however often they are rebound. Thresholded at \
                 Miller's 7±2 — green at 7 or below, amber to 12, red above — and crossing the \
                 red line is what raises Overfull Head, so `ws 16` and `⚠ Overfull Head` on one \
                 row are a single fact stated twice. The only entity metric here that is not \
                 about control flow, which is the point of it: `ws 3` beside `cog 40` reads \
                 branchy but narrow, `ws 16` beside `cog 2` reads flat but crowded. A floor, not \
                 a total — languages that let a method write `total` for `this.total` (Java, \
                 Kotlin, Groovy, Go) under-count the field part.",
    },
    Token {
        key: "methods",
        aliases: &["member", "members", "method count"],
        sample: "methods 4",
        gloss: "members declared inside a container",
        detail: "Printed on classes, structs, enums and interfaces, and omitted at 0. It counts \
                 the members without naming them — `mezz map --depth 3` lists them as their own \
                 rows underneath, each with a suffix of its own.",
    },
    Token {
        key: "in",
        aliases: &["fan-in", "fan in"],
        sample: "in 3",
        gloss: "fan-in — distinct entities that depend on this one",
        detail: "How far a change here reaches, counted over the graph mezz resolved. Very high \
                 fan-in raises Shotgun Surgery. Omitted at 0, so a row without it is depended on \
                 by nothing mezz can see — which for an entry point or a trait method means the \
                 graph is missing the caller, not that there is none. `mezz impact` names them.",
    },
    Token {
        key: "out",
        aliases: &["fan-out", "fan out"],
        sample: "out 5",
        gloss: "fan-out — distinct entities this one depends on",
        detail: "Every name the body reaches for: the ones mezz resolved to something it has \
                 seen and the ones it could not — a `std` call, a library method, a link in a \
                 builder chain against an external type. On this repository the median entity \
                 with `out >= 5` has about 71% of it landing on unresolved names, so `out` is a \
                 count of names mentioned rather than of things you must understand. That is \
                 what `N of it identified` corrects. Omitted at 0.",
    },
    Token {
        key: "cycle",
        aliases: &[],
        sample: "cycle",
        gloss: "this entity sits in a dependency cycle",
        detail: "It is inside a strongly connected component larger than one: following its \
                 dependencies far enough gets you back to it. A flag, not a count — absent means \
                 not in one. `mezz cycles`, or the cycles section of `mezz quality`, names the \
                 loop.",
    },
    Token {
        key: "identified",
        aliases: &["of it identified", "unresolved"],
        sample: "6 of it identified",
        gloss: "how much of `out` mezz resolved to something it has seen",
        detail: "Appended by `quality` to its offender rows rather than printed everywhere, \
                 because it only matters where `out` is being read as pressure. `out 32, 6 of it \
                 identified` means 26 of those names are external or unresolvable — the score \
                 counts all 32. A large `out` with a small identified count is a row mentioning \
                 a lot of names, not one depending on a lot of things.",
    },
    Token {
        key: "⚠",
        aliases: &["smell", "smells", "warning"],
        sample: "⚠ Overfull Head",
        gloss: "the smells that fired, comma-separated",
        detail: "Each smell is a named combination of the numbers beside it, not a separate \
                 measurement — so a smell and the metric that raised it are the same finding. \
                 Data Bag is informational rather than a red flag. `mezz explain \"Overfull \
                 Head\"` for any of them.",
    },
];

/// The token whose key or alias matches `query`, if any.
pub fn token(query: &str) -> Option<&'static Token> {
    TOKENS.iter().find(|t| t.names(query))
}

/// The smell whose label matches `query`, if any. `overfull head`,
/// `Overfull-Head` and `OverfullHead` all find it.
pub fn smell(query: &str) -> Option<SmellKind> {
    let normalized = normalize(query);
    SmellKind::all()
        .into_iter()
        .find(|s| normalize(s.label()) == normalized)
}

/// The whole key: the suffix tokens, then the smells.
pub fn key() -> String {
    let mut out = vec![
        "Every tool that lists entities annotates them with the same suffix:".to_string(),
        String::new(),
        "  function readingOf (L44, loc 89, cx 11, cog 14, ws 16, out 5, ⚠ Overfull Head)"
            .to_string(),
        String::new(),
        "## Metrics".to_string(),
    ];
    // Chars, not bytes: `{:width$}` pads by char count, so a byte width would
    // over-pad every row the moment the widest sample stops being ASCII.
    let width = TOKENS
        .iter()
        .map(|t| t.sample.chars().count())
        .max()
        .unwrap_or(0);
    out.extend(
        TOKENS
            .iter()
            .map(|t| format!("  {:width$}  {}", t.sample, t.gloss)),
    );
    out.push(String::new());
    out.push(
        "Tokens are omitted rather than printed at zero, and a metric the parser did not \
         measure for that language is absent too — so a missing `in`/`out` means uncoupled in \
         the graph, and a missing `ws` means unmeasured, not 0."
            .to_string(),
    );
    out.push(String::new());
    out.push("## Smells".to_string());
    out.extend(smell_rows());
    out.push(String::new());
    out.push(
        "`mezz explain <token>` says more about one of them — `mezz explain ws`, \
         `mezz explain \"Overfull Head\"`."
            .to_string(),
    );
    out.join("\n")
}

/// One line per smell, aligned on the label.
///
/// The informational marker leads the meaning rather than trailing it: on a
/// three-line wrap the qualifier that says "this is not a defect" is the one
/// part a reader must not reach last.
fn smell_rows() -> Vec<String> {
    let all = SmellKind::all();
    let width = all.iter().map(|s| s.label().chars().count()).max().unwrap_or(0);
    all.iter()
        .map(|s| {
            let note = if s.is_informational() {
                "(informational) "
            } else {
                ""
            };
            format!("  ⚠ {:width$}  {note}{}", s.label(), s.meaning())
        })
        .collect()
}

/// The long answer about one token or one smell, or `None` if nothing here
/// goes by that name.
pub fn lookup(query: &str) -> Option<String> {
    if let Some(t) = token(query) {
        return Some(format!("{}\n{}\n\n{}", t.sample, t.gloss, t.detail));
    }
    let s = smell(query)?;
    let note = if s.is_informational() {
        "\n\nInformational: it reports a shape, it does not claim a defect."
    } else {
        ""
    };
    Some(format!(
        "⚠ {}\na smell\n\n{}{note}\n\nFix: {}",
        s.label(),
        s.meaning(),
        s.hint()
    ))
}

/// Everything `lookup` answers to, for the error message when it answers to
/// none of it.
pub fn known() -> String {
    TOKENS
        .iter()
        .map(|t| t.key.to_string())
        .chain(SmellKind::all().iter().map(|s| format!("\"{}\"", s.label())))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::metric_suffix;
    use crate::models::entity::{CodeEntity, EntityKind};
    use crate::models::file_info::Span;

    /// An entity with every metric the suffix can print populated, so the row
    /// it renders exercises every branch of `metric_suffix`.
    fn fully_measured() -> CodeEntity {
        let mut e = CodeEntity::new(
            "readingOf",
            EntityKind::Function,
            "src/thing.rs",
            Span::from_positions(43, 0, 131, 1),
        );
        let m = &mut e.metrics;
        m.loc = 89;
        m.cyclomatic = Some(11);
        m.cognitive_complexity = Some(14);
        m.working_set = Some(16);
        m.method_count = 4;
        m.fan_in = 3;
        m.fan_out = 5;
        m.in_cycle = true;
        m.smells = vec![SmellKind::OverfullHead];
        e
    }

    /// The token name a rendered suffix part goes by — `L44` is `L`,
    /// `⚠ Overfull Head` is `⚠`.
    fn token_name(part: &str) -> String {
        if part.starts_with('⚠') {
            return "⚠".to_string();
        }
        part.chars().take_while(|c| c.is_alphabetic()).collect()
    }

    /// Every token the suffix can emit has an entry in the key.
    ///
    /// This is the check that was missing when `ws` shipped: the renderer
    /// grew a token, the written key did not, and nothing failed. Adding a
    /// token to `metric_suffix` now fails here until it is explained.
    #[test]
    fn every_emitted_token_is_explained() {
        let row = metric_suffix(&fully_measured());
        for part in row.split(", ") {
            let name = token_name(part);
            assert!(
                token(&name).is_some(),
                "`{name}` is emitted in `{row}` but `mezz explain` does not define it"
            );
        }
    }

    /// The sample in the key is the shape the renderer actually produces.
    /// A key whose samples drift from the output is worse than none — it
    /// teaches a reader to look for text that is not there.
    #[test]
    fn samples_match_the_rendered_row() {
        let row = metric_suffix(&fully_measured());
        for sample in TOKENS.iter().map(|t| t.sample) {
            if sample.starts_with('⚠') || sample.starts_with(|c: char| c.is_numeric()) {
                continue;
            }
            assert!(
                row.split(", ").any(|part| part == sample),
                "sample `{sample}` does not appear in the rendered row `{row}`"
            );
        }
    }

    #[test]
    fn lookup_takes_the_spellings_a_reader_would_type() {
        for query in ["ws", "WS", "working set", "working-set", "workingSet"] {
            assert!(lookup(query).is_some(), "`{query}` found nothing");
        }
        for query in ["Overfull Head", "overfull-head", "overfullhead"] {
            assert!(lookup(query).is_some(), "`{query}` found nothing");
        }
        assert!(lookup("⚠").is_some());
        assert!(lookup("fan-in").is_some());
    }

    #[test]
    fn lookup_declines_a_name_it_does_not_carry() {
        assert!(lookup("churn").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("-").is_none());
    }

    /// Every smell is reachable by its own label, so the `⚠` entry's promise
    /// that `mezz explain "<label>"` works holds for all six.
    #[test]
    fn every_smell_is_in_the_key() {
        let key = key();
        for s in SmellKind::all() {
            assert!(key.contains(s.label()), "{} missing from the key", s.label());
            assert!(lookup(s.label()).is_some(), "{} not explained", s.label());
        }
    }
}
