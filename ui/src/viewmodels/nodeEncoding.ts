/**
 * UI-014 — metric-driven node encoding.
 *
 * Two visual channels carry data instead of one:
 *
 *   size  — a magnitude metric (default `loc`), area-proportional
 *   fill  — severity, from the same composite score the Quality panel ranks by
 *
 * Before this, radius switched on `kind_raw` and fill came from a per-kind
 * palette, so at File level — the level auto-level lands on for any
 * non-trivial scope — every node was an identical circle. Every metric was
 * already on the node and none of it reached the picture.
 *
 * This module is the ONE place that answers "how big / what colour is this
 * node". GraphView has two separate node-join sites (initial mount and the
 * incremental live-reload update) and the review found that same duplication
 * caused the theme bug in UI-009 — so both call in here rather than each
 * carrying its own copy of the rules.
 *
 * No DOM, no subscriptions: callers pass the node set and the resolved theme,
 * and the result is a plain lookup object. It does import the two scoring
 * functions from `stores/quality`, which read that module's threshold cache —
 * deliberately, so severity on the canvas is the same number the Quality
 * panel ranks by. Unlike `collapseGraph`, this module is not in the
 * `stores/graph` import cycle, which is why the scoring lives here.
 */

import type { D3Link, D3Node, GraphLevel } from '../types/graph';
import type { ThemeId } from '../stores/settings';
import { compositeScore, scopeCompositeScore, SCORE_TIERS } from '../stores/quality';
import { linkDegrees } from './linkDegrees';
import {
  clampBoost, clampBins, sizeCurveDef, quantiseFraction, binEdges, displayEdges,
  SIZE_BINS_OFF, type SizeCurveDef, type SizeCurveId,
} from './sizeCurve';

// ── Severity ramp ────────────────────────────────────────────────────────
//
// Generated from OKLCH targets and validated with the `dataviz` skill's
// `validate_palette.js --ordinal` against every theme's canvas background
// (`--bg-body`). Both ramps pass lightness-monotone, adjacent ΔL ≥ 0.06 and
// the light-end contrast floor. They FAIL that validator's "single hue"
// check, which is deliberate and the one rule broken on purpose:
//
//   A one-hue sequential ramp is the rule for *magnitude*. This channel
//   encodes *status* (good → critical), which the same skill defines as a
//   fixed reserved green→red scale — and UI-018 requires the canvas and the
//   Quality panel to share one severity scale rather than invent a second.
//   Traffic-light hues are the scale the panel already uses.
//
// Lightness is the load-bearing channel, not hue. Under protanopia the green
// and amber steps collapse to ΔE ~4 whatever hues are chosen — green vs amber
// is precisely the distinction protanopes lack — so the ramps are built to a
// wide, strictly monotone lightness span (0.32 dark / 0.36 light in OKLCH L,
// ~0.08–0.09 per step). That is what satisfies "distinguishable without
// relying on hue alone": desaturate either ramp and the ordering survives.
//
// Two ramps, not one flipped: on a near-black canvas severity has to read as
// "worse = brighter", and on the light theme as "worse = darker". Both are
// the same rule — worse = further from the page — but they are separately
// validated steps, not an inversion.

/** Dark canvases (midnight, obsidian, nord, llvator). Worse = brighter.
 *  Worst contrast is 3.15:1, on nord's #2e3440 — the lightest dark body. */
export const SEVERITY_RAMP_DARK = ['#009439', '#8B9E00', '#D6A200', '#FFAF7E', '#FFD2CC'];

/** The `light` theme. Worse = darker. The pale green end sits at 2.14:1,
 *  which clears the ordinal light-end floor (2.0:1) rather than the 3:1 mark
 *  floor — correct for the low end of a ramp, and every node also carries a
 *  `--text` stroke ring, so the circle is never carried by fill alone. */
export const SEVERITY_RAMP_LIGHT = ['#31C35A', '#829400', '#8C6800', '#863E00', '#780007'];

/**
 * Composite-score breakpoints the five ramp steps span.
 *
 * Anchored on `SCORE_TIERS` so the canvas and the Quality panel agree on
 * where ok becomes warn and warn becomes bad. The two extra stops subdivide
 * the outer bands: `bad` is unbounded above and would otherwise flatten every
 * problem file to one colour, which is the whole reason this channel is
 * continuous rather than a three-step tier.
 *
 *   step 0  score ≤ 0.25          ok, clean
 *   step 1  0.25 < score ≤ 0.50   ok  ← SCORE_TIERS.ok boundary
 *   step 2  0.50 < score ≤ 1.00   warn ← SCORE_TIERS.warn boundary
 *   step 3  1.00 < score ≤ 1.40   bad
 *   step 4  score > 1.40          bad, severe
 */
export const SEVERITY_STOPS = [0.25, SCORE_TIERS.ok, SCORE_TIERS.warn, 1.4] as const;

export function severityRamp(theme: ThemeId): string[] {
  return theme === 'light' ? SEVERITY_RAMP_LIGHT : SEVERITY_RAMP_DARK;
}

/** Index into the ramp for a composite score. */
export function severityStep(score: number): number {
  for (let i = 0; i < SEVERITY_STOPS.length; i++) {
    if (score <= SEVERITY_STOPS[i]) return i;
  }
  return SEVERITY_STOPS.length;
}

// ── "No data" ────────────────────────────────────────────────────────────
//
// Ghosts, builtins, Parameter, Branch and Loop carry no metrics. Rendering
// them at the bottom of the ramp would say "zero complexity", which is a
// different and false claim. (The Elevator and ansible-deploy kinds have no
// metrics either, but a graph made of them takes the `kindOnly` path below
// and never reaches this fill at all.)
//
// The distinguishing channel is fill-opacity, not colour. A neutral grey at
// full opacity is only distinct from a low ramp step by hue, and collapses
// into it in greyscale — exactly the failure mode this is meant to avoid. A
// half-transparent fill reads as *hollow* against any background, in any
// theme, under any colour vision, and nothing else on a node uses
// fill-opacity, so it composes with selection, hover, search and diff (all of
// which use stroke and element opacity).
export const NO_DATA_FILL = '#7A8794';
export const NO_DATA_FILL_OPACITY = 0.35;
/** "Unknown" must not read as "smallest" either — so it draws a step above
 *  the floor rather than at it. At the default ×1 size scale this is 9px;
 *  `radiusRange` keeps that position as the scale moves. */
export const NO_DATA_RADIUS = 9;

// ── Size scale ───────────────────────────────────────────────────────────

/** The radius range at the default ×1 scale.
 *
 *  Exported because the legend has to draw its sample dots on the *same*
 *  scale as the canvas. It renders them shrunk by one factor (the widest
 *  radius in play → the panel's dot budget), which preserves every ratio; a
 *  per-dot clamp does not, and that is exactly how the legend ended up
 *  showing three identical circles.
 *
 *  UI-106 made the top of the range a multiple of `R_MAX` rather than `R_MAX`
 *  itself, so the panel's shrink factor now comes off `SizeLegend.rMax` —
 *  a constant denominator would rescale the dots correctly only at ×1. */
export const R_MIN = 7;
export const R_MAX = 34;

/** Where "no data" sits inside the range, as a fraction of the span.
 *
 *  Held as a fraction rather than the flat 9px it used to be so it keeps its
 *  meaning under a size scale: the point of that radius is that it reads as
 *  neither the smallest node nor an average one, and at ×3 a fixed 9px would
 *  be indistinguishable from the floor — "unknown" would have quietly become
 *  "smallest" again, which is the exact claim it exists to avoid. */
const NO_DATA_SPAN_FRACTION = (9 - R_MIN) / (R_MAX - R_MIN);

/** The drawn radius range at a given size scale. The floor does not move: a
 *  node still has to be clickable and carry its kind code. See `clampBoost`
 *  for why the multiplier applies to the span. */
export function radiusRange(boost: number): { rMin: number; rMax: number; rNoData: number } {
  const rMax = R_MIN + (R_MAX - R_MIN) * clampBoost(boost);
  return { rMin: R_MIN, rMax, rNoData: R_MIN + (rMax - R_MIN) * NO_DATA_SPAN_FRACTION };
}

/** Kind-based radius: the pre-UI-014 behaviour, kept as the `kind` size
 *  channel and as the encoding for metric-free graphs (see
 *  `isMetricFreeKind`). */
export function kindRadius(kindRaw: string): number {
  switch (kindRaw) {
    case 'Class': case 'Dataclass': case 'AbstractClass': case 'Struct': case 'Interface': case 'Trait': return 15;
    case 'Module': case 'File': return 18;
    case 'Function': case 'Method': return 10;
    case 'Parameter': return 7;
    case 'Branch': case 'Loop': return 6;
    // Elevator hierarchy (size encodes vertical level):
    // Extensions > Categories > Features > Concepts > Functionalities > UiPages.
    case 'Extension': return 22;
    case 'Category': return 19;
    case 'Feature': return 16;
    case 'Concept': return 13;
    case 'Functionality': return 11;
    case 'UiPage': return 9;
    // ansible-deploy containment, three tiers deep. Same tiering as
    // `utils/kindPriority`, which already answers "which end of this edge is
    // the parent" for these kinds — size and arrow direction should not
    // disagree about what contains what.
    //   Playbook ⊃ Role, HostGroup ⊃ DeploymentSet ⊃ DeploymentEntry,
    //   TemplateFile ⊃ K8sResource.
    case 'Playbook': case 'HostGroup': return 20;
    case 'Role': case 'DeploymentSet': case 'TemplateFile': return 15;
    case 'DeploymentEntry': case 'K8sResource': case 'HelmChart': return 11;
    default: return 12;
  }
}

/**
 * Kinds that carry no code metrics — Elevator (`.elv`) domain kinds and the
 * ansible-deploy topology kinds.
 *
 * These deliberately use size to encode hierarchy depth. A metric encoding
 * says something false about them: the Elevator kinds have no metrics
 * object at all and render as uniform "no data", while the ansible-deploy
 * parser builds every entity on a single-line span and never touches
 * `metrics`, so `populate_composite_scores` scores the whole graph at ~0 and
 * every node comes out at the clean end of the severity ramp — a confident
 * claim rather than a missing one.
 *
 * Neither is a code-quality view. Both keep kind-based size and the kind
 * palette, and the channel selector is hidden rather than offering controls
 * that would do nothing. Decided explicitly for Elevator (UI-014 HITL) and
 * extended to ansible-deploy on the same reasoning.
 */
const METRIC_FREE_KINDS = new Set([
  'Extension', 'Category', 'Feature', 'Concept', 'Functionality', 'UiPage',
  'Playbook', 'Role', 'HostGroup', 'DeploymentSet', 'DeploymentEntry',
  'TemplateFile', 'K8sResource', 'HelmChart',
]);

export function isMetricFreeKind(kindRaw: string): boolean {
  return METRIC_FREE_KINDS.has(kindRaw);
}

/**
 * True when a single node carries no code metrics.
 *
 * Kind alone is not enough on the ansible-deploy side. That parser also
 * emits plain `Variable` entities for every `group_vars` and Jinja template
 * variable — 313 of 786 nodes on the deploy repo this was found on — and
 * `Variable` is a real code kind everywhere else, so it can't go in the set.
 * The `ansible` tag is the parser's own marker (`new_entity` in
 * `src/parser/ansible/mod.rs` stamps it on every entity it builds), which
 * makes it the exact signal rather than a proxy for one.
 */
export function isMetricFreeNode(d: D3Node): boolean {
  return isMetricFreeKind(d.kind_raw) || (d.tags?.includes('ansible') ?? false);
}

/** True when the graph is a domain or topology view and metric encoding does
 *  not apply. Majority test rather than `some`, so one stray kind in a mixed
 *  graph doesn't disable metric encoding for real code — a deploy repo with a
 *  Jenkinsfile in it still measures the Jenkinsfile. */
export function isMetricFreeGraph(nodes: D3Node[]): boolean {
  if (nodes.length === 0) return false;
  let n = 0;
  for (const d of nodes) if (isMetricFreeNode(d)) n++;
  return n * 2 > nodes.length;
}

// ── Channels ─────────────────────────────────────────────────────────────

export type SizeChannel = 'loc' | 'churn' | 'degree' | 'coupling' | 'methodCount' | 'wmc' | 'cyclomatic' | 'pagerank' | 'kind';
export type ColorChannel = 'severity' | 'kind';

/**
 * What a channel needs that a node doesn't carry on its own.
 *
 * Two channels are like this, for the same underlying reason and from
 * different directions. `degree` is a property of the *drawn graph* rather
 * than of the entity, and changes with the aggregation level and with which
 * scopes are open — `collapseGraph` rebuilds the links on every one of those.
 * `churn` is a property of the *comparison*, which exists only while a diff
 * is loaded and is measured from two source maps this module has no business
 * fetching. Neither can be read off `d.metrics`, and both are passed in.
 */
export interface SizeContext {
  /** Links incident to a node in the graph being drawn. */
  degree(id: string): number;
  /** Lines a loaded diff added or removed here — see `viewmodels/diffChurn`.
   *  Undefined when the node was not measurable, which is *not* zero. */
  churn(d: D3Node): number | undefined;
}

export interface SizeChannelDef {
  id: SizeChannel;
  label: string;
  /** Shown in the legend under the ramp. */
  unit: string;
  /** Aggregation levels at which this metric has a meaningful value. A
   *  metric absent from a level is not offered there rather than silently
   *  reading zero — `collapseGraph` genuinely has no file/module rollup for
   *  the per-callable complexity metrics. */
  levels: GraphLevel[];
  /** True for a channel that only means anything while a diff is loaded. It
   *  is withheld from the picker the rest of the time, on the same rule as a
   *  metric with no rollup at the current level: a channel that would read
   *  "nothing changed" over a repo nobody is comparing is a confident answer
   *  to a question that was never asked. */
  requiresDiff?: boolean;
  value(d: D3Node, ctx: SizeContext): number | undefined;
}

const ALL_LEVELS: GraphLevel[] = ['entity', 'file', 'module'];
/** Metrics `collapseGraph`'s `scopeToEntityMetrics` leaves undefined. */
const ENTITY_ONLY: GraphLevel[] = ['entity'];

export const SIZE_CHANNELS: SizeChannelDef[] = [
  {
    id: 'loc', label: 'Lines of code', unit: 'LOC', levels: ALL_LEVELS,
    value: (d) => d.metrics?.loc,
  },
  // Only ever offered in Current Changes / Compare Commits, where it is the
  // one channel that answers "which of these circles is most of the change".
  // Every other channel describes the code as it stands and says nothing
  // about how much of it moved, so a diff drew twenty nodes of identical
  // weight and left the reader to open each one.
  //
  // Added + removed, deliberately, not the net `loc` delta the diff already
  // ships: see `viewmodels/diffChurn` for why the net figure draws the
  // largest rewrite as the smallest circle.
  {
    id: 'churn', label: 'Lines changed', unit: 'lines added + removed',
    levels: ALL_LEVELS, requiresDiff: true,
    // Undefined passes through as "no data", so an entity whose before-source
    // could not be read draws hollow rather than joining the unchanged at the
    // bottom of the ramp.
    value: (d, ctx) => ctx.churn(d),
  },
  // Two connectivity channels, deliberately, because they answer different
  // questions and a reader comparing them learns something:
  //
  //   Relationships — edges touching this node *in the graph being drawn*.
  //     Every relationship kind counts (containment, inheritance, calls), it
  //     follows the aggregation level and the open scopes, and it needs no
  //     metrics — so ghosts, parameters and branches size honestly too.
  //
  //   Coupling — the backend's `fan_in + fan_out`: distinct *dependency*
  //     neighbours over the whole analysis. Containment and inheritance are
  //     excluded on purpose (`populate_coupling_metrics`), and it does not
  //     shrink when the view narrows.
  //
  // A node that is large under Relationships and small under Coupling holds
  // a lot of children; large under both is a genuine hub.
  {
    id: 'degree', label: 'Relationships', unit: 'links', levels: ALL_LEVELS,
    // Never undefined: an isolated node has zero relationships, which is a
    // fact about it, not missing data — it should draw at R_MIN rather than
    // at the "no data" radius.
    value: (d, ctx) => ctx.degree(d.id),
  },
  {
    id: 'coupling', label: 'Coupling', unit: 'edges', levels: ALL_LEVELS,
    value: (d) => (d.metrics ? d.metrics.fan_in + d.metrics.fan_out : undefined),
  },
  {
    id: 'methodCount', label: 'Method count', unit: 'methods', levels: ALL_LEVELS,
    value: (d) => d.metrics?.method_count,
  },
  {
    id: 'wmc', label: 'WMC', unit: 'WMC', levels: ENTITY_ONLY,
    value: (d) => d.metrics?.wmc,
  },
  {
    id: 'cyclomatic', label: 'Cyclomatic', unit: 'CC', levels: ENTITY_ONLY,
    value: (d) => d.metrics?.cyclomatic,
  },
  {
    id: 'pagerank', label: 'PageRank', unit: '×10⁻³', levels: ENTITY_ONLY,
    value: (d) => (d.metrics?.pagerank == null ? undefined : d.metrics.pagerank * 1000),
  },
  {
    id: 'kind', label: 'Entity kind', unit: '', levels: ALL_LEVELS,
    value: () => undefined,
  },
];

export const COLOR_CHANNELS: { id: ColorChannel; label: string }[] = [
  { id: 'severity', label: 'Severity' },
  { id: 'kind', label: 'Entity kind' },
];

/** What the picker offers: the channels that mean something at this level,
 *  minus the diff-only ones when there is no diff to measure. */
export function sizeChannelsFor(
  level: GraphLevel,
  opts: { diffLoaded?: boolean } = {},
): SizeChannelDef[] {
  return SIZE_CHANNELS.filter(
    (c) => c.levels.includes(level) && (!c.requiresDiff || opts.diffLoaded === true),
  );
}

export function sizeChannelDef(id: SizeChannel): SizeChannelDef {
  return SIZE_CHANNELS.find((c) => c.id === id) ?? SIZE_CHANNELS[0];
}

// ── Severity source ──────────────────────────────────────────────────────

/**
 * Composite score for a node, or undefined when it has no metrics.
 *
 * Mirrors `qualityRows`: prefer the backend's `composite_score`, fall back to
 * the front-end `compositeScore`. Same numbers as the Quality panel, so
 * cross-checking a node against the ranked table agrees.
 */
export function severityScore(d: D3Node): number | undefined {
  const m = d.metrics;
  if (!m) return undefined;
  if (m.composite_score != null) return m.composite_score;
  // File/Module nodes are scope rollups, and the two formulas are not
  // interchangeable. `compositeScore` is the *per-entity* one; run on
  // `collapseGraph`'s promoted fields (entity_count posing as field_count,
  // callable_count as method_count) it would return a confident number that
  // means nothing. Score them the way `fileRows` / `moduleRows` do, off the
  // raw rollup, so canvas and Quality panel never disagree.
  if (d.kind_raw === 'File' || d.kind_raw === 'Module') {
    return d.scope_metrics
      ? scopeCompositeScore(d.scope_metrics, d.kind_raw === 'Module')
      : undefined;
  }
  return compositeScore(m, d.kind_raw);
}

// ── The encoding ─────────────────────────────────────────────────────────

export interface EncodingOptions {
  sizeChannel: SizeChannel;
  colorChannel: ColorChannel;
  /** How a value maps onto the radius range — see `viewmodels/sizeCurve`.
   *  Optional so every existing caller (and every test) keeps the
   *  area-proportional behaviour this shipped with. */
  sizeCurve?: SizeCurveId;
  /** Multiplier on the radius *span*. Optional, defaults to ×1. */
  sizeBoost?: number;
  /** Number of size groups to snap radii to, or `SIZE_BINS_OFF` for the
   *  continuous scale. Optional, defaults to continuous. */
  sizeBins?: number;
  /** The graph's links — the domain of the `degree` channel. Optional so a
   *  caller that never selects that channel doesn't have to supply one; the
   *  channel then reads zero everywhere and the legend withholds itself,
   *  rather than the build throwing. */
  links?: D3Link[];
  /** The `churn` channel's domain — lines a loaded diff moved at this node.
   *  Absent means no diff, which is also what makes the channel unselectable;
   *  supplying nothing therefore falls back to kind sizing rather than
   *  painting every circle as "no data". */
  churn?: (d: D3Node) => number | undefined;
  level: GraphLevel;
  theme: ThemeId;
  /** Per-kind palette, injected so this module stays free of the type
   *  barrel's colour table and remains unit-testable. */
  kindColors: Record<string, string>;
  kindColorFallback: string;
}

export interface SizeLegend {
  label: string;
  unit: string;
  /**
   * Ascending, and what a row means depends on `bins`.
   *
   * Continuous: three sample values off a smooth ramp — `value` is a point on
   * it and `from`/`to` are absent, because a sample is not a class and a
   * legend that drew bands around one would be inventing boundaries.
   *
   * Grouped: one row per group, `from`/`to` the value band it covers (`to`
   * null on the open-topped last one), `value` its lower edge. Every radius
   * here is one the canvas actually draws — that is the difference the mode
   * is for.
   */
  stops: { value: number; radius: number; from?: number; to?: number | null }[];
  /** Group count when sizes are quantised, `SIZE_BINS_OFF` when continuous. */
  bins: number;
  /** What the active curve does to the picture, for the note under the ramp.
   *  Was the hardcoded sentence "area scales with the value", which described
   *  one of the five curves now on offer. */
  hint: string;
  /** Largest radius the canvas will draw under this encoding — the
   *  denominator the panel shrinks its sample dots by. Not a constant since
   *  UI-106: it moves with the size scale. */
  rMax: number;
}

export interface ColorLegend {
  kind: 'severity' | 'kind';
  /** Severity only: ramp swatches with the score band each covers. */
  steps?: { color: string; from: number; to: number | null; tier: 'ok' | 'warn' | 'bad' }[];
}

export interface NodeEncoding {
  radius(d: D3Node): number;
  fill(d: D3Node): string;
  fillOpacity(d: D3Node): number;
  /** Ink for the two-letter kind code drawn inside the circle. */
  labelInk(d: D3Node): string;
  /** Largest radius in play — the force simulation's collision radius and
   *  the hit-test padding both derive from this. */
  maxRadius: number;
  /** True when metric encoding is suppressed (Elevator / ansible-deploy). */
  kindOnly: boolean;
  sizeLegend: SizeLegend | null;
  colorLegend: ColorLegend;
}

/** Relative luminance, for choosing readable ink on a fill. */
function luminance(hex: string): number {
  const h = hex.replace('#', '');
  if (h.length !== 6) return 0;
  const ch = [0, 2, 4].map((i) => {
    const c = parseInt(h.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
}

/** White or near-black, whichever contrasts better with `fill`.
 *
 *  Previously the kind code was always white at 0.9 alpha, which assumed
 *  every node fill was a saturated mid-tone. The severity ramp's bright end
 *  breaks that assumption (and so, already, did `Macro` #FFEB3B and the
 *  Elevator `Functionality` #FFE082 — white on those was never readable). */
function inkFor(fill: string): string {
  return luminance(fill) > 0.36 ? 'rgba(20,20,20,0.92)' : 'rgba(255,255,255,0.92)';
}

/** Round to a readable legend number: 1/2/5 × a power of ten. */
function niceValue(v: number): number {
  if (v <= 0) return 0;
  const mag = 10 ** Math.floor(Math.log10(v));
  const n = v / mag;
  const step = n >= 5 ? 5 : n >= 2 ? 2 : 1;
  return step * mag;
}

/**
 * Fractions of the *radius* range the legend's two lower samples sit at.
 *
 * Picked in radius space, not value space. The pre-UI-014 0.15/0.5 of the
 * maximum landed at radius 17.4 and 26.1 out of a 7–34 range: three samples
 * all crowded into the top half, describing none of the small nodes that make
 * up most of a real graph. A quarter and two-thirds of the way up the radius
 * range is what makes three visibly different dots.
 *
 * Held as radii is also what lets the curve become a control (UI-106): the
 * value each dot stands for is now `curve.invert(fraction)`, so the samples
 * stay spread across the drawn range whichever curve is selected. Under the
 * default `area` curve they invert to 0.04 and 0.400 of vMax — the exact two
 * fractions this used to hardcode.
 */
const LEGEND_RADIUS_FRACTIONS = [0.2, 0.6325];

/**
 * Sample values the size legend draws, ascending.
 *
 * `niceValue` rounds each sample to something readable, which can collapse
 * two stops onto one number on a small domain (a graph whose largest file is
 * 3 LOC) — hence the dedupe, so the legend never shows the same value twice
 * at two different sizes. The top stop is the real maximum rather than a
 * rounded one, so the legend states the true range.
 */
function legendStops(
  vMax: number,
  curve: SizeCurveDef,
  radiusFor: (v: number) => number,
): { value: number; radius: number }[] {
  const samples = LEGEND_RADIUS_FRACTIONS.map((t) => curve.invert(t))
    .map((f) => niceValue(vMax * f))
    // Never propose a fractional count of lines or edges just because the
    // domain is small; 1 is the floor for any metric measured in whole units.
    .map((v) => (vMax >= 1 ? Math.max(1, Math.round(v)) : v));

  const stops: { value: number; radius: number }[] = [];
  for (const v of [...samples, Math.round(vMax)]) {
    if (v <= 0 || stops.some((s) => s.value === v)) continue;
    stops.push({ value: v, radius: radiusFor(v) });
  }
  // `radiusFor(vMax)` is R_MAX by construction, but the rounded top value can
  // sit a hair above vMax and clamp to the same radius — either way the last
  // stop is the largest circle on the canvas.
  return stops;
}

/**
 * One legend row per size group (UI-110), ascending.
 *
 * Not a variant of `legendStops`: the two describe different claims. A sample
 * is a point on a ramp and says "a node this big is about this value"; a group
 * is a *class*, and its row has to name the band it covers or the reader
 * cannot tell which group a node fell in. Boundaries come from `binEdges` —
 * the same midpoints `quantiseFraction` actually switches at, pushed back
 * through the curve — so the legend can never name an edge the canvas does not
 * draw.
 *
 * Rows are kept even when `niceValue` rounds two adjacent edges onto one
 * number, unlike the continuous stops which dedupe. A group that exists on the
 * canvas has to appear here: dropping it would leave a radius on screen with
 * nothing in the legend explaining it, and on a tiny domain (largest file 3
 * LOC at 8 groups) that is most of them.
 */
function legendGroups(
  vMax: number,
  curve: SizeCurveDef,
  bins: number,
  radiusAt: (t: number) => number,
): { value: number; radius: number; from: number; to: number | null }[] {
  const steps = bins - 1;
  // `displayEdges`, not `niceValue`: a boundary has to stay distinct from its
  // neighbour, which the 1/2/5 ladder does not guarantee — at 8 groups on a
  // 2,520-line domain it printed `1,000–1,000` for a band nothing can be in.
  const edges = displayEdges(binEdges(curve, bins).map((f) => vMax * f));
  return Array.from({ length: bins }, (_, i) => ({
    // The lower edge of the band, and 0 for the first group — which starts at
    // whatever the smallest node is, not at a boundary.
    value: i === 0 ? 0 : edges[i - 1],
    from: i === 0 ? 0 : edges[i - 1],
    // Open-topped last group: `vMax` is the largest node drawn, not a ceiling
    // the metric respects, and the same graph one commit later has a bigger
    // one. `> 500` stays true; `500–2,455` would not.
    to: i === bins - 1 ? null : edges[i],
    radius: radiusAt(i / steps),
  }));
}

export function buildNodeEncoding(nodes: D3Node[], opts: EncodingOptions): NodeEncoding {
  const { colorChannel, level, theme, kindColors, kindColorFallback } = opts;
  const kindOnly = isMetricFreeGraph(nodes);

  // A metric-free graph, or a size channel that isn't meaningful at this
  // aggregation level, falls back to kind sizing rather than rendering
  // everything as "no data".
  const def = sizeChannelDef(opts.sizeChannel);
  const metricSizing = !kindOnly
    && def.id !== 'kind'
    && def.levels.includes(level)
    // A diff-only channel with no diff behind it. The picker already withholds
    // it, but a channel is persisted across reloads and restored from a saved
    // view, so the encoding has to be able to refuse it on its own.
    && (!def.requiresDiff || opts.churn != null);
  const useSeverity = !kindOnly && colorChannel === 'severity';

  // Counted once per encoding, only for the channel that reads it — an O(E)
  // pass has no business running when size is on `loc`.
  const degrees = metricSizing && def.id === 'degree'
    ? linkDegrees(opts.links ?? [])
    : null;
  const ctx: SizeContext = {
    degree: (id) => degrees?.get(id) ?? 0,
    churn: (d) => opts.churn?.(d),
  };

  // Size domain from the nodes actually in play, so the scale adapts to the
  // scope rather than to some global maximum the user can't see.
  let vMax = 0;
  if (metricSizing) {
    for (const d of nodes) {
      const v = def.value(d, ctx);
      if (v != null && isFinite(v) && v > vMax) vMax = v;
    }
  }

  // Both size controls (UI-106). The curve decides how the value range is
  // distributed over the radius range; the scale decides how wide that radius
  // range is. They are independent on purpose — a log curve on a 27px span
  // and a √ curve on an 80px span are different answers to "I can't tell
  // these circles apart", and a reader gets to pick either or both.
  const curve = sizeCurveDef(opts.sizeCurve ?? 'area');
  const boost = clampBoost(opts.sizeBoost ?? 1);
  const bins = clampBins(opts.sizeBins ?? SIZE_BINS_OFF);
  const { rMin, rMax, rNoData } = radiusRange(boost);

  /** A radius fraction — post-curve, post-quantisation — as a radius. */
  const radiusAt = (t: number): number => rMin + (rMax - rMin) * t;

  const radiusFor = (v: number): number => {
    if (vMax <= 0) return rNoData;
    // Quantisation is the last step, after the curve: the groups are evenly
    // spaced in RADIUS so consecutive ones always differ by a visible number
    // of pixels, and where that lands in value space is the curve's business.
    // Binning the value instead would overrule the curve and let two groups
    // draw a pixel apart.
    const t = quantiseFraction(curve.f(Math.max(0, Math.min(v, vMax)) / vMax), bins);
    return radiusAt(t);
  };

  const radius = (d: D3Node): number => {
    // Kind sizing answers to the scale too. It is a fixed table of radii
    // rather than a range, so the multiplier applies to the radius directly
    // — "make the circles bigger" is the whole request, and a control that
    // did nothing on the one channel with no metric behind it (and on every
    // Elevator / ansible-deploy graph, which have no other) would be a
    // control that lies.
    if (!metricSizing) return kindRadius(d.kind_raw) * boost;
    const v = def.value(d, ctx);
    if (v == null || !isFinite(v)) return rNoData;
    return radiusFor(v);
  };

  const ramp = severityRamp(theme);
  const fill = (d: D3Node): string => {
    if (!useSeverity) return kindColors[d.kind_raw] || kindColorFallback;
    const s = severityScore(d);
    if (s == null || !isFinite(s)) return NO_DATA_FILL;
    return ramp[severityStep(s)];
  };

  const fillOpacity = (d: D3Node): number => {
    if (!useSeverity) return 1;
    const s = severityScore(d);
    return s == null || !isFinite(s) ? NO_DATA_FILL_OPACITY : 1;
  };

  // Precompute the ink per distinct fill — there are at most a few dozen.
  const inkCache = new Map<string, string>();
  const labelInk = (d: D3Node): string => {
    const f = fill(d);
    let ink = inkCache.get(f);
    if (ink === undefined) { ink = inkFor(f); inkCache.set(f, ink); }
    return ink;
  };

  let maxRadius = rNoData;
  for (const d of nodes) {
    const r = radius(d);
    if (r > maxRadius) maxRadius = r;
  }

  const sizeLegend: SizeLegend | null = metricSizing && vMax > 0
    ? {
      label: def.label,
      unit: def.unit,
      stops: bins === SIZE_BINS_OFF
        ? legendStops(vMax, curve, radiusFor)
        : legendGroups(vMax, curve, bins, radiusAt),
      bins,
      hint: curve.hint,
      rMax,
    }
    : null;

  const colorLegend: ColorLegend = useSeverity
    ? {
      kind: 'severity',
      steps: ramp.map((color, i) => ({
        color,
        from: i === 0 ? 0 : SEVERITY_STOPS[i - 1],
        to: i < SEVERITY_STOPS.length ? SEVERITY_STOPS[i] : null,
        tier: (i <= 1 ? 'ok' : i === 2 ? 'warn' : 'bad') as 'ok' | 'warn' | 'bad',
      })),
    }
    : { kind: 'kind' };

  return { radius, fill, fillOpacity, labelInk, maxRadius, kindOnly, sizeLegend, colorLegend };
}
