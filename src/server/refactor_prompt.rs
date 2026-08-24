//! Builds the `refactor_prompt` scope export (SRV-010).
//!
//! Every other export in [`super::types::ScopeExports`] is raw context —
//! paths, line ranges, source. This one is the instruction that goes on top
//! of it, so a user can hand the whole thing to a coding agent without
//! writing the ask themselves.
//!
//! Two rules keep the prompt honest:
//!
//! 1. **Every number comes from the analyzer.** Measured values come off
//!    [`EntityMetrics`], thresholds off [`Thresholds`] — the same struct the
//!    frontend reads, so the prompt can't disagree with the quality report.
//! 2. **The metrics cited are the metrics that rank the entity.** The set
//!    mirrors `populate_composite_scores` in `graph.rs`, splitting on
//!    `is_callable()`. Citing a metric the ranking ignores would send an
//!    agent after the wrong thing.

use crate::models::thresholds::WarnBad;
use crate::models::{CodeEntity, EntityKind, SmellKind, Thresholds};

/// What the prompt's Context section carries (SRV-016).
///
/// The full-source form is 98% of the prompt's bytes on a real hotspot, and
/// the agent can read those files itself. The cheaper shapes exist so
/// EV-003 can measure whether skipping them costs anything; `Full` stays the
/// default until it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum PromptContext {
    /// Every in-scope entity's source, inline. ~23k tokens on a real hotspot.
    #[default]
    Full,
    /// `path:start-end` for every in-scope entity; the agent reads what it
    /// decides it needs.
    Ranges,
    /// The target's source in full, ranges for its neighbours.
    Hybrid,
}

impl PromptContext {
    /// Parse the wire value. `None` keeps the default so existing callers are
    /// unaffected; an unknown value is an error rather than a silent fallback,
    /// matching how `mode` is handled in `collect_scope`.
    pub(super) fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim) {
            None | Some("") | Some("full") => Ok(Self::Full),
            Some("ranges") => Ok(Self::Ranges),
            Some("hybrid") => Ok(Self::Hybrid),
            Some(other) => Err(format!(
                "Unknown prompt_context: {other} (expected full, ranges, or hybrid)"
            )),
        }
    }

    /// The Context preamble. Each mode has to describe what actually follows
    /// it — promising source that isn't there is worse than either shape.
    fn preamble(self) -> &'static str {
        match self {
            Self::Full => {
                "The source of the target entity and its immediate neighbours follows. Each \
                 block is preceded by a `// <path>:<start>-<end>` comment. This is a starting \
                 set, not the full picture — read more of the repository as you need it.\n"
            }
            Self::Ranges => {
                "The entities in scope are listed below as `<path>:<start>-<end>` — locations, \
                 not contents. Read the ones you need before changing anything; they were \
                 selected because they call the target, are called by it, share its parent, or \
                 supply its types. The list is a starting set, not the full picture.\n"
            }
            Self::Hybrid => {
                "The target entity's source follows in full, then the locations of the other \
                 entities in scope as `<path>:<start>-<end>`. Read the ones you need — they \
                 were selected because they call the target, are called by it, share its \
                 parent, or supply its types.\n"
            }
        }
    }
}

/// One metric worth putting in front of the agent: what it measures, what it
/// currently reads, and the line it has crossed.
struct Finding {
    label: &'static str,
    gloss: &'static str,
    value: f32,
    warn: f32,
    bad: f32,
}

impl Finding {
    /// `true` once the metric is at or past its warn line — below that it is
    /// not worth the agent's attention.
    fn is_over(&self) -> bool {
        self.value >= self.warn
    }

    /// Whole numbers render without a decimal tail; ratios keep two places.
    /// `loc 214` reads better than `loc 214.00`, and `0.83` is meaningless
    /// as `1`.
    fn fmt_value(v: f32) -> String {
        if v.fract().abs() < f32::EPSILON {
            format!("{}", v as i64)
        } else {
            format!("{v:.2}")
        }
    }

    fn render(&self) -> String {
        let past_red = self.value >= self.bad;
        let (tier, line) = if past_red {
            ("red", self.bad)
        } else {
            ("warn", self.warn)
        };
        // A value sitting exactly on its threshold is *at* the line, not over
        // it. Saying "over the red threshold of 5" when the value reads 5
        // makes the agent doubt every other number in the prompt.
        let relation = if (self.value - line).abs() < f32::EPSILON {
            "at"
        } else {
            "over"
        };
        format!(
            "- {} = {} ({} the {} threshold of {}) — {}",
            self.label,
            Self::fmt_value(self.value),
            relation,
            tier,
            Self::fmt_value(line),
            self.gloss,
        )
    }
}

fn finding(label: &'static str, gloss: &'static str, value: f32, t: &WarnBad) -> Finding {
    Finding {
        label,
        gloss,
        value,
        warn: t.warn,
        bad: t.bad,
    }
}

/// Metrics that drive `composite_score` for a callable.
///
/// Mirrors the callable arm of `populate_composite_scores`; keep the two in
/// step so the prompt never cites a metric the ranking ignores.
fn callable_findings(e: &CodeEntity, t: &Thresholds) -> Vec<Finding> {
    let m = &e.metrics;
    vec![
        finding(
            "cyclomatic complexity",
            "number of independent paths through the body; every branch adds one",
            m.cyclomatic.unwrap_or(0) as f32,
            &t.cc,
        ),
        finding(
            "cognitive complexity",
            "branching weighted by nesting depth — deeply nested logic scores far above flat guard clauses",
            m.cognitive_complexity.unwrap_or(0) as f32,
            &t.cognitive,
        ),
        finding(
            "max nesting depth",
            "deepest level of nested control flow",
            m.max_nesting.unwrap_or(0) as f32,
            &t.nest,
        ),
        finding("lines of code", "size of the entity body", m.loc as f32, &t.loc_callable),
        finding(
            "parameter count",
            "arguments taken, excluding self/this",
            m.param_count.unwrap_or(0) as f32,
            &t.params,
        ),
        finding(
            "fan-out",
            "distinct entities this one depends on; high fan-out means it knows about too much",
            m.fan_out as f32,
            &t.fan_out,
        ),
    ]
}

/// Metrics that drive `composite_score` for a container (class, struct,
/// trait, enum, module). Mirrors the container arm of
/// `populate_composite_scores`.
fn container_findings(e: &CodeEntity, t: &Thresholds) -> Vec<Finding> {
    let m = &e.metrics;
    let is_enum = e.kind == EntityKind::Enum;
    let mut out = vec![
        finding(
            if is_enum {
                "variant count"
            } else {
                "field count"
            },
            "declared members held by this type",
            m.field_count.unwrap_or(0) as f32,
            if is_enum { &t.variants } else { &t.fields },
        ),
        finding(
            "method count",
            "callables contained directly by this type",
            m.method_count as f32,
            &t.method_count,
        ),
        finding(
            "lines of code",
            "size of the entity body",
            m.loc as f32,
            &t.loc_container,
        ),
        finding(
            "fan-out",
            "distinct entities this one depends on; high fan-out means it knows about too much",
            m.fan_out as f32,
            &t.fan_out,
        ),
    ];
    // Encapsulation only counts once a type has real behaviour — a plain
    // record with public fields is a legitimate shape, not weak
    // encapsulation. Same `method_count > 3` gate the score applies.
    if m.method_count > 3 {
        if let Some(ratio) = m.public_field_ratio {
            out.push(finding(
                "public field ratio",
                "fraction of fields exposed publicly; high values on a type with behaviour suggest weak encapsulation",
                ratio,
                &t.public_field_ratio,
            ));
        }
    }
    out
}

/// The always-relevant signals that sit outside the per-kind metric split.
fn cross_cutting(e: &CodeEntity, t: &Thresholds) -> Vec<String> {
    let mut out = Vec::new();
    if e.metrics.in_cycle {
        out.push(
            "- this entity sits in a dependency cycle, so changes here can ripple back around to it"
                .to_string(),
        );
    }
    if e.metrics.fan_in as f32 >= t.shotgun_fan_in {
        out.push(format!(
            "- fan-in = {} — many callers depend on this, so its signature is expensive to change",
            e.metrics.fan_in
        ));
    }
    out
}

/// Build the instruction half of the prompt: everything except the source
/// context, which the caller appends.
///
/// `rel_path` is the entity's path relative to the repo root — the caller
/// already computes it for the other exports, so it is passed in rather than
/// re-derived here.
pub(super) fn build_header(
    e: &CodeEntity,
    rel_path: &str,
    t: &Thresholds,
    ctx: PromptContext,
) -> String {
    let mut out = String::new();

    out.push_str(
        "# Refactoring task\n\n\
         You are working in an existing repository. A static-analysis pass over this \
         codebase flagged the entity below as carrying high refactor pressure. Your job is \
         to reduce that pressure while preserving behaviour.\n\n\
         The measurements below are computed from the code as it stands — they are facts \
         about the current source, not opinions. Treat them as the starting point for your \
         own reading of the code, not as a specification to satisfy.\n\n",
    );

    let line = e.span.start.line + 1;
    let end_line = e.span.end.line + 1;
    out.push_str("## Target\n\n");
    out.push_str(&format!(
        "- {} `{}`\n- {}:{}-{}\n",
        e.kind.display_name(),
        e.qualified_name,
        rel_path,
        line,
        end_line,
    ));
    out.push_str(&format!(
        "- refactor pressure score: {:.2} (composite of the metrics below; \
         roughly 0 = healthy, 1 = at the red line)\n\n",
        e.metrics.composite_score
    ));

    let findings = if e.kind.is_callable() {
        callable_findings(e, t)
    } else {
        container_findings(e, t)
    };
    let over: Vec<&Finding> = findings.iter().filter(|f| f.is_over()).collect();
    let notes = cross_cutting(e, t);

    out.push_str("## What the analysis found\n\n");
    if over.is_empty() && notes.is_empty() && e.metrics.smells.is_empty() {
        // Saying "no problems" plainly beats an empty section the agent has
        // to interpret — and beats inventing work to justify the prompt.
        out.push_str(
            "Every metric for this entity is within its threshold and no smells were \
             detected. There may still be a good reason to change this code, but the \
             analyzer is not the one asking for it — check with whoever sent you here \
             before restructuring anything.\n\n",
        );
    } else {
        if over.is_empty() {
            out.push_str("No individual metric is over its threshold.\n");
        } else {
            for f in &over {
                out.push_str(&f.render());
                out.push('\n');
            }
        }
        for n in &notes {
            out.push_str(n);
            out.push('\n');
        }
        out.push('\n');
    }

    if !e.metrics.smells.is_empty() {
        out.push_str("## Detected smells\n\n");
        for s in &e.metrics.smells {
            out.push_str(&format!("- **{}** — {}\n", s.label(), s.hint()));
        }
        out.push('\n');
        out.push_str(
            "These are pattern signals derived from the metrics above, not diagnoses. \
             Confirm each one against the source before acting on it.\n\n",
        );
    }

    out.push_str(
        "## Constraints\n\n\
         - Preserve the public API and observable behaviour of this entity unless the task \
         you were given says otherwise.\n\
         - Keep the existing tests passing. If no test covers the behaviour you are about \
         to change, add one first so the refactor is verifiable.\n\
         - Prefer the smallest change that moves the metric for a real reason. Splitting an \
         entity to make a number drop, without making the code clearer, is a regression.\n\
         - Do not reformat, rename, or restructure code you are not otherwise touching.\n\
         - If you conclude the entity does not actually need refactoring, say so and stop \
         rather than changing it anyway.\n\n",
    );

    out.push_str("## Context\n\n");
    out.push_str(ctx.preamble());

    out
}

/// Assemble the Context body for `ctx`.
///
/// `entity_context` is the concatenated source already built for the
/// `entity_context` export; `ranges` is the `path:start-end` listing already
/// built for the `ranges` export; `target_source` is the selected entity's own
/// block, needed only by `Hybrid`. Reusing the existing exports keeps the
/// prompt consistent with what the copy buttons produce.
pub(super) fn context_body(
    ctx: PromptContext,
    entity_context: &str,
    ranges: &str,
    target_source: Option<&str>,
) -> String {
    match ctx {
        PromptContext::Full => entity_context.to_string(),
        PromptContext::Ranges => ranges.to_string(),
        PromptContext::Hybrid => match target_source {
            // The target's source, then everyone else's location. The target
            // is the thing being restructured, so it is the one block the
            // agent should never have to go and fetch.
            Some(src) => format!("{src}\n\n### Other entities in scope\n\n{ranges}"),
            // Selected entity excluded from context (its file was unchecked)
            // — degrade to ranges rather than emitting an empty section.
            None => ranges.to_string(),
        },
    }
}

/// Smells the engine can detect but the caller may not have text for.
/// Compile-time proof that every variant has a label and a hint, so adding a
/// `SmellKind` without extending the vocabulary is a build error rather than
/// a prompt that silently omits it.
#[allow(dead_code)]
fn assert_smell_vocabulary_total(s: SmellKind) -> (&'static str, &'static str) {
    match s {
        SmellKind::GodClass
        | SmellKind::Dispatcher
        | SmellKind::FeatureEnvy
        | SmellKind::ShotgunSurgery
        | SmellKind::DataBag => (s.label(), s.hint()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntityMetrics, Position, Span};

    /// Entity spanning source lines 10–41 (the struct stores them 0-based).
    fn entity(kind: EntityKind, metrics: EntityMetrics) -> CodeEntity {
        let span = Span {
            start: Position {
                line: 9,
                column: 0,
                offset: 0,
            },
            end: Position {
                line: 40,
                column: 0,
                offset: 0,
            },
        };
        let mut e = CodeEntity::new("target", kind, "src/app.rs", span);
        e.qualified_name = "app::target".to_string();
        e.metrics = metrics;
        e
    }

    /// The headline claim of the export: a smelly entity produces a prompt
    /// naming the measured value, the threshold it crossed, and the smell.
    #[test]
    fn smelly_callable_cites_values_thresholds_and_smells() {
        let m = EntityMetrics {
            cyclomatic: Some(31),
            cognitive_complexity: Some(40),
            max_nesting: Some(6),
            loc: 210,
            param_count: Some(9),
            fan_out: 22,
            composite_score: 1.4,
            smells: vec![SmellKind::Dispatcher],
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Function, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("cyclomatic complexity = 31"), "{p}");
        assert!(
            p.contains("threshold of 20"),
            "missing the cc red line:\n{p}"
        );
        assert!(p.contains("Dispatcher"), "{p}");
        assert!(
            p.contains(SmellKind::Dispatcher.hint()),
            "hint not rendered:\n{p}"
        );
        assert!(p.contains("src/app.rs:10-41"), "wrong 1-based span:\n{p}");
    }

    /// A clean entity must say so rather than emitting an empty findings
    /// section for the agent to interpret as "look harder".
    #[test]
    fn clean_entity_says_it_is_within_thresholds() {
        let m = EntityMetrics {
            cyclomatic: Some(2),
            cognitive_complexity: Some(1),
            max_nesting: Some(1),
            loc: 12,
            param_count: Some(1),
            fan_out: 1,
            composite_score: 0.1,
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Function, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("within its threshold"), "{p}");
        assert!(
            !p.contains("## Detected smells"),
            "clean entity got a smell section:\n{p}"
        );
    }

    /// Metrics below the warn line are noise — the agent should not have to
    /// filter them out itself.
    #[test]
    fn metrics_under_the_warn_line_are_omitted() {
        let m = EntityMetrics {
            cyclomatic: Some(25),          // over
            cognitive_complexity: Some(1), // well under
            loc: 8,                        // well under
            fan_out: 1,                    // well under
            param_count: Some(1),
            max_nesting: Some(1),
            composite_score: 0.6,
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Function, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("cyclomatic complexity = 25"), "{p}");
        assert!(
            !p.contains("cognitive complexity ="),
            "under-threshold metric leaked:\n{p}"
        );
        assert!(
            !p.contains("lines of code ="),
            "under-threshold metric leaked:\n{p}"
        );
    }

    /// Containers are scored on a different metric set than callables, and
    /// the prompt has to follow that split or it cites numbers the ranking
    /// never looked at.
    #[test]
    fn container_cites_container_metrics_not_callable_ones() {
        let m = EntityMetrics {
            field_count: Some(24),
            method_count: 31,
            loc: 400,
            fan_out: 18,
            composite_score: 1.7,
            smells: vec![SmellKind::GodClass],
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Struct, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("field count = 24"), "{p}");
        assert!(p.contains("method count = 31"), "{p}");
        assert!(
            !p.contains("cyclomatic"),
            "callable metric on a container:\n{p}"
        );
        assert!(
            !p.contains("parameter count"),
            "callable metric on a container:\n{p}"
        );
    }

    /// Enums hold variants, not fields, and are scored against a different
    /// threshold — the label has to follow.
    #[test]
    fn enum_reports_variants_against_the_variant_threshold() {
        let m = EntityMetrics {
            field_count: Some(20),
            loc: 60,
            composite_score: 0.9,
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Enum, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("variant count = 20"), "{p}");
        assert!(!p.contains("field count"), "enum labelled as fields:\n{p}");
    }

    /// A high-fan-in entity is expensive to change even when its own metrics
    /// are fine; that has to reach the agent as a constraint.
    #[test]
    fn high_fan_in_and_cycle_membership_are_surfaced() {
        let m = EntityMetrics {
            cyclomatic: Some(3),
            loc: 10,
            fan_in: 40,
            in_cycle: true,
            composite_score: 0.7,
            ..Default::default()
        };
        let p = build_header(
            &entity(EntityKind::Function, m),
            "src/app.rs",
            &Thresholds::default(),
            PromptContext::Full,
        );

        assert!(p.contains("fan-in = 40"), "{p}");
        assert!(p.contains("dependency cycle"), "{p}");
    }

    // --- SRV-016: selectable context ---

    const CTX: &str = "// src/a.rs:1-9\nfn target() {}\n\n// src/b.rs:4-6\nfn neighbour() {}";
    const RANGES: &str = "src/a.rs:1-9\nsrc/b.rs:4-6";
    const TARGET: &str = "// src/a.rs:1-9\nfn target() {}";

    /// The default has to stay byte-for-byte what shipped, or every existing
    /// caller silently changes behaviour.
    #[test]
    fn absent_prompt_context_is_full() {
        assert_eq!(PromptContext::parse(None).unwrap(), PromptContext::Full);
        assert_eq!(PromptContext::parse(Some("")).unwrap(), PromptContext::Full);
        assert_eq!(
            PromptContext::parse(Some(" full ")).unwrap(),
            PromptContext::Full
        );
        assert_eq!(
            context_body(PromptContext::Full, CTX, RANGES, Some(TARGET)),
            CTX
        );
    }

    /// The point of the mode: locations, no source bodies.
    #[test]
    fn ranges_mode_carries_locations_not_source() {
        let body = context_body(PromptContext::Ranges, CTX, RANGES, Some(TARGET));
        assert_eq!(body, RANGES);
        assert!(
            !body.contains("fn target()"),
            "source leaked into ranges mode:\n{body}"
        );
        assert!(body.len() < CTX.len(), "ranges should be smaller");
    }

    /// The target is what the agent is being asked to restructure, so its
    /// source is present exactly once — not duplicated by the ranges list.
    #[test]
    fn hybrid_carries_the_target_source_once_plus_ranges() {
        let body = context_body(PromptContext::Hybrid, CTX, RANGES, Some(TARGET));
        assert_eq!(body.matches("fn target() {}").count(), 1, "{body}");
        assert!(
            !body.contains("fn neighbour()"),
            "neighbour source leaked:\n{body}"
        );
        assert!(
            body.contains("src/b.rs:4-6"),
            "neighbour range missing:\n{body}"
        );
    }

    /// Unchecking the target's file leaves no source to embed. Degrading to
    /// ranges beats emitting a section that promises source and has none.
    #[test]
    fn hybrid_without_target_source_degrades_to_ranges() {
        assert_eq!(
            context_body(PromptContext::Hybrid, CTX, RANGES, None),
            RANGES
        );
    }

    /// Silently treating a typo as `full` would hand back an expensive prompt
    /// the caller did not ask for, and hide the mistake.
    #[test]
    fn unknown_prompt_context_is_rejected() {
        let err = PromptContext::parse(Some("summary")).unwrap_err();
        assert!(err.contains("summary"), "{err}");
        assert!(err.contains("full, ranges, or hybrid"), "{err}");
    }

    /// Each preamble must describe what actually follows it.
    #[test]
    fn each_mode_describes_its_own_body() {
        let m = EntityMetrics {
            cyclomatic: Some(30),
            composite_score: 1.0,
            ..Default::default()
        };
        let e = entity(EntityKind::Function, m);
        let t = Thresholds::default();

        let full = build_header(&e, "src/app.rs", &t, PromptContext::Full);
        assert!(
            full.contains("source of the target entity and its immediate neighbours follows"),
            "{full}"
        );

        let ranges = build_header(&e, "src/app.rs", &t, PromptContext::Ranges);
        assert!(ranges.contains("locations, not contents"), "{ranges}");
        assert!(
            !ranges.contains("source of the target entity and its immediate neighbours follows"),
            "{ranges}"
        );

        let hybrid = build_header(&e, "src/app.rs", &t, PromptContext::Hybrid);
        assert!(
            hybrid.contains("target entity's source follows in full"),
            "{hybrid}"
        );
    }

    /// Ratios keep their decimals; counts do not grow a `.00` tail.
    #[test]
    fn values_render_readably() {
        assert_eq!(Finding::fmt_value(31.0), "31");
        assert_eq!(Finding::fmt_value(0.8333), "0.83");
    }

    /// A value sitting exactly on its threshold is *at* the line. Reported as
    /// "over", it makes every other number in the prompt look sloppy — this
    /// showed up live as `max nesting depth = 5 (over the ... threshold of 5)`.
    #[test]
    fn a_value_on_its_threshold_reads_as_at_not_over() {
        let t = Thresholds::default();
        let on_the_line = finding("max nesting depth", "gloss", t.nest.bad, &t.nest);
        assert!(
            on_the_line.render().contains("at the red threshold of 5"),
            "{}",
            on_the_line.render()
        );

        let past_it = finding("max nesting depth", "gloss", t.nest.bad + 2.0, &t.nest);
        assert!(
            past_it.render().contains("over the red threshold of 5"),
            "{}",
            past_it.render()
        );

        let on_the_warn_line = finding("parameter count", "gloss", t.params.warn, &t.params);
        assert!(
            on_the_warn_line
                .render()
                .contains("at the warn threshold of 4"),
            "{}",
            on_the_warn_line.render()
        );
    }
}
