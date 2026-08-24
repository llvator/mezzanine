/**
 * UI-106 — the shape of the value → radius mapping, as a chooseable curve.
 *
 * `nodeEncoding` normalises a node's metric to `u = value / vMax` and turns
 * that into a radius. Until this module there was exactly one way to do it —
 * `√u`, area-proportional — and that is the right *default* and the wrong
 * *only option*:
 *
 *   A real repo's `loc` is heavily skewed. One 4,000-line file sets vMax and
 *   the other nine hundred files sit under 400, i.e. u ≤ 0.1, i.e. inside the
 *   bottom third of the radius range under √. Every circle a reader is
 *   actually comparing is drawn within a few pixels of every other. The
 *   encoding is faithful and useless at the same time.
 *
 * So the curve becomes a control. Each entry is a monotone map of [0,1] onto
 * [0,1] with `f(0) = 0` and `f(1) = 1` — the endpoints are fixed so that
 * "smallest metric" and "largest metric" always mean R_MIN and R_MAX and only
 * the *distribution between them* changes. A curve cannot make a small node
 * bigger than a large one; it decides which part of the range gets the pixels.
 *
 * ## Why every curve carries its inverse
 *
 * The size legend samples the ramp at fixed fractions of the *radius* range
 * (see `legendStops`) — a quarter and two-thirds of the way up — because that
 * is what makes the three dots visibly different sizes. Those are radii, and
 * the legend has to print *values*, so it needs `f⁻¹`. Deriving the samples
 * in value space instead is exactly the bug the pre-UI-106 comment in
 * `legendStops` describes: the fractions 0.04 / 0.4 were hand-picked to land
 * well under √ and land nowhere in particular under anything else.
 *
 * Keeping `invert` next to `f` — rather than numerically searching for it at
 * the call site — is also what makes the pair testable as a round-trip, which
 * is the one property a wrong constant in either direction breaks.
 *
 * No DOM, no stores, no imports: this module exists separately from
 * `nodeEncoding` for the same reason `linkDegrees` does — so a unit test can
 * reach it without pulling `stores/quality` in behind it.
 */

export type SizeCurveId = 'area' | 'linear' | 'log' | 'exponential' | 'sigmoid';

export interface SizeCurveDef {
  id: SizeCurveId;
  label: string;
  /**
   * What this curve does to the picture, in the legend's own voice. Printed
   * under the size ramp where "area scales with the value" used to be
   * hardcoded — that sentence was true of one curve and is now one of five.
   */
  hint: string;
  /** Value fraction (0–1) → radius fraction (0–1). Monotone, f(0)=0, f(1)=1. */
  f(u: number): number;
  /** The inverse: radius fraction → value fraction. */
  invert(t: number): number;
}

const clamp01 = (x: number): number => (x < 0 ? 0 : x > 1 ? 1 : Number.isFinite(x) ? x : 0);

// ── Curve constants ──────────────────────────────────────────────────────
//
// Each curve has one shape parameter, and the value is chosen so the curve is
// meaningfully *different from √* — an option that does roughly what the
// default already did is a control that lies about having an effect.
//
//   LOG_K = 99 makes `log` a clean two-decade scale: f(u) = log₁₀(1+99u)/2.
//     At u = 0.01 it gives 0.15 against √'s 0.10, and at u = 0.1 it gives
//     0.52 against √'s 0.32. A smaller k (24 was tried) puts the curve
//     *under* √ at the low end, which is the opposite of the reason anyone
//     reaches for a log scale.
//
//   EXP_K = 4 keeps the bottom 60% of the value range inside the bottom 20%
//     of the radius range. That is the point of this curve: on a graph where
//     the question is "which two files are the monsters", everything else is
//     supposed to recede.
//
//   SIG_K = 8 about the midpoint gives an S with usable flat ends — steep
//     enough that the middle of the range spreads out, shallow enough that
//     the top and bottom don't fully collapse.
const LOG_K = 99;
const EXP_K = 4;
const SIG_K = 8;

const EXP_SPAN = Math.exp(EXP_K) - 1;

/** Logistic about 0.5, before renormalisation. */
const sig = (u: number): number => 1 / (1 + Math.exp(-SIG_K * (u - 0.5)));
const SIG_LO = sig(0);
const SIG_HI = sig(1);

export const SIZE_CURVES: SizeCurveDef[] = [
  {
    id: 'area',
    label: 'Area (√)',
    hint: 'area scales with the value',
    f: (u) => Math.sqrt(clamp01(u)),
    invert: (t) => clamp01(t) ** 2,
  },
  {
    // Radius, not area, proportional to the value — which overstates
    // magnitude to the eye (a 2× radius is a 4× disc) and is exactly why it
    // is not the default. It is also the curve that makes a *narrow* domain
    // readable, where √ crushes an already-small spread into nothing.
    id: 'linear',
    label: 'Linear',
    hint: 'radius scales with the value',
    f: (u) => clamp01(u),
    invert: (t) => clamp01(t),
  },
  {
    id: 'log',
    label: 'Logarithmic',
    hint: 'spreads the small end apart',
    f: (u) => clamp01(Math.log10(1 + LOG_K * clamp01(u)) / Math.log10(1 + LOG_K)),
    invert: (t) => clamp01(((1 + LOG_K) ** clamp01(t) - 1) / LOG_K),
  },
  {
    id: 'exponential',
    label: 'Exponential',
    hint: 'only the largest stand out',
    f: (u) => clamp01((Math.exp(EXP_K * clamp01(u)) - 1) / EXP_SPAN),
    invert: (t) => clamp01(Math.log(1 + clamp01(t) * EXP_SPAN) / EXP_K),
  },
  {
    id: 'sigmoid',
    label: 'S-curve',
    hint: 'spreads the middle, flattens both ends',
    f: (u) => clamp01((sig(clamp01(u)) - SIG_LO) / (SIG_HI - SIG_LO)),
    invert: (t) => {
      const y = SIG_LO + clamp01(t) * (SIG_HI - SIG_LO);
      // y is strictly inside (0,1) for any t in [0,1] because SIG_LO > 0 and
      // SIG_HI < 1, so the log is never asked for a pole — but the guard
      // costs nothing and a future k would otherwise fail silently at NaN.
      if (y <= 0 || y >= 1) return y <= 0 ? 0 : 1;
      return clamp01(0.5 + Math.log(y / (1 - y)) / SIG_K);
    },
  },
];

export const SIZE_CURVE_IDS = SIZE_CURVES.map((c) => c.id) as SizeCurveId[];

export function sizeCurveDef(id: SizeCurveId): SizeCurveDef {
  return SIZE_CURVES.find((c) => c.id === id) ?? SIZE_CURVES[0];
}

// ── Size scale ───────────────────────────────────────────────────────────
//
// The second half of the same complaint. A curve redistributes a *fixed* 7–34
// px range; when a graph is sparse there is no reason that range is the right
// one, and 27px of span is not much to discriminate with.
//
// The multiplier scales the SPAN, not the whole radius: the floor stays at
// R_MIN because a node still has to be clickable, carry its two-letter kind
// code, and not vanish. So ×2 does not double every circle — it doubles the
// distance between the smallest and the largest, which is precisely the
// "better discrimination between the groups" this is for.
export const SIZE_BOOST_MIN = 0.5;
export const SIZE_BOOST_MAX = 3;
export const SIZE_BOOST_DEFAULT = 1;

export function clampBoost(v: number): number {
  if (!Number.isFinite(v)) return SIZE_BOOST_DEFAULT;
  return Math.min(SIZE_BOOST_MAX, Math.max(SIZE_BOOST_MIN, v));
}

// ── Size groups ──────────────────────────────────────────────────────────
//
// UI-110. The curve and the scale both keep size *continuous*: every node
// gets its own radius, and two files 20 lines apart draw 0.4px apart. That is
// maximal information and it is not a reading — "is this one of the big ones"
// has no answer when there are 117 different sizes, and the legend's three
// sample dots imply three classes the canvas does not actually draw.
//
// Grouping snaps the radius to one of N levels, so the classes become real:
// a node is in a group, the legend names the group's value band, and the
// question the picture answers changes from "how big exactly" to "which
// group". The cost is deliberate — within-group differences are discarded,
// which is the whole point of a bin.
//
// ## Where the boundaries go, and why nobody places them by hand
//
// Quantisation happens on `t`, the RADIUS fraction, after the curve — not on
// the value. That single choice is what automates the boundaries:
//
//   * The N levels are evenly spaced in radius, so consecutive groups always
//     differ by `span / (N - 1)` pixels. At ×1 with 6 groups that is 5.4px,
//     and at ×2 it is 10.8px — which is what makes the scale control the
//     thing that keeps groups apart as the count rises.
//   * The value boundaries fall wherever the curve puts them. Under `log` the
//     low groups cover narrow value bands and the top group is enormous;
//     under `exponential` it is the reverse. Both are the curve doing exactly
//     what it was selected to do, one level up.
//
// Placing the edges in value space instead (equal counts, or equal value
// spans) would silently overrule the curve, and would let two groups land a
// pixel apart on screen — a group nobody can see is not a group.
export const SIZE_BINS_OFF = 0;
export const SIZE_BINS_MIN = 2;
export const SIZE_BINS_MAX = 8;

/** `SIZE_BINS_OFF` (continuous) or a group count in range. Anything else —
 *  a fractional count, a NaN out of `localStorage` — falls back to
 *  continuous, which is the mode that cannot mislead. */
export function clampBins(v: number): number {
  if (!Number.isFinite(v)) return SIZE_BINS_OFF;
  const n = Math.round(v);
  if (n < SIZE_BINS_MIN) return SIZE_BINS_OFF;
  return Math.min(SIZE_BINS_MAX, n);
}

/**
 * Snap a radius fraction to one of `bins` evenly spaced levels.
 *
 * `Math.round` and not `Math.floor`: rounding keeps 0 → 0 and 1 → 1, so the
 * smallest node still draws at R_MIN and the largest at R_MAX and the group
 * count cannot change what the two ends of the scale mean — the same endpoint
 * contract every curve holds. Flooring would put the largest node in a bin of
 * its own and leave the top level empty on every graph but a uniform one.
 */
export function quantiseFraction(t: number, bins: number): number {
  const n = clampBins(bins);
  if (n === SIZE_BINS_OFF) return clamp01(t);
  const steps = n - 1;
  return Math.round(clamp01(t) * steps) / steps;
}

/**
 * The value fractions where one group becomes the next, ascending.
 *
 * `bins - 1` of them, sitting at the midpoints between levels — which is
 * where `Math.round` actually switches — pushed back through the curve. The
 * legend prints these as each group's band; nothing else may derive them, or
 * the legend would name boundaries the canvas does not draw.
 */
export function binEdges(curve: SizeCurveDef, bins: number): number[] {
  const n = clampBins(bins);
  if (n === SIZE_BINS_OFF) return [];
  const steps = n - 1;
  return Array.from({ length: steps }, (_, i) => curve.invert((i + 0.5) / steps));
}

/**
 * Round bin edges for display at the coarsest precision that keeps them
 * distinct.
 *
 * A boundary is not a sample, and the two round differently. `niceValue`'s
 * 1/2/5 ladder is right for "a node about this big is about this many lines"
 * — a readable number near the sample. Run on boundaries it produces
 * `1,000–1,000`, seen at 8 groups on a 2,520-line domain where the raw edges
 * 1,041 and 1,556 both land on 1,000. That row names a band nothing can be
 * in, next to a size the canvas is definitely drawing.
 *
 * So edges get exact rounding, at the fewest decimals that keeps every
 * adjacent pair apart — integers on any real LOC domain, decimals only when
 * the metric range is genuinely narrow (a pagerank column, a graph whose
 * largest file is 3 lines). If even three decimals collide, the domain has
 * fewer distinguishable values than the reader asked for groups, and the
 * repeated number is then the honest answer rather than a rounding artifact.
 */
export function displayEdges(raw: number[]): number[] {
  for (const decimals of [0, 1, 2, 3]) {
    const rounded = raw.map((v) => {
      const p = 10 ** decimals;
      return Math.round(v * p) / p;
    });
    if (rounded.every((v, i) => i === 0 || v > rounded[i - 1])) return rounded;
  }
  return raw;
}
