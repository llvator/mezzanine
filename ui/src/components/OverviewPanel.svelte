<script lang="ts">
  /**
   * The overview panel — the whole drawn graph in a corner of the canvas,
   * with a box around the part the viewport is showing.
   *
   * Zooming in trades context for detail, and past about 2× the canvas stops
   * answering "where is this in the codebase" at all: a dozen circles fill
   * the screen and every one of them looks like the middle. This panel is
   * where that answer goes instead, so the canvas can be zoomed as far as the
   * reader likes without the view becoming unplaceable.
   *
   * It draws circles and nothing else — no labels, no edges, no outlines. At
   * ~170px across, a full-repo graph's edges are a grey wash and its labels
   * overlap into a smear, and both would bury the one mark that matters here,
   * which is the box. What survives the shrink is position, relative size and
   * fill, which between them are enough to recognise the canvas beside it.
   *
   * Position, size and fill come from `GraphView`'s live encoding rather than
   * from a palette of this panel's own — see `stores/overview`. The geometry
   * is all in `viewmodels/overviewFrame`, which is where it can be tested.
   */
  import type GraphView from './GraphView.svelte';
  import { overviewDots, canvasViewport, overviewOpen } from '../stores/overview';
  import {
    viewportWorldRect, overviewWorld, fitWorld, projectRect, overviewToWorld,
    dotRadius, type OverviewFit,
  } from '../viewmodels/overviewFrame';

  /** Bound instance of the graph, for `panTo`. Undefined until `App`'s
   *  `bind:this` lands, which is after this component's first render — the
   *  same contract `CanvasToolbar` works under. */
  export let graphView: GraphView | undefined = undefined;

  /** How far above the canvas floor to sit, in px. The bottom strip below
   *  grows and wraps with the controls it carries, so the clearance is
   *  measured by `App` rather than guessed at here. */
  export let bottomInset = 60;

  /** Fixed, and small. A resizable overview is a second thing to manage in
   *  the corner of the screen, and the panel has no detail that more pixels
   *  would reveal — it is deliberately a picture with no labels in it. */
  const BOX_W = 176;
  const BOX_H = 124;
  const PAD = 5;

  let svgEl: SVGSVGElement;
  let dragging = false;

  $: view = $canvasViewport
    ? viewportWorldRect($canvasViewport, $canvasViewport.w, $canvasViewport.h)
    : null;
  $: world = overviewWorld($overviewDots, view);
  $: fit = world ? fitWorld(world, BOX_W, BOX_H, PAD) : null;
  $: box = fit && view ? projectRect(view, fit) : null;

  /** Nothing drawn means nothing to overview. Folding away beats an empty
   *  frame: the canvas already says "no nodes" in its own overlay, and a
   *  second blank rectangle repeating it just occupies the corner. */
  $: hasFrame = $overviewDots.length > 0 && fit !== null;

  /** Centre the canvas on the world point under the pointer.
   *
   *  Centre-on-pointer rather than drag-the-box-by-its-grab-offset because
   *  the two only differ when the press lands off-centre, and picking the
   *  offset up means a click on empty panel *elsewhere* does nothing until
   *  the pointer moves. One rule for click and drag alike, and the box
   *  arrives under the cursor either way. */
  function panFrom(event: PointerEvent, activeFit: OverviewFit): void {
    if (!graphView) return;
    const r = svgEl.getBoundingClientRect();
    const w = overviewToWorld(event.clientX - r.left, event.clientY - r.top, activeFit);
    graphView.panTo(w.x, w.y);
  }

  function onPointerDown(event: PointerEvent): void {
    if (!fit || event.button !== 0) return;
    dragging = true;
    // Capture, so a drag that leaves the little panel — which at 176px wide
    // is most of them — keeps steering instead of stopping at the edge.
    svgEl.setPointerCapture(event.pointerId);
    panFrom(event, fit);
    event.stopPropagation();
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging || !fit) return;
    panFrom(event, fit);
  }

  function onPointerUp(event: PointerEvent): void {
    if (!dragging) return;
    dragging = false;
    svgEl.releasePointerCapture(event.pointerId);
  }
</script>

<div class="overview" class:collapsed={!$overviewOpen} data-probe="overview-panel"
  style="bottom: {bottomInset}px">
  <button
    type="button"
    class="overview-handle"
    aria-expanded={$overviewOpen}
    title={$overviewOpen ? 'Hide overview' : 'Show overview'}
    on:click={() => overviewOpen.set(!$overviewOpen)}
  >
    <span class="overview-chev">{$overviewOpen ? '▾' : '▴'}</span>
    <span class="overview-title">Overview</span>
  </button>

  {#if $overviewOpen}
    <!-- The pointer handlers are on the SVG and the whole point of it is to
         be dragged, so the a11y linter's "add a keyboard handler" is the
         wrong fix here: panning already has keyboard and button routes on the
         canvas itself, and this is a pointer shortcut to them, not the only
         way in. -->
    <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
    <svg
      bind:this={svgEl}
      class="overview-map"
      class:dragging
      class:idle={!hasFrame}
      width={BOX_W}
      height={BOX_H}
      viewBox="0 0 {BOX_W} {BOX_H}"
      role="presentation"
      on:pointerdown={onPointerDown}
      on:pointermove={onPointerMove}
      on:pointerup={onPointerUp}
      on:pointercancel={onPointerUp}
    >
      {#if hasFrame && fit}
        {#each $overviewDots as dot, i (i)}
          <circle
            cx={dot.x * fit.scale + fit.offsetX}
            cy={dot.y * fit.scale + fit.offsetY}
            r={dotRadius(dot.r, fit)}
            fill={dot.fill}
            fill-opacity={dot.opacity}
          />
        {/each}
        {#if box}
          <rect
            class="viewport-box"
            x={box.x} y={box.y} width={box.width} height={box.height}
          />
        {/if}
      {:else}
        <text class="overview-empty" x={BOX_W / 2} y={BOX_H / 2}>nothing drawn</text>
      {/if}
    </svg>
  {/if}
</div>

<style>
  /* Bottom-right, above the canvas rather than beside it. The corner is the
     one part of a force layout that is reliably empty — the simulation pulls
     toward the centre — and taking a column instead would shrink the very
     view this exists to help the reader hold.

     The bottom inset used to clear the changes strip, which shared this corner
     and carried the live-status and endpoint chips: at 12px this panel covered
     them completely, and a status indicator that is invisible is worse than
     absent, because the page still looks like it is reporting one. Since
     UI-150 that strip is a row of the app shell and no longer overlaps the
     canvas at all, so the inset is a constant again — what is left in this
     corner is the build stamp the VS Code webview pins there. */
  .overview {
    position: absolute;
    right: 12px;
    bottom: 60px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35);
  }

  .overview-handle {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 3px 7px;
    background: var(--bg-surface-alt);
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    cursor: pointer;
  }
  .overview-handle:hover { color: var(--text); background: var(--bg-hover); }
  .overview.collapsed .overview-handle { border-bottom: none; }

  .overview-chev { font-size: 9px; }
  .overview-title { line-height: 1; }

  .overview-map {
    display: block;
    background: var(--bg-deep);
    cursor: grab;
    /* The panel is a control, and every pointer gesture on it is a pan — so
       the browser's own pan/zoom gestures on a touch device would fight it
       for the same drag. */
    touch-action: none;
  }
  .overview-map.dragging { cursor: grabbing; }
  /* With no graph there is nothing to steer toward, and a grab cursor over an
     empty box promises otherwise. */
  .overview-map.idle { cursor: default; }

  /* The one mark the panel exists to draw, so it gets the accent — the same
     colour a selected node wears on the canvas, and for the same reason:
     it is where the reader's attention is. Fill as well as stroke, because a
     zoomed-out viewport can cover most of the panel and an outline that large
     stops reading as a region; at high zoom it shrinks toward a stroked dot,
     which is exactly when the fill stops mattering. */
  .viewport-box {
    fill: var(--accent);
    fill-opacity: 0.13;
    stroke: var(--accent);
    stroke-width: 1.5;
    /* The box tracks the pointer during a drag, so it must never be the
       thing the pointer hits — capture is on the SVG. */
    pointer-events: none;
  }

  .overview-empty {
    fill: var(--text-disabled);
    font-size: 10px;
    text-anchor: middle;
    dominant-baseline: middle;
  }
</style>
