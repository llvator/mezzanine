<!--
  The Elevator layer, drawn as its own canvas beside the code graph (ADR 0011).

  Deliberately *not* a second `GraphView`. That component encodes node size and
  fill from metrics, collapses to file and module levels, overlays a diff, draws
  folder hulls and marks arrivals — every one of which is meaningless for a
  Feature, which has no complexity, no file worth collapsing to and no commit
  history of its own. What the spec layer needs instead is a tier per kind and a
  click that filters the other pane, so that is all this draws.

  It also runs no simulation, and no zoom. Positions come from
  `layoutSpecGraph`, which wraps each tier into sub-rows — see its header for
  why a force layout does not survive a real spec in a side pane. d3 is used
  here for the data join and nothing else; the pane scrolls.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import * as d3 from 'd3';
  import type { D3Node, D3Link } from '../types/graph';
  import { NODE_COLORS, KIND_CODES } from '../types/graph';
  import { canvasChrome, type CanvasChrome } from '../utils/canvasChrome';
  import { activeTheme } from '../stores/settings';
  import { selectedNode } from '../stores/graph';
  import {
    specGraph, visibleSpecGraph, drawnSpecGraph, specScopeState, specFocus,
    specSelection, specSelectedNodes, specTrail, specHighlightIds,
    focusSpecEntity, clearSpecFocus, drillTo,
  } from '../stores/crossFilter';
  import { followAnalysisScope } from '../stores/panes';
  import type { SpecScopeState } from '../viewmodels/specGraph';
  import {
    layoutSpecGraph, tierOf, COL_PITCH, type SpecGraph, type SpecLayout,
  } from '../viewmodels/specGraph';

  let container: HTMLDivElement;
  let svgEl: SVGSVGElement;
  /** The group everything is drawn into. Held so an emphasis repaint can
   *  restyle without relaying anything out. */
  let plot: d3.Selection<SVGGElement, unknown, null, undefined> | null = null;
  let colors: CanvasChrome = canvasChrome();
  let paneWidth = 400;

  /** Radius per tier — the hierarchy is legible from size alone, so a reader
   *  can tell a Category from a Functionality without reading the caption. */
  const RADII = [10, 9, 7, 5, 6, 6];
  const DEFAULT_RADIUS = 5;

  function radiusOf(node: D3Node): number {
    return RADII[tierOf(node)] ?? DEFAULT_RADIUS;
  }

  /** Names run past their column at 10px; the label is trimmed rather than
   *  allowed to collide with its neighbour, and the full name is one hover
   *  away in the Details pane. */
  const LABEL_CHARS = Math.floor(COL_PITCH / 6);
  function labelOf(node: D3Node): string {
    return node.name.length > LABEL_CHARS ? `${node.name.slice(0, LABEL_CHARS - 1)}…` : node.name;
  }

  /** Endpoint id, whichever form the link is in. Links reach here straight off
   *  `fullGraphDataStore`, and d3 rewrites `source`/`target` from ids to node
   *  objects in place the first time any simulation binds them — including the
   *  code canvas's. Reading `.id` blindly would work until the user opened the
   *  spec pane second. */
  function endId(end: string | D3Node): string {
    return typeof end === 'object' ? end.id : end;
  }

  /**
   * What a marked entity's mark means, and what to do about it.
   *
   * Three reasons an entity can show nothing, three different fixes — so one
   * shared "shows nothing" treatment would be worse than no mark at all,
   * because it would send a reader editing a correct `cr:` when the real
   * answer is to widen a scope. The glyph carries the distinction and the
   * tooltip spells it out; the dim is shared because "clicking this does
   * nothing" *is* the same fact in all three cases.
   */
  const MARKS: Record<Exclude<SpecScopeState, 'in-scope'>, { glyph: string; why: string }> = {
    'out-of-scope': {
      glyph: '\u25CC',
      why: 'claims code outside the analysis scope — widen the scope to reach it',
    },
    unanalyzed: {
      glyph: '\u2716',
      why: 'no analysed code at its cr: paths — either drift, or a file type outside the parsed languages; elevator --drift tells you which',
    },
    unanchored: {
      glyph: '\u25CB',
      why: 'no cr: anywhere in its subtree — nobody has written down where this lives',
    },
  };

  function markOf(node: D3Node): { glyph: string; why: string } | null {
    const state = $specScopeState.get(node.id);
    if (!state || state === 'in-scope') return null;
    return MARKS[state];
  }

  /** Marked entities read as unavailable before they are clicked, which is the
   *  point — a click on one shows nothing, and the reader should be able to
   *  see that in advance. */
  function nodeOpacity(node: D3Node): number {
    return markOf(node) ? 0.45 : 1;
  }

  /** The open path, as a set, for the ring tests. */
  $: onTrail = new Set($specTrail.map((n) => n.id));

  // ------------------------------------------------------------------
  // Emphasis
  // ------------------------------------------------------------------
  //
  // Rings only, and no dimming of the unselected. The flat view needed to fade
  // three quarters of the pane to make a selection findable; a drilled pane
  // never draws the three quarters, so fading anything would only be taking
  // contrast away from the handful of entities left. The one dim that stays is
  // the scope mark below, which says something different — "this one is not
  // worth clicking".

  function linkOpacity(link: D3Link): number {
    // Containment edges carry the descent and should read; the dashed
    // cross-links are context and should not compete with it.
    return link.kind_raw === 'Contains' ? 0.5 : 0.22;
  }

  function ringColor(node: D3Node): string {
    if ($specSelection.has(node.id)) return colors.accent;
    if (onTrail.has(node.id) || $specHighlightIds.has(node.id)) return colors.accent;
    return colors.nodeStroke;
  }

  /**
   * Selected is the thickest ring, the open path is thinner but still
   * accented, and a reverse-highlighted claimant matches the path.
   *
   * Selection outranks the path because selection is what the code canvas is
   * actually filtered by, and the two can now differ: the Filters pane can
   * select an entity the pane has not drilled to, and climbing the breadcrumb
   * moves the path without touching the filter.
   */
  function ringWidth(node: D3Node): number {
    if ($specSelection.has(node.id)) return 3;
    if (onTrail.has(node.id)) return 2;
    if ($specHighlightIds.has(node.id)) return 2;
    return 1;
  }

  // ------------------------------------------------------------------
  // Render
  // ------------------------------------------------------------------

  /**
   * Rebuild from scratch on every graph or width change.
   *
   * A spec is small — a large one is a few hundred entities — so the
   * incremental data-join `GraphView` needs to stay responsive buys nothing
   * here, and a full rebuild has no stale-state failure mode to debug.
   */
  function render(graph: SpecGraph, width: number): void {
    if (!svgEl) return;
    colors = canvasChrome();
    const svg = d3.select(svgEl);
    svg.selectAll('*').remove();
    plot = null;
    if (graph.empty) return;

    const layout: SpecLayout = layoutSpecGraph(graph, width);
    const at = (id: string) => layout.positions.get(id) ?? { x: 0, y: 0 };

    // Sized to the layout in raw pixels, and scrolled by its container. No
    // zoom behaviour: the layout already wraps to the pane's width, so there is
    // nothing to pan to horizontally, and d3.zoom's wheel handler would eat the
    // vertical scroll that is the one navigation this pane actually needs.
    svg.attr('width', width).attr('height', layout.height);

    const root = svg.append('g').attr('class', 'plot');
    plot = root;

    // Clicking the background clears the cross-filter — the same gesture that
    // clears a selection on the code canvas.
    svg.on('click', (event: MouseEvent) => {
      if (event.target === svgEl) clearSpecFocus();
    });

    const captions = root.append('g').attr('class', 'bands');
    for (const band of layout.bands) {
      captions.append('text')
        .attr('x', 4)
        .attr('y', band.y)
        .attr('fill', colors.hullLabelFill)
        .attr('font-size', '9px')
        .attr('letter-spacing', '0.08em')
        .text(`${band.kind.toUpperCase()} · ${band.count}`);
      captions.append('line')
        .attr('x1', 4).attr('x2', width - 8)
        .attr('y1', band.y + 5).attr('y2', band.y + 5)
        .attr('stroke', colors.hullStroke)
        .attr('stroke-opacity', 0.15);
    }

    const links = graph.links.filter(
      (l) => layout.positions.has(endId(l.source)) && layout.positions.has(endId(l.target)),
    );

    root.append('g').attr('class', 'links')
      .selectAll<SVGLineElement, D3Link>('line')
      .data(links)
      .join('line')
      .attr('x1', (d) => at(endId(d.source)).x)
      .attr('y1', (d) => at(endId(d.source)).y)
      .attr('x2', (d) => at(endId(d.target)).x)
      .attr('y2', (d) => at(endId(d.target)).y)
      .attr('stroke', colors.arrowFill)
      .attr('stroke-width', (d) => (d.kind_raw === 'Contains' ? 1.2 : 1))
      // Dashed for everything that is not containment: `References` and the
      // Concept cross-links are what the hierarchy cannot express, and a reader
      // has to tell them from it at a glance or the tiers stop meaning anything.
      .attr('stroke-dasharray', (d) => (d.kind_raw === 'Contains' ? null : '3,3'));

    const nodeSel = root.append('g').attr('class', 'nodes')
      .selectAll<SVGGElement, D3Node>('g')
      .data(graph.nodes)
      .join('g')
      .attr('class', 'spec-node')
      .attr('cursor', 'pointer')
      .attr('transform', (d) => `translate(${at(d.id).x},${at(d.id).y})`)
      .on('click', (event: MouseEvent, d: D3Node) => {
        event.stopPropagation();
        focusSpecEntity(d);
      });

    nodeSel.append('title').text((d) => {
      const mark = markOf(d);
      return mark ? `${d.kind} ${d.qualified_name} — ${mark.why}` : `${d.kind} ${d.qualified_name}`;
    });

    nodeSel.append('circle')
      .attr('r', radiusOf)
      .attr('fill', (d) => NODE_COLORS[d.kind_raw] ?? colors.arrowFill);

    nodeSel.append('text')
      .attr('text-anchor', 'middle')
      .attr('dy', 2.5)
      .attr('font-size', '7px')
      .attr('font-weight', '600')
      // Near-black on the per-kind fills, which are saturated mid-tones in
      // every theme — the same reasoning as the order badge in canvasChrome.
      .attr('fill', '#141414')
      .attr('pointer-events', 'none')
      .text((d) => KIND_CODES[d.kind_raw] ?? '');

    // Off the circle rather than on it: the two-letter kind code already owns
    // the inside, and a mark that displaced it would trade one piece of
    // information for another.
    nodeSel.filter((d) => markOf(d) !== null)
      .append('text')
      .attr('class', 'mark')
      .attr('x', (d) => radiusOf(d) + 1)
      .attr('y', (d) => -radiusOf(d) + 3)
      .attr('font-size', '8px')
      .attr('fill', colors.hullLabelFill)
      .attr('pointer-events', 'none')
      .text((d) => markOf(d)?.glyph ?? '');

    nodeSel.append('text')
      .attr('text-anchor', 'middle')
      .attr('dy', (d) => radiusOf(d) + 10)
      .attr('font-size', '9px')
      .attr('fill', colors.nameLabelFill)
      .attr('pointer-events', 'none')
      .text(labelOf);

    applyEmphasis();
  }

  /** Repaint the emphasis without relaying anything out. */
  function applyEmphasis(): void {
    if (!plot) return;
    const nodeSel = plot.selectAll<SVGGElement, D3Node>('g.spec-node');
    nodeSel.attr('opacity', (d) => nodeOpacity(d));
    nodeSel.select<SVGCircleElement>('circle')
      .attr('stroke', (d) => ringColor(d))
      .attr('stroke-width', (d) => ringWidth(d));
    plot.selectAll<SVGLineElement, D3Link>('line')
      .attr('stroke-opacity', (d) => (d ? linkOpacity(d) : 0.15));
  }

  function measure(): void {
    const next = container?.clientWidth ?? 0;
    if (next > 0) paneWidth = next;
  }

  let observer: ResizeObserver | null = null;

  onMount(() => {
    measure();
    observer = new ResizeObserver(() => measure());
    if (container) observer.observe(container);
  });

  onDestroy(() => observer?.disconnect());

  // Two statements rather than one because they cost very different amounts: a
  // rebuild relays the whole spec out, a repaint only touches attributes. Both
  // go through a named function so the dependency list is the argument list —
  // Svelte orders reactive statements by the variables they name, and an
  // implicit dependency here would be a repaint that runs before the value it
  // repaints from has updated.
  //
  // `$activeTheme` belongs to the rebuild because the chrome colours are baked
  // onto SVG attributes at join time; a CSS variable swap alone leaves the
  // canvas stale (UI-009).
  $: rebuild($drawnSpecGraph, paneWidth, $activeTheme, $specScopeState, svgEl);
  $: repaint(onTrail, $specSelection, $specHighlightIds, plot);

  function rebuild(
    graph: SpecGraph,
    width: number,
    _theme: string,
    _states: Map<string, SpecScopeState>,
    el: SVGSVGElement | undefined,
  ): void {
    if (!el) return;
    render(graph, width);
  }

  /** How many drawn entities carry a mark, for the footer legend. Zero is the
   *  healthy case and the legend stays out of the way there. */
  $: marked = $drawnSpecGraph.nodes.filter((n) => markOf(n) !== null).length;

  /** How many children the focus just revealed — the difference between "this
   *  is a leaf" and "there is another level down here", which a Functionality
   *  and an unexpanded Feature otherwise look identical about. */
  $: childCount = $specSelectedNodes.length === 1
    ? ($drawnSpecGraph.children.get($specSelectedNodes[0].id) ?? []).length
    : 0;

  function repaint(
    _trail: Set<string>,
    _selection: Set<string>,
    _highlight: Set<string>,
    target: typeof plot,
  ): void {
    if (!target) return;
    applyEmphasis();
  }
</script>

<div class="spec-pane" bind:this={container} data-pane="spec">
  <header>
    <span class="title">Spec</span>
    {#if !$specGraph.empty}
      <!-- Drawn of total. The drawn number moves as you drill, so on its own
           it would read as a shrinking spec rather than a focused one. -->
      <span class="count">
        {$drawnSpecGraph.empty ? 0 : $drawnSpecGraph.nodes.length}<span class="of"> of {$specGraph.nodes.length}</span>
      </span>
      <button
        type="button"
        class="toggle"
        class:active={$followAnalysisScope}
        aria-pressed={$followAnalysisScope}
        title="Draw only entities whose cr: reaches code the analysis scope has loaded"
        on:click={() => followAnalysisScope.update((v) => !v)}
      >Follow scope</button>
    {/if}
  </header>

  <!-- The trail. It is the pane's only record of where you are: the canvas
       shows one branch, and without this there is nothing saying which. Each
       crumb climbs back to that level; the root crumb closes everything. -->
  {#if $specTrail.length > 0}
    <nav class="trail" aria-label="Spec drill path">
      <button type="button" class="crumb root" on:click={clearSpecFocus} title="Back to the top level">
        All
      </button>
      {#each $specTrail as step (step.id)}
        <span class="sep">›</span>
        <button
          type="button"
          class="crumb"
          class:current={step.id === $specFocus?.id}
          on:click={() => drillTo(step.id)}
        >{step.name}</button>
      {/each}
    </nav>
  {/if}

  {#if $specGraph.empty}
    <p class="empty">
      No Elevator spec in this project. Add <code>.elv</code> files describing
      its Categories and Features and they draw here.
    </p>
  {:else if $visibleSpecGraph.empty}
    <!-- Following, and nothing survived. A different fact from "no spec", and
         one the reader can act on — so it must not borrow that message. -->
    <p class="empty">
      No spec entity reaches the current analysis scope. Widen it, or
      <button type="button" class="link" on:click={() => followAnalysisScope.set(false)}>
        stop following it
      </button>
      to see the whole spec again.
    </p>
  {:else}
    <div class="canvas">
      <svg bind:this={svgEl} role="presentation"></svg>
    </div>
    <footer data-probe="spec-status">
      {#if $specSelectedNodes.length === 1}
        Code graph filtered to <strong>{$specSelectedNodes[0].name}</strong>{#if childCount > 0} · {childCount} below{/if}
      {:else if $specSelectedNodes.length > 1}
        Code graph filtered to <strong>{$specSelectedNodes.length}</strong> spec entities
      {:else if $specHighlightIds.size > 0}
        Ringed entities claim the selected code
      {:else if marked > 0}
        <span class="legend">◌</span> out of scope ·
        <span class="legend">○</span> no <code>cr:</code> ·
        <span class="legend">✖</span> drift
      {:else}
        Click an entity to open it and filter the code graph
      {/if}
    </footer>
  {/if}
</div>

<style>
  .spec-pane {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .title {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--text);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .count { font-size: 0.7rem; color: var(--text-muted); }
  .of { color: var(--text-dim); }

  .toggle {
    margin-left: auto;
    background: none;
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 0.68rem;
    padding: 2px 6px;
  }

  .toggle:hover { background: var(--bg-hover); color: var(--text); }
  .toggle.active { border-color: var(--accent); color: var(--accent); }

  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    text-decoration: underline;
  }

  .legend { color: var(--text-secondary); }

  .trail {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 2px;
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .crumb {
    background: none;
    border: none;
    padding: 1px 3px;
    border-radius: 2px;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 0.7rem;
    font-family: inherit;
  }

  .crumb:hover { background: var(--bg-hover); color: var(--text); }
  .crumb.root { color: var(--text-muted); }
  .crumb.current { color: var(--accent); font-weight: 600; cursor: default; }
  .sep { color: var(--text-dim); font-size: 0.7rem; }

  /* The spec is taller than the pane by design — tiers wrap rather than
     shrink — so the canvas scrolls vertically and zooms for everything else. */
  .canvas {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  svg { display: block; }

  .empty {
    padding: 16px 12px;
    font-size: 0.75rem;
    line-height: 1.5;
    color: var(--text-muted);
  }

  footer {
    padding: 6px 10px;
    border-top: 1px solid var(--border);
    font-size: 0.68rem;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  footer strong { color: var(--text-secondary); }
</style>
