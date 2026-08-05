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
/** Fixed mid radius: "unknown" must not read as "smallest" either. */
export const NO_DATA_RADIUS = 9;

// ── Size scale ───────────────────────────────────────────────────────────

/** Area, not radius, should read as magnitude — hence sqrt.
 *
 *  Exported because the legend has to draw its sample dots on the *same*
 *  scale as the canvas. It renders them shrunk by one constant factor
 *  (`R_MAX` → the panel's dot budget), which preserves every ratio; a
 *  per-dot clamp does not, and that is exactly how the legend ended up
 *  showing three identical circles. */
export const R_MIN = 7;
export const R_MAX = 34;

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

export type SizeChannel = 'loc' | 'degree' | 'coupling' | 'methodCount' | 'wmc' | 'cyclomatic' | 'pagerank' | 'kind';
export type ColorChannel = 'severity' | 'kind';

/**
 * What a channel needs that a node doesn't carry on its own.
 *
 * `degree` is the only such channel: it is a property of the *drawn graph*,
 * not of the entity, so it can't be read off `d.metrics` like every other
 * one. Passed in rather than precomputed onto the node because the count
 * changes with the aggregation level and with which scopes are open —
 * `collapseGraph` rebuilds the links on every one of those changes.
 */
export interface SizeContext {
  /** Links incident to a node in the graph being drawn. */
  degree(id: string): number;
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

export function sizeChannelsFor(level: GraphLevel): SizeChannelDef[] {
  return SIZE_CHANNELS.filter((c) => c.levels.includes(level));
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
  /** The graph's links — the domain of the `degree` channel. Optional so a
   *  caller that never selects that channel doesn't have to supply one; the
   *  channel then reads zero everywhere and the legend withholds itself,
   *  rather than the build throwing. */
  links?: D3Link[];
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
  /** Ascending sample values with the radius each renders at. */
  stops: { value: number; radius: number }[];
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
 * Sample values the size legend draws, ascending.
 *
 * The fractions are picked in *radius* space, not value space. Size is
 * sqrt-scaled, so the old 0.15/0.5 of the maximum landed at radius 17.4 and
 * 26.1 out of a 7–34 range: three samples all crowded into the top half,
 * describing none of the small nodes that make up most of a real graph.
 * These land near a quarter and two-thirds of the way up the radius range
 * instead, so the legend spans what the canvas actually draws.
 *
 * `niceValue` rounds each sample to something readable, which can collapse
 * two stops onto one number on a small domain (a graph whose largest file is
 * 3 LOC) — hence the dedupe, so the legend never shows the same value twice
 * at two different sizes. The top stop is the real maximum rather than a
 * rounded one, so the legend states the true range.
 */
function legendStops(
  vMax: number,
  radiusFor: (v: number) => number,
): { value: number; radius: number }[] {
  const samples = [0.04, 0.4]
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

export function buildNodeEncoding(nodes: D3Node[], opts: EncodingOptions): NodeEncoding {
  const { colorChannel, level, theme, kindColors, kindColorFallback } = opts;
  const kindOnly = isMetricFreeGraph(nodes);

  // A metric-free graph, or a size channel that isn't meaningful at this
  // aggregation level, falls back to kind sizing rather than rendering
  // everything as "no data".
  const def = sizeChannelDef(opts.sizeChannel);
  const metricSizing = !kindOnly && def.id !== 'kind' && def.levels.includes(level);
  const useSeverity = !kindOnly && colorChannel === 'severity';

  // Counted once per encoding, only for the channel that reads it — an O(E)
  // pass has no business running when size is on `loc`.
  const degrees = metricSizing && def.id === 'degree'
    ? linkDegrees(opts.links ?? [])
    : null;
  const ctx: SizeContext = { degree: (id) => degrees?.get(id) ?? 0 };

  // Size domain from the nodes actually in play, so the scale adapts to the
  // scope rather than to some global maximum the user can't see.
  let vMax = 0;
  if (metricSizing) {
    for (const d of nodes) {
      const v = def.value(d, ctx);
      if (v != null && isFinite(v) && v > vMax) vMax = v;
    }
  }

  const radiusFor = (v: number): number => {
    if (vMax <= 0) return NO_DATA_RADIUS;
    const t = Math.sqrt(Math.max(0, Math.min(v, vMax)) / vMax);
    return R_MIN + (R_MAX - R_MIN) * t;
  };

  const radius = (d: D3Node): number => {
    if (!metricSizing) return kindRadius(d.kind_raw);
    const v = def.value(d, ctx);
    if (v == null || !isFinite(v)) return NO_DATA_RADIUS;
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

  let maxRadius = NO_DATA_RADIUS;
  for (const d of nodes) {
    const r = radius(d);
    if (r > maxRadius) maxRadius = r;
  }

  const sizeLegend: SizeLegend | null = metricSizing && vMax > 0
    ? { label: def.label, unit: def.unit, stops: legendStops(vMax, radiusFor) }
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
