<script lang="ts">
  /**
   * A quality scatter plot.
   *
   * Extracted because three charts in QualityReport shared one shape and one
   * set of gaps: axis lines but no ticks, a corner max-value annotation
   * standing in for an axis title, and no legend for a four-state colour
   * encoding (UI-019). Fixing that in place would have meant doing it three
   * times in an already-oversized component.
   *
   * What was already right and must stay: a `<title>` per point naming the
   * entity and its values, and click-to-select. Those worked before this
   * ticket and the probe guards them.
   */

  import { axisMax, sampleScatter } from '../viewmodels/scatterPoints';

  /** One plotted point. `datum` is handed back to `onSelect` untouched. */
  export let points: { x: number; y: number; datum: unknown }[] = [];
  export let xTitle: string;
  export let yTitle: string;
  /** Per-point colour — the shared severity encoding, supplied by the caller. */
  export let colorOf: (datum: unknown) => string;
  /** Per-point tooltip text. */
  export let labelOf: (datum: unknown, x: number, y: number) => string;
  /** True when the point should be marked as sitting in a dependency cycle. */
  export let inCycleOf: (datum: unknown) => boolean = () => false;
  export let onSelect: (datum: unknown) => void = () => {};
  export let emptyMessage = 'No data in the current scope.';

  const W = 260;
  const H = 180;
  const PAD_L = 34;
  const PAD_B = 30;
  const PAD_T = 8;
  const PAD_R = 10;

  /**
   * Tick values at a round step.
   *
   * `1/2/5 × 10^n` keeps labels readable at any magnitude — fan-in runs to
   * ~300 while nesting runs to ~5, and a fixed divisor would give either
   * fractional ticks on the small charts or unreadably dense ones on the
   * large. Four intervals is the most a ~230px axis fits without collisions.
   */
  function ticksFor(max: number, target = 4): number[] {
    if (!isFinite(max) || max <= 0) return [0];
    const raw = max / target;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const norm = raw / mag;
    const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag;
    const out: number[] = [];
    for (let v = 0; v <= max + step * 0.001; v += step) out.push(Math.round(v * 1000) / 1000);
    return out;
  }

  // A fold, not `Math.max(1, ...points.map(...))`: the spread passes one
  // argument per point and overflowed the stack above ~125k of them, taking
  // the whole Quality tab down on a large repo. See `scatterPoints.ts`.
  $: maxX = axisMax(points, (p) => p.x);
  $: maxY = axisMax(points, (p) => p.y);
  // Scaled by the whole population, drawn from a sample of it.
  $: sample = sampleScatter(points);
  $: sx = (x: number) => PAD_L + (x / maxX) * (W - PAD_L - PAD_R);
  $: sy = (y: number) => H - PAD_B - (y / maxY) * (H - PAD_B - PAD_T);
  $: xTicks = ticksFor(maxX);
  $: yTicks = ticksFor(maxY);
</script>

<div class="scatter-wrap">
  <div class="scatter-title">{yTitle} × {xTitle}</div>

  {#if points.length === 0}
    <!-- Previously an empty <svg> rendered: a blank box that reads as a
         rendering failure rather than as "nothing to plot". -->
    <div class="scatter-empty" data-probe="chart-empty">{emptyMessage}</div>
  {:else}
    <svg viewBox="0 0 {W} {H}" class="scatter" data-probe="quality-chart" role="img"
      aria-label="{yTitle} against {xTitle}, {points.length} entities">
      <!-- Gridlines first so points and axes draw over them. -->
      {#each xTicks as t}
        <line class="grid" x1={sx(t)} y1={PAD_T} x2={sx(t)} y2={H - PAD_B} />
      {/each}
      {#each yTicks as t}
        <line class="grid" x1={PAD_L} y1={sy(t)} x2={W - PAD_R} y2={sy(t)} />
      {/each}

      <line class="axis" x1={PAD_L} y1={H - PAD_B} x2={W - PAD_R} y2={H - PAD_B} />
      <line class="axis" x1={PAD_L} y1={PAD_T} x2={PAD_L} y2={H - PAD_B} />

      {#each xTicks as t}
        <g class="tick" data-probe="tick">
          <line x1={sx(t)} y1={H - PAD_B} x2={sx(t)} y2={H - PAD_B + 3} />
          <text x={sx(t)} y={H - PAD_B + 12} text-anchor="middle">{t}</text>
        </g>
      {/each}
      {#each yTicks as t}
        <g class="tick" data-probe="tick">
          <line x1={PAD_L - 3} y1={sy(t)} x2={PAD_L} y2={sy(t)} />
          <text x={PAD_L - 5} y={sy(t) + 3} text-anchor="end">{t}</text>
        </g>
      {/each}

      <text class="axis-title" x={(PAD_L + W - PAD_R) / 2} y={H - 2} text-anchor="middle">{xTitle}</text>
      <text class="axis-title" x={-(PAD_T + H - PAD_B) / 2} y={9}
        text-anchor="middle" transform="rotate(-90)">{yTitle}</text>

      {#each sample.shown as p}
        <!-- In-cycle points get a ring rather than a fourth fill: the cycle
             red and the bad red were near-identical, so two distinct meanings
             rendered as effectively one colour. -->
        <circle
          class="dot"
          class:in-cycle={inCycleOf(p.datum)}
          cx={sx(p.x)}
          cy={sy(p.y)}
          r={inCycleOf(p.datum) ? 3.4 : 2.6}
          fill={colorOf(p.datum)}
          on:click={() => onSelect(p.datum)}
          role="button"
          tabindex="-1"
          aria-label={labelOf(p.datum, p.x, p.y)}
          on:keydown={(e) => { if (e.key === 'Enter') onSelect(p.datum); }}
        ><title>{labelOf(p.datum, p.x, p.y)}</title></circle>
      {/each}
    </svg>

    <div class="scatter-legend" data-probe="chart-legend">
      <span class="key"><i class="swatch t-ok"></i>ok</span>
      <span class="key"><i class="swatch t-warn"></i>warn</span>
      <span class="key"><i class="swatch t-bad"></i>bad</span>
      <span class="key"><i class="swatch t-bad ring"></i>in cycle</span>
      <!-- Says so when it is a sample. The axes still measure the whole
           population, so silence here would read as "this is all of it". -->
      {#if sample.sampled}
        <span class="key sampled" data-probe="chart-sampled"
          title="Too many points to draw. Every {Math.ceil(sample.total / sample.shown.length)}th is plotted, plus both extremes; the axes and the table below still measure all {sample.total.toLocaleString()}."
        >{sample.shown.length.toLocaleString()} of {sample.total.toLocaleString()} drawn</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .scatter-wrap { margin-bottom: 10px; }

  .scatter-title {
    font-size: 0.72rem;
    font-weight: 600;
    color: var(--text-muted);
    margin-bottom: 2px;
  }

  .scatter { width: 100%; height: auto; overflow: visible; }

  .scatter-empty {
    padding: 14px 10px;
    border: 1px dashed var(--border-subtle);
    border-radius: 4px;
    font-size: 0.72rem;
    color: var(--text-dim);
    text-align: center;
  }

  .grid { stroke: var(--border-subtle); stroke-width: 0.5; opacity: 0.45; }
  .axis { stroke: var(--text-disabled); stroke-width: 1; }

  .tick line { stroke: var(--text-disabled); stroke-width: 1; }
  .tick text { fill: var(--text-dim); font-size: 7px; }
  .axis-title { fill: var(--text-muted); font-size: 8px; font-weight: 600; }

  .dot {
    opacity: 0.75;
    cursor: pointer;
    transition: opacity 0.12s, stroke-width 0.12s;
  }
  /* Advertises the click before the OS tooltip delay elapses. */
  .dot:hover { opacity: 1; stroke: var(--text); stroke-width: 1.5; }
  .dot.in-cycle { stroke: var(--text); stroke-width: 1.2; opacity: 0.95; }

  .scatter-legend {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 3px;
    font-size: 0.66rem;
    color: var(--text-dim);
  }
  .key { display: inline-flex; align-items: center; gap: 3px; }
  .key.sampled { color: var(--text-muted); font-style: italic; cursor: help; }

  .swatch {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }
  .swatch.t-ok { background: var(--tier-ok-fg); }
  .swatch.t-warn { background: var(--tier-warn-fg); }
  .swatch.t-bad { background: var(--tier-bad-fg); }
  .swatch.ring { box-shadow: 0 0 0 1.5px var(--text); }
</style>
