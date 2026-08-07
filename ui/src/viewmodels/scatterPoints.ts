/**
 * scatterPoints — what a scatter measures, and what it draws.
 *
 * Two rules, both learned from one repo. Analysing tinygrad puts 213,060 rows
 * in the Quality population, and `Scatter.svelte` did this with them:
 *
 *     $: maxX = Math.max(1, ...points.map((p) => p.x));
 *
 * A spread passes one *argument* per element, and V8's argument stack gives
 * out between 100k and 125k of them. So the axis scale threw `RangeError:
 * Maximum call stack size exceeded` inside a `$:` pre-effect, which took
 * `Scatter` down, and with it `QualityReport` and the whole Quality tab — no
 * error surfaced, the tab simply never mounted. A metric that only fails on
 * the largest codebase is a metric that fails where it is most needed.
 *
 * The fold below has no such ceiling. But a chart that no longer crashes at
 * 213,060 points would still emit 213,060 `<circle>` elements, so the same
 * number needs a *drawing* answer as well as an *arithmetic* one — hence
 * `sampleScatter`, in the spirit of `drawCeiling` for the canvas.
 *
 * Kept pure and store-free, importing nothing, so the arithmetic is testable
 * without a browser (`scripts/scatter-points.test.ts`) — the same isolation
 * `drawCeiling.ts` keeps, and for the same reason: a click-through can show
 * you a chart, it cannot show you that the number under the axis is right.
 */

/** The shape every scatter point shares. `datum` is the caller's, untouched. */
export interface Point {
  x: number;
  y: number;
}

/**
 * Largest value of `of` across `points`, never below `floor`.
 *
 * A fold rather than a spread — that is the whole point of the function.
 * `floor` defaults to 1 because the axis it scales divides by this number,
 * and an all-zero population would otherwise map every point to `NaN`.
 *
 * Non-finite values are skipped rather than propagated: one `NaN` metric on
 * one entity would otherwise make `NaN` the maximum and blank the chart.
 */
export function axisMax<T>(points: readonly T[], of: (p: T) => number, floor = 1): number {
  let max = floor;
  for (const p of points) {
    const v = of(p);
    if (Number.isFinite(v) && v > max) max = v;
  }
  return max;
}

/** Most points a scatter will put in the DOM.
 *
 *  4,000 is chosen against what the panel already ships rather than as a
 *  round number: the canvas draws up to `DRAW_CEILING` (2,000) far heavier
 *  nodes — each a `<g>` with a circle, a label and a simulation body — while
 *  a scatter point is one `<circle>` with a `<title>`. Twice the canvas
 *  ceiling is therefore comfortably inside the load this UI is known to
 *  carry, and it is dense enough that a 260×180 plot is saturated: at 4,000
 *  points over ~46,000 square units the dots already overlap, so the next
 *  thousand change the picture by nothing a reader can see. */
export const SCATTER_CEILING = 4000;

/** A population, and the part of it that gets drawn. */
export interface ScatterSample<T> {
  /** The points to render — every point, or an even sample of them. */
  shown: T[];
  /** How many there were before sampling. */
  total: number;
  /** True when `shown` is a sample, so the chart can say so. A chart that
   *  silently drops 98% of its population is lying about the population. */
  sampled: boolean;
}

/**
 * Cut a population down to what a scatter can draw, without moving the axes.
 *
 * Even stride, so the sample keeps the *shape* of the cloud — the dense
 * region stays dense and the sparse tail stays sparse, which is what a
 * scatter is read for. Deterministic, so the same population draws the same
 * picture twice; a random sample would shimmer on every reactive update.
 *
 * The two extremes are then forced back in. The axes are scaled by
 * `axisMax` over the *whole* population (that is the honest scale), so
 * dropping the point that defines the top of an axis leaves a chart whose
 * gridlines run to 300 with nothing drawn past 40 — which reads as a broken
 * renderer rather than as a sample. The outliers are also the rows a reader
 * opens this chart to find.
 */
export function sampleScatter<T extends Point>(
  points: readonly T[],
  ceiling: number = SCATTER_CEILING,
): ScatterSample<T> {
  const total = points.length;
  if (total <= ceiling) return { shown: points as T[], total, sampled: false };

  const stride = Math.ceil(total / ceiling);
  const keep = new Set<number>();
  for (let i = 0; i < total; i += stride) keep.add(i);
  keep.add(extremeIndex(points, (p) => p.x));
  keep.add(extremeIndex(points, (p) => p.y));

  const shown: T[] = [];
  for (const i of [...keep].sort((a, b) => a - b)) shown.push(points[i]);
  return { shown, total, sampled: true };
}

/** Index of the largest `of` value, or 0 for an empty list. First wins on a
 *  tie — the sample only needs *a* point at the axis end, not every one. */
function extremeIndex<T>(points: readonly T[], of: (p: T) => number): number {
  let best = 0;
  let bestValue = -Infinity;
  for (let i = 0; i < points.length; i++) {
    const v = of(points[i]);
    if (Number.isFinite(v) && v > bestValue) {
      bestValue = v;
      best = i;
    }
  }
  return best;
}
