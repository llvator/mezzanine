<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import * as d3 from 'd3';
  import type { D3Node, D3Link } from '../types/graph';
  import { LINK_COLORS, KIND_CODES } from '../types/graph';
  import {
    graphData, selectedNode, hoveredNode, hoverLocked, hoverDepth, viewMode,
    showLabels, showKindLabels, showLinkLabels, viewportWidth, graphLevel, hoverMode,
    toggleExpanded,
  } from '../stores/graph';
  import { focusedPane } from '../stores/keymap';
  import { displayPlan, displaySearchMatchIds, linkKeyFor, type DisplayPlan } from '../viewmodels/displayPlan';
  import { drawnIdsOf } from '../viewmodels/drawCeiling';
  import { diffActive, diffStatusMap, diffSourceChangedMap, diffDimOpacity, DIFF_COLORS, normalizeEntityId } from '../stores/diff';
  import { autoFitView, activeTheme, folderCohesion, showFolderHulls, hullDepth } from '../stores/settings';
  import { forceFolderCohesion, cohesionStrengthFor, folderKeyOf, ancestorChainOf, type FolderCohesionForce } from '../utils/forceCohesion';
  import { makeSeeder } from '../utils/layoutSeed';
  import {
    newcomers, worthMarking, noteArrivals, pruneArrivals, msUntilNextExpiry,
    ARRIVAL_HIGHLIGHT_MS, type ArrivalLog,
  } from '../utils/arrivals';
  import { liveReloading } from '../stores/liveReload';
  import { computeFolderHulls, type FolderHull } from '../viewmodels/folderHulls';
  import { groupMemberIds } from '../viewmodels/hoverHighlight';
  import { canvasChrome, type CanvasChrome } from '../utils/canvasChrome';
  import { drillIn, refreshing } from '../stores/scope';
  import { isMarked, markedPaths, toggleMark } from '../stores/marks';
  import { isMoreChildThan } from '../utils/kindPriority';
  import { nodeEncoding } from '../stores/encoding';
  import type { NodeEncoding } from '../viewmodels/nodeEncoding';
  import { ARROW_LEN, arrowHeadPoint, linkStrokeWidth } from '../viewmodels/linkGeometry';
  import { overviewDots, canvasViewport } from '../stores/overview';
  import { centreTransform, type OverviewDot } from '../viewmodels/overviewFrame';

  /** Drill into a collapsed (file or module) node: narrow the scope to its
   *  path and re-enable auto-level so the view expands to the finest level
   *  the new (smaller) subset allows. */
  function drillInto(d: D3Node): void {
    console.log(`[drill] drillInto() on canvas dblclick — kind=${d.kind_raw} id=${d.id} original_id=${d.original_id}`);
    if (d.kind_raw !== 'File' && d.kind_raw !== 'Module') {
      console.log(`[drill] drillInto() skipped — not a File/Module node`);
      return;
    }
    void drillIn(d.original_id);
  }

  /**
   * Double-click on a collapsed node: drill in, or — with shift — open it in
   * place (UI-057).
   *
   * The two are genuinely different operations and both are worth having.
   * Drilling *replaces* the view with one scope and re-runs the whole
   * pipeline; expanding *adds* one scope's contents to the view already on
   * screen and touches nothing else. Plain double-click keeps its existing
   * meaning because it is reachable from the details panel and the extension
   * and people already have it in their hands; shift takes the new one.
   */
  function onNodeDoubleClick(event: MouseEvent, d: D3Node): void {
    event.stopPropagation();
    if (d.kind_raw !== 'File' && d.kind_raw !== 'Module') return;
    if (event.shiftKey) {
      toggleExpanded(d.original_id);
      return;
    }
    drillInto(d);
  }

  /**
   * Single click: make this the subject, or — with ⌘/Ctrl — mark it.
   *
   * The modifier is the one every list on every platform already uses for
   * "add this to what I have picked", and it is free here: `mod+k` is the only
   * modified key the graph pane binds, and the canvas has no native
   * ⌘-click of its own. Marking deliberately does **not** move the subject:
   * the Details and Description panes are answering about the node you last
   * clicked, and having a third mark silently re-point them would make
   * building a set destructive to the reading you built it from.
   */
  function onNodeClick(event: MouseEvent, d: D3Node): void {
    if (!event.metaKey && !event.ctrlKey) {
      selectedNode.set(d);
      return;
    }
    event.stopPropagation();
    toggleMark(d);
  }

  let container: HTMLDivElement;
  let svgEl: SVGSVGElement;
  let simulation: d3.Simulation<D3Node, D3Link>;
  /** Chrome colours for the active theme. Re-read at each render and on
   *  theme change — the values are baked onto SVG attributes at join time,
   *  so a CSS variable swap alone would leave the canvas stale (UI-009). */
  let canvasColors: CanvasChrome = canvasChrome();

  let svg: d3.Selection<SVGSVGElement, unknown, null, undefined>;
  let g: d3.Selection<SVGGElement, unknown, null, undefined>;
  let fileHullGroup: d3.Selection<SVGGElement, unknown, null, undefined>;
  let linkSel: d3.Selection<SVGLineElement, D3Link, SVGGElement, unknown>;
  let linkLabelSel: d3.Selection<SVGGElement, D3Link, SVGGElement, unknown>;
  let orderBadgeSel: d3.Selection<SVGGElement, D3Link, SVGGElement, unknown>;
  let nodeSel: d3.Selection<SVGGElement, D3Node, SVGGElement, unknown>;
  let zoom: d3.ZoomBehavior<SVGSVGElement, unknown>;

  let unsubscribers: (() => void)[] = [];
  let initialized = false;
  /** Track what mode we last applied so resize knows whether to restart sim. */
  let currentMode: 'force' | 'tree' = 'force';
  /** Pending auto-fit timer — cleared when a new layout starts so we don't
   *  queue multiple fits on rapid plan changes. */
  let autoFitTimer: ReturnType<typeof setTimeout> | null = null;
  /** Nodes that arrived on a recent reload, and when their mark lapses.
   *  Survives a rebuild on purpose: a scope change mid-window should not
   *  erase the answer to "what just appeared" (UI-066). */
  let arrivals: ArrivalLog = new Map();
  /** One sweep timer for the whole log — see `msUntilNextExpiry`. */
  let arrivalTimer: ReturnType<typeof setTimeout> | null = null;
  /** Mirrors plan.selectedId so the force-tick and tree-positioning paths
   *  can decide whether to visually flip an edge's endpoints. The flip
   *  keeps the arrow head aligned with the reading direction of
   *  direction-aware labels ("inherited by", "called by", …): when the
   *  selection is the edge's target, we swap x1/x2 so the arrow lands on
   *  the OTHER node, and the passive-form label reads correctly. */
  let currentSelectedId: string | null = null;
  /** Per-link perpendicular offset for the LABEL, used when two edges
   *  connect the same pair of nodes in opposite directions (e.g.
   *  `class --contains--> method` plus `method --returns--> class`
   *  when the method returns `self`). Without this offset the two
   *  labels land on the same midpoint pixel and pile on top of each
   *  other. The map is keyed by the same "linkKeyFor" used for
   *  visibility classification so every label path can agree on the
   *  same offset. 0 = no reciprocal, +1/-1 = reciprocal present,
   *  sign chosen deterministically by kind_raw order so the two
   *  partners pick opposite sides of the line. */
  let labelOffsetDir: Map<string, number> = new Map();

  function rebuildLabelOffsets(data: import('../types/graph').GraphData): void {
    labelOffsetDir = new Map();
    const byPair = new Map<string, import('../types/graph').D3Link[]>();
    for (const l of data.links) {
      const s = typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
      const t = typeof l.target === 'object' ? (l.target as D3Node).id : l.target;
      const k = s < t ? `${s}::${t}` : `${t}::${s}`;
      let arr = byPair.get(k);
      if (!arr) { arr = []; byPair.set(k, arr); }
      arr.push(l);
    }
    for (const group of byPair.values()) {
      if (group.length < 2) continue;
      // Sort by kind_raw so the assignment is stable across renders —
      // otherwise a map insertion order change would flip the labels
      // left/right on every recompute.
      group.sort((a, b) => a.kind_raw.localeCompare(b.kind_raw));
      for (let i = 0; i < group.length; i++) {
        const l = group[i];
        // Alternate sides: -1, +1, -1, … so the first two reciprocals
        // land symmetrically around the line; additional duplicates
        // stack further out.
        const sign = i % 2 === 0 ? -1 : 1;
        const mag = Math.floor(i / 2) + 1;
        labelOffsetDir.set(linkKeyFor(l), sign * mag);
      }
    }
  }

  /** Offset a label midpoint perpendicular to the edge direction.
   *  Magnitude 14 px gives enough separation for a short label at the
   *  usual font sizes without drifting far from the line.
   *
   *  Critically, the perpendicular is computed from a canonical edge
   *  direction (sorted by node id) rather than from source → target.
   *  Reciprocal links (`A --contains--> B` plus `B --returns--> A`)
   *  have opposite source/target, which would flip the perpendicular
   *  too and cancel out the sign picked in `rebuildLabelOffsets`,
   *  landing both labels at the same point again. Anchoring to
   *  sorted-id direction guarantees both reciprocals see the same
   *  perpendicular vector — the sign then actually separates them. */
  function labelPerpOffset(d: D3Link, sx: number, sy: number, tx: number, ty: number): { dx: number; dy: number } {
    const dir = labelOffsetDir.get(linkKeyFor(d)) ?? 0;
    if (dir === 0) return { dx: 0, dy: 0 };
    const sId = typeof d.source === 'object' ? (d.source as D3Node).id : d.source;
    const tId = typeof d.target === 'object' ? (d.target as D3Node).id : d.target;
    // Normalise direction vector: always from the "smaller" id to the
    // "larger" id regardless of which is stored as source.
    const flip = sId > tId;
    const ex = flip ? sx - tx : tx - sx;
    const ey = flip ? sy - ty : ty - sy;
    const len = Math.hypot(ex, ey);
    if (len < 1) return { dx: 0, dy: 0 };
    return { dx: -ey / len * 14 * dir, dy: ex / len * 14 * dir };
  }
  /** True iff `d` should render with flipped endpoints. Two rules,
   *  checked in order:
   *
   *  1. Focal-on-edge rule (tree or graph with a selection that touches
   *     this edge): the focal node is the subject of the label, so the
   *     arrow must leave it outward. If the focal is the edge's target
   *     in the data, flip; if the focal is the source, keep as-is.
   *
   *  2. Hierarchy rule (focal not on this edge): arrow goes from the
   *     more-parent kind to the more-child kind per `kindPriority`. The
   *     stored data direction may have source=child (e.g. TakesParam
   *     stores param→function, but function is the parent), so we flip
   *     when source is "more child" than target. Same-priority pairs
   *     (e.g. Method→Method calls) fall back to the stored direction. */
  function isReversed(d: D3Link): boolean {
    const srcId = sourceId(d);
    const tgtId = targetId(d);
    if (currentSelectedId !== null) {
      if (tgtId === currentSelectedId) return true;
      if (srcId === currentSelectedId) return false;
    }
    const srcKind = typeof d.source === 'object' ? (d.source as D3Node).kind_raw : undefined;
    const tgtKind = typeof d.target === 'object' ? (d.target as D3Node).kind_raw : undefined;
    if (!srcKind || !tgtKind) return false;
    return isMoreChildThan(srcKind, tgtKind);
  }

  /**
   * Write both endpoints of every link in `sel`.
   *
   * `1` = tail, `2` = arrow head, flipped for reversed edges so
   * "inherited by"/"called by" labels read with the arrow rather than against
   * it. The head end stops at the rim of the node it points at rather than at
   * its centre — the arrow is a fixed-size marker anchored to that point, and
   * node radius is a user-chosen channel, so the setback has to be per link.
   *
   * One pass with direct attribute writes rather than four `.attr(fn)` calls:
   * this runs on every simulation tick, and the `.attr` form would walk the
   * selection four times and re-trim on each.
   */
  function positionLinks(
    sel: d3.Selection<SVGLineElement, D3Link, SVGGElement, unknown>,
    sx: (d: D3Link) => number, sy: (d: D3Link) => number,
    tx: (d: D3Link) => number, ty: (d: D3Link) => number,
  ): void {
    sel.each(function (d) {
      const rev = isReversed(d);
      const tailX = rev ? tx(d) : sx(d);
      const tailY = rev ? ty(d) : sy(d);
      const headEnd = rev ? d.source : d.target;
      // Still an id string until the simulation's first tick swaps in the
      // node object; no radius to trim to yet, and one frame later there is.
      const r = typeof headEnd === 'object' ? getNodeSize(headEnd as D3Node) : 0;
      const p = arrowHeadPoint(tailX, tailY, rev ? sx(d) : tx(d), rev ? sy(d) : ty(d), r);
      this.setAttribute('x1', String(tailX));
      this.setAttribute('y1', String(tailY));
      this.setAttribute('x2', String(p.x));
      this.setAttribute('y2', String(p.y));
    });
  }

  /**
   * UI-014 — the active metric→visual-channel mapping.
   *
   * Rebuilt whenever the data, the chosen channels, the aggregation level or
   * the theme changes. Every `r` / `fill` decision on the canvas goes through
   * this object; the rules themselves live in `viewmodels/nodeEncoding.ts`.
   *
   * Seeded with an empty node set so the accessors are safe to call before
   * the first graph lands (fitView's getBBox fallback can run that early).
   */
  let encoding: NodeEncoding = get(nodeEncoding);

  function getNodeSize(d: D3Node): number {
    return encoding.radius(d);
  }

  /** Collision radius for the force simulation.
   *
   *  Was a flat 40 while every node was ≤ 22px. With metric-driven sizing the
   *  largest node reaches 34px, so a constant would let big circles overlap
   *  while wasting space around small ones. Derived from the widest node in
   *  the current encoding plus room for the name label's first line. */
  function collisionRadius(): number {
    return encoding.maxRadius + 14;
  }

  /** Re-apply size- and colour-derived attributes to an already-rendered
   *  canvas. Used when the encoding changes but the data does not — switching
   *  channel or theme should not re-settle the layout. */
  function restyleEncoding(): void {
    if (!nodeSel) return;
    nodeSel.select<SVGCircleElement>('circle')
      .attr('r', (d) => encoding.radius(d))
      .attr('fill', (d) => encoding.fill(d))
      .attr('fill-opacity', (d) => encoding.fillOpacity(d));
    nodeSel.select<SVGTextElement>('.kind-label').attr('fill', (d) => encoding.labelInk(d));
    nodeSel.select<SVGTextElement>('.name-label').attr('dy', (d) => encoding.radius(d) + 12);
    // Both rings are sized off the radius the encoding just changed.
    applyArrivals();
    applyMarks();
    // So is every dot in the overview. Switching channel or theme has to
    // reach it, or the panel keeps describing the encoding the canvas left.
    publishOverview(true);
    simulation?.force('collision', d3.forceCollide().radius(collisionRadius()));
    // Arrow-head setbacks are measured from the radius that just changed, so
    // they are stale until something repositions the links. In force mode the
    // reheat below would get there eventually; in tree mode the simulation is
    // pinned and nothing else would, and either way "eventually" is a visible
    // frame of arrows sunk into or floating off the resized circles.
    if (linkSel) {
      positionLinks(
        linkSel,
        (d) => (d.source as D3Node).x ?? 0, (d) => (d.source as D3Node).y ?? 0,
        (d) => (d.target as D3Node).x ?? 0, (d) => (d.target as D3Node).y ?? 0,
      );
    }
    simulation?.alpha(0.2).restart();
  }

  // ── Arrival marks (UI-066) ────────────────────────────────────────────
  //
  // A node that appears on a live reload keeps a ring around it for
  // `ARRIVAL_HIGHLIGHT_MS`. The bookkeeping is in `utils/arrivals.ts`; what
  // is left here is painting it and one timer.

  /**
   * Paint the marks currently in the log.
   *
   * Idempotent, and deliberately not part of the enter selection: a
   * structural rebuild throws the DOM away mid-window, and re-deriving the
   * rings from the log means the marks come back with it. The ring is
   * *inserted* before the labels so `nodeSel.select('circle')` — which
   * `restyleEncoding` uses — still finds the node's own circle first.
   *
   * The `just-arrived` class carries no styling; it is the stable selector
   * for anything asking *which* nodes are marked without reaching for the
   * ring element.
   */
  function applyArrivals(): void {
    if (!nodeSel || !encoding) return;
    nodeSel.classed('just-arrived', (d) => arrivals.has(d.id));
    nodeSel.each(function (d: D3Node) {
      const group = d3.select(this);
      const ring = group.select<SVGCircleElement>('circle.arrival-ring');
      if (!arrivals.has(d.id)) {
        ring.remove();
        return;
      }
      if (ring.empty()) {
        group.insert('circle', 'text')
          .attr('class', 'arrival-ring')
          .attr('fill', 'none')
          .attr('pointer-events', 'none')
          .attr('r', encoding.radius(d) + 6);
      } else {
        ring.attr('r', encoding.radius(d) + 6);
      }
    });
  }

  /**
   * Ring every node whose scope is marked.
   *
   * Its own element rather than the node's own stroke, for the reason the
   * arrival ring is: a marked node that is also selected, searched or
   * diff-coloured has to keep saying all of those things, and there is exactly
   * one stroke to say them with. Sits inside the arrival ring at radius + 4 so
   * the two read as two rings when both apply.
   *
   * Matched on `file_path`, so at File level the file's own circle is ringed
   * and at Entity level every entity in it is — the mark is on the scope, and
   * both pictures are honest answers to "what did I pick".
   */
  function applyMarks(): void {
    if (!nodeSel || !encoding) return;
    const marks = get(markedPaths);
    nodeSel.classed('marked', (d) => isMarked(d, marks));
    nodeSel.each(function (d: D3Node) {
      const group = d3.select(this);
      const ring = group.select<SVGCircleElement>('circle.mark-ring');
      if (!isMarked(d, marks)) {
        ring.remove();
        return;
      }
      if (ring.empty()) {
        group.insert('circle', 'text')
          .attr('class', 'mark-ring')
          .attr('fill', 'none')
          .attr('pointer-events', 'none')
          .attr('r', encoding.radius(d) + 4);
      } else {
        ring.attr('r', encoding.radius(d) + 4);
      }
    });
  }

  /**
   * Record a batch of arrivals.
   *
   * Callers gate on the update being reload-driven. Widening a scope also
   * brings ids that were not there before, but those are nodes the user just
   * asked for — marking them would answer a question nobody asked and bury
   * the case the mark exists for.
   */
  function markArrivals(ids: string[]): void {
    if (!worthMarking(ids.length)) {
      if (ids.length) {
        console.log(`[GraphView] ${ids.length} new nodes — a rebuild, not an arrival; not marking`);
      }
      return;
    }
    noteArrivals(arrivals, ids, Date.now(), ARRIVAL_HIGHLIGHT_MS);
    scheduleArrivalSweep();
  }

  /** One timer for the whole log, re-aimed at the next deadline each sweep. */
  function scheduleArrivalSweep(): void {
    if (arrivalTimer) clearTimeout(arrivalTimer);
    arrivalTimer = null;
    const due = msUntilNextExpiry(arrivals, Date.now());
    if (due === null) return;
    arrivalTimer = setTimeout(() => {
      arrivalTimer = null;
      pruneArrivals(arrivals, Date.now());
      applyArrivals();
      scheduleArrivalSweep();
    }, due + 50);
  }
  /** Re-read the folder-cohesion strength and reheat.
   *
   *  A strength change is a layout change, so unlike `restyleEncoding` it
   *  does have to re-settle — but it is still not a rebuild: the same nodes
   *  move to new positions. In tree mode the simulation is deliberately
   *  stopped and the nodes are pinned to computed positions, so we record
   *  the new strength and leave the restart alone; it takes effect when the
   *  user returns to force mode. */
  function applyCohesion(): void {
    const force = simulation?.force('cohesion') as FolderCohesionForce | undefined;
    if (!force) return;
    force.strength(cohesionStrengthFor(get(graphLevel), get(folderCohesion)));
    if (currentMode !== 'force') return;
    simulation.alpha(0.3).restart();
  }

  // ── Folder hulls (UI-055) ──────────────────────────────────────────────

  /** Node ids the plan is currently drawing. A hull is computed from these
   *  rather than from the whole node set, or it would stretch to reach a
   *  filtered-out node and enclose empty canvas — which reads as a sparse
   *  folder rather than as a hidden one. */
  let hullNodeIds: Set<string> = new Set();

  /** Hulls recompute at most this often while the simulation runs. A hull
   *  trailing its nodes by a frame is imperceptible; recomputing a polygon
   *  per group on every one of ~60 ticks a second is not. */
  const HULL_INTERVAL_MS = 100;
  let lastHullDraw = 0;

  const hullPath = d3.line<[number, number]>()
    .x((p) => p[0])
    .y((p) => p[1])
    .curve(d3.curveCatmullRomClosed.alpha(0.5));

  function hullsEnabled(): boolean {
    if (!get(showFolderHulls)) return false;
    // Tree mode draws no hulls, as before. The layout is a strict hierarchy
    // and an outline over it fights the thing it is outlining.
    if (currentMode !== 'force') return false;
    // Module level: every node already *is* a folder, so outlining their
    // parent directories stacks a second grouping tier on the first with
    // nothing to tell them apart. Same rule as the cohesion force.
    if (get(graphLevel) === 'module') return false;
    return true;
  }

  function drawHulls(immediate = false): void {
    if (!fileHullGroup || !nodeSel) return;
    if (!hullsEnabled()) { fileHullGroup.selectAll('*').remove(); return; }
    const now = performance.now();
    if (!immediate && now - lastHullDraw < HULL_INTERVAL_MS) return;
    lastHullDraw = now;

    const drawn: D3Node[] = [];
    nodeSel.each((d: D3Node) => { if (hullNodeIds.has(d.id)) drawn.push(d); });
    // The whole chain, with the tier count passed separately (UI-070): a
    // region has to hold every node beneath it whatever depth is on show, or
    // its name promises more than its shape contains. `tiers: 1` is the
    // UI-055 picture — the tiers are additive, never a different answer for
    // the leaf regions.
    const hulls = computeFolderHulls(drawn, {
      radiusOf: (d) => encoding.radius(d),
      keysOf: (d) => {
        const key = folderKeyOf(d);
        return key === null ? [] : ancestorChainOf(key, Infinity);
      },
      tiers: get(hullDepth),
    });

    const sel = fileHullGroup.selectAll<SVGGElement, FolderHull>('g.folder-hull')
      .data(hulls, (h) => (h as FolderHull).path)
      .join(
        (enter) => {
          const grp = enter.append('g').attr('class', 'folder-hull');
          grp.append('path').attr('class', 'hull-shape');
          grp.append('text').attr('class', 'hull-label').attr('text-anchor', 'middle');
          return grp;
        },
        (update) => update,
        (exit) => exit.remove(),
      );

    // `data-path` and the parent class are read by the probe, which cannot
    // tell a region's ancestry from a label that carries only the last
    // segment — and got it wrong by guessing from the basename before.
    sel.attr('class', (h) => (h.hasChildren ? 'folder-hull parent' : 'folder-hull'))
      .attr('data-path', (h) => h.path);

    sel.select<SVGPathElement>('path.hull-shape')
      .attr('d', (h) => hullPath(h.points))
      .attr('fill', canvasColors.hullFill)
      // A parent gets no wash at all. Three nested fills at 6% each stack to
      // 17% over the innermost nodes, which is where a region starts shifting
      // what the metric-driven fills inside it appear to say — the thing
      // UI-055's achromatic decision exists to prevent. The outline and the
      // name carry the parent; only the leaf region is filled.
      .attr('fill-opacity', (h) => (h.hasChildren ? 0 : canvasColors.hullFillOpacity))
      .attr('stroke', canvasColors.hullStroke)
      .attr('stroke-opacity', canvasColors.hullStrokeOpacity)
      .attr('stroke-width', 1.5);

    // Upper-cased here rather than by `text-transform`, which SVG text
    // support for is uneven. Small caps plus the dimmer ink is what keeps a
    // region name from reading as an entity name; the extra size on a parent
    // is what makes it read as the heading over the regions inside it.
    sel.select<SVGTextElement>('text.hull-label')
      .attr('x', (h) => h.labelX)
      .attr('y', (h) => h.labelY)
      .attr('fill', canvasColors.hullLabelFill)
      .text((h) => h.label.toUpperCase());
  }

  function sourceId(link: D3Link): string {
    return typeof link.source === 'object' ? (link.source as D3Node).id : link.source;
  }
  function targetId(link: D3Link): string {
    return typeof link.target === 'object' ? (link.target as D3Node).id : link.target;
  }

  /**
   * UI-004 — relationship-tag-aware stroke styling for graph edges.
   *
   * Two helpers (one for stroke colour, one for dash pattern) so the
   * two link-render passes (initial mount at line ~546, enter() update
   * at ~711) stay in sync. The semantics:
   *
   * - `null_safe` → dashed stroke (visual hint that the call may not
   *   execute if the receiver is null). Pairs with the `rel-tag-null_safe`
   *   chip in EntityInfo so the panel and the graph carry the same
   *   non-colour-only signal.
   * - `bean_lookup` → lavender stroke matching the Bean entity colour
   *   so script→bean dependency lines read as one family across the
   *   graph (composes with UI-002).
   * - `dynamic_sql` / `dynamic_impex` / `unresolved` → amber dashed
   *   stroke as a "the parser knows this edge exists but couldn't
   *   open the box" warning marker.
   *
   * Returns/TakesParam keep their pre-existing dashed pattern — those
   * are kind-level distinctions independent of the new tag layer.
   */
  function linkStrokeColour(d: D3Link): string {
    const tags = d.tags ?? [];
    if (tags.includes('bean_lookup')) return '#9575CD';
    if (tags.includes('dynamic_sql') || tags.includes('dynamic_impex') || tags.includes('unresolved')) {
      return '#FFB74D';
    }
    return LINK_COLORS[d.kind_raw] || '#666';
  }

  function bindingSuffix(d: D3Link): string {
    if (d.binds_to) {
      return d.binds_type ? `  → ${d.binds_to}: ${d.binds_type}` : `  → ${d.binds_to}`;
    }
    if (d.rebinds_to) return `  ⟲ ${d.rebinds_to}`;
    return '';
  }

  function linkStrokeDashArray(d: D3Link): string | null {
    if (d.kind_raw === 'Returns' || d.kind_raw === 'TakesParam') return '6,3';
    const tags = d.tags ?? [];
    if (tags.includes('null_safe')) return '4,3';
    if (tags.includes('dynamic_sql') || tags.includes('dynamic_impex') || tags.includes('unresolved')) {
      return '2,3';
    }
    return null;
  }

  /**
   * Apply a freshly-computed displayPlan to the DOM. This is the ONLY
   * place that decides what's drawn — `applyFilters` and `applyTreeLayout`
   * no longer exist as discrete functions. The plan is a derived store, so
   * it recomputes atomically on any input change and we react to one event.
   */
  /** Share of visible edges one relationship kind must exceed before its
   *  labels are suppressed.
   *
   *  At file aggregation essentially every edge is the same kind, so
   *  labelling all of them produced ~68 identical "depends on" pills that
   *  overlapped each other and the node names — the only text on the canvas
   *  carrying information. Labelling the exceptions instead keeps the signal
   *  and drops the noise. 0.6 is deliberately not 0.9: a graph that is 60%
   *  one kind is already dominated by it. (UI-015) */
  const DOMINANT_KIND_SHARE = 0.6;

  /** Kind whose labels are currently suppressed, or null when the mix is
   *  varied enough that every label earns its place. */
  let suppressedKind: string | null = null;

  /** Retained so the showLinkLabels subscription can re-apply visibility
   *  without waiting for the next display-plan run. */
  let lastLinkVisible: ((d: D3Link) => string | null) | null = null;

  function computeSuppressedKind(plan: DisplayPlan): string | null {
    if (!linkSel) return null;
    // Graph mode only. The tree layout is sparse, its labels don't collide,
    // and they carry the hierarchy's meaning — suppressing there would remove
    // signal rather than noise.
    if (plan.mode === 'tree') return null;
    const counts = new Map<string, number>();
    let total = 0;
    linkSel.each((d: D3Link) => {
      if (!plan.visibleLinkKeys.has(linkKeyFor(d))) return;
      counts.set(d.kind, (counts.get(d.kind) ?? 0) + 1);
      total++;
    });
    if (total < 8) return null; // too few edges for clutter to be the problem
    for (const [kind, n] of counts) {
      if (n / total > DOMINANT_KIND_SHARE) return kind;
    }
    return null;
  }

  /** A label renders when labels are on, the edge is visible, and either its
   *  kind is not the dominant one or the edge touches the current selection
   *  — selecting a node should still explain its own edges. */
  function linkLabelDisplay(d: D3Link, planVisible: string | null): string | null {
    if (!get(showLinkLabels)) return 'none';
    if (planVisible === 'none') return 'none';
    if (suppressedKind && d.kind === suppressedKind) {
      const sel = currentSelectedId;
      const touchesSelection = sel != null && (sourceId(d) === sel || targetId(d) === sel);
      if (!touchesSelection) return 'none';
    }
    return planVisible;
  }

  function applyDisplayPlan(plan: DisplayPlan): void {
    if (!nodeSel || !linkSel) { console.log('[GraphView] applyDisplayPlan: skipped (no DOM)'); return; }
    console.log(`[GraphView] applyDisplayPlan: mode=${plan.mode} visibleNodes=${plan.visibleNodeIds.size} visibleLinks=${plan.visibleLinkKeys.size} selectedId=${plan.selectedId}`);
    const prevSelectedId = currentSelectedId;
    currentSelectedId = plan.selectedId ?? null;
    // In force mode the simulation may have cooled — nothing re-runs the
    // tick handler that reads `currentSelectedId` until the user interacts.
    // Re-apply edge endpoints once here so the arrow flip happens
    // immediately on selection change. In tree mode the tree branch
    // re-runs `positionEdgesAndLabels`, which already handles reversal.
    if (currentMode === 'force' && linkSel && prevSelectedId !== currentSelectedId) {
      positionLinks(
        linkSel,
        (d) => (d.source as D3Node).x ?? 0, (d) => (d.source as D3Node).y ?? 0,
        (d) => (d.target as D3Node).x ?? 0, (d) => (d.target as D3Node).y ?? 0,
      );
    }

    // Visibility: nodes. Visible nodes get opacity 1, dimmed nodes get
    // the user-controlled dim opacity, fully hidden nodes get display:none.
    // From the plan, not the diff store: the plan knows which rule dimmed
    // these nodes and therefore which opacity it meant (UI-050).
    const dimOpacity = plan.dimOpacity;
    nodeSel
      .style('display', (d) => {
        if (plan.visibleNodeIds.has(d.id)) return null;
        if (plan.dimmedNodeIds.has(d.id) && dimOpacity > 0) return null;
        return 'none';
      })
      .attr('opacity', (d) => {
        if (plan.visibleNodeIds.has(d.id)) return 1;
        if (plan.dimmedNodeIds.has(d.id)) return dimOpacity;
        return 0;
      });

    // Visibility: links + labels + badges. Show links where both endpoints
    // are visible or dimmed (when dimOpacity > 0).
    const nodeShown = (id: string) =>
      plan.visibleNodeIds.has(id) || (plan.dimmedNodeIds.has(id) && dimOpacity > 0);
    const lkVis = (d: D3Link) => {
      if (plan.visibleLinkKeys.has(linkKeyFor(d))) return null;
      // Show links between dimmed nodes at reduced opacity
      if (dimOpacity > 0 && nodeShown(sourceId(d)) && nodeShown(targetId(d))) return null;
      return 'none';
    };
    linkSel.style('display', lkVis).attr('opacity', (d: D3Link) => {
      if (plan.visibleLinkKeys.has(linkKeyFor(d))) return 1;
      return dimOpacity;
    });
    suppressedKind = computeSuppressedKind(plan);
    lastLinkVisible = lkVis;
    linkLabelSel.style('display', (d: D3Link) => linkLabelDisplay(d, lkVis(d)))
      .attr('opacity', (d: D3Link) => {
        if (plan.visibleLinkKeys.has(linkKeyFor(d))) return 1;
        return dimOpacity;
      });
    orderBadgeSel.style('display', lkVis).attr('opacity', (d: D3Link) => {
      if (plan.visibleLinkKeys.has(linkKeyFor(d))) return 1;
      return dimOpacity;
    });

    // Direction-aware edge labels: follow the same `isReversed` predicate
    // that the arrow-flip uses, so arrow direction and label perspective
    // stay in lock-step. Reversed edges use the passive/incoming form
    // ("calls" → "called by"); forward edges use the active form.
    // Applied unconditionally (not just when a node is selected) so the
    // hierarchy-based flip in graph view also picks up the passive label.
    linkLabelSel.select('text').text((d) => {
      const label = isReversed(d) ? (d.incoming_kind || d.kind) : d.kind;
      const base = d.order ? `${d.order}. ${label}` : label;
      const withWeight = d.weight && d.weight > 1 ? `${base} (${d.weight})` : base;
      return `${withWeight}${bindingSuffix(d)}`;
    });

    // Highlight classes
    const searchActive = plan.searchMatched.size > 0;
    nodeSel.classed('search-match', (d) => searchActive && plan.searchMatched.has(d.id));
    nodeSel.classed('search-neighbor',
      (d) => searchActive && !plan.searchMatched.has(d.id) && plan.searchNeighbors.has(d.id));
    // Display-search highlight is applied by a separate subscription (see
    // onMount) so it stays outside the displayPlan feedback loop.
    nodeSel.classed('selected', (d) => d.id === plan.selectedId);

    // Diff mode: color node strokes by change status when active.
    // Core changes get solid stroke, impact-only changes get dashed stroke.
    if ($diffActive) {
      const statusMap = get(diffStatusMap);
      const sourceMap = get(diffSourceChangedMap);
      nodeSel.select('circle')
        .attr('stroke', (d) => {
          // Use normalizeEntityId to strip temp worktree paths for stable matching.
          const status = statusMap?.get(normalizeEntityId(d.original_id));
          if (status && DIFF_COLORS[status]) return DIFF_COLORS[status];
          return d.id === plan.selectedId ? '#e94560' : '#fff';
        })
        .attr('stroke-width', (d) => {
          const status = statusMap?.get(normalizeEntityId(d.original_id));
          return (status === 'added' || status === 'removed' || status === 'modified') ? 4 : 2;
        })
        .attr('stroke-dasharray', (d) => {
          const status = statusMap?.get(normalizeEntityId(d.original_id));
          const isCore = sourceMap?.get(normalizeEntityId(d.original_id));
          // Dashed stroke for modified entities that are impact-only (not core changes).
          if (status === 'modified' && isCore === false) return '4,2';
          return null;
        });
    }

    // Layout
    const w = container.clientWidth;
    const h = container.clientHeight;
    if (plan.mode === 'tree') {
      currentMode = 'tree';
      simulation?.stop();
      // No file hulls in the new tree mode — they clashed with the strict
      // hierarchical layout. Can be reintroduced when needed.
      fileHullGroup?.selectAll('*').remove();

      const cx = w / 2;
      const cy = h / 2;
      const posOf = (id: string) => {
        const p = plan.treePositions.get(id);
        return p ? { x: cx + p.x, y: cy + p.y } : null;
      };

      // Pin and animate visible nodes to their tree positions. The
      // .attr('opacity', 1) ensures nodes that just entered via an
      // incremental updateGraph (which starts them at opacity 0) become
      // visible — without it, this transition cancels the enter fade-in
      // and the node stays invisible.
      nodeSel
        .filter((d) => plan.visibleNodeIds.has(d.id))
        .transition().duration(600)
        .attr('opacity', 1)
        .attr('transform', (d) => {
          const p = posOf(d.id);
          if (!p) return `translate(${d.x ?? 0},${d.y ?? 0})`;
          d.fx = p.x; d.fy = p.y; d.x = p.x; d.y = p.y;
          return `translate(${p.x},${p.y})`;
        });

      // Position links + labels deterministically once the node transition
      // has had a chance to land. Guarded against teardown — if `linkSel`
      // gets nulled while the timer is pending we no-op.
      const positionEdgesAndLabels = () => {
        if (!initialized || !linkSel) return;
        const sx = (d: D3Link) => posOf(sourceId(d))?.x ?? (d.source as D3Node).x ?? 0;
        const sy = (d: D3Link) => posOf(sourceId(d))?.y ?? (d.source as D3Node).y ?? 0;
        const tx = (d: D3Link) => posOf(targetId(d))?.x ?? (d.target as D3Node).x ?? 0;
        const ty = (d: D3Link) => posOf(targetId(d))?.y ?? (d.target as D3Node).y ?? 0;
        positionLinks(linkSel, sx, sy, tx, ty);
        linkLabelSel.attr('transform', (d) => {
          const mx = (sx(d) + tx(d)) / 2;
          const my = (sy(d) + ty(d)) / 2;
          const off = labelPerpOffset(d, sx(d), sy(d), tx(d), ty(d));
          return `translate(${mx + off.dx},${my + off.dy})`;
        });
        // Order badges sit 25 % from the visual tail so they stay on the
        // opposite side of the arrow head, regardless of direction flip.
        orderBadgeSel.attr('transform', (d) => {
          const [hx, hy, tx2, ty2] = isReversed(d)
            ? [tx(d), ty(d), sx(d), sy(d)]
            : [sx(d), sy(d), tx(d), ty(d)];
          return `translate(${hx * 0.75 + tx2 * 0.25},${hy * 0.75 + ty2 * 0.25})`;
        });
      };
      setTimeout(positionEdgesAndLabels, 650);

      // Auto-fit after tree layout settles
      if ($autoFitView) {
        if (autoFitTimer) clearTimeout(autoFitTimer);
        autoFitTimer = setTimeout(() => { autoFitTimer = null; fitView(); }, 700);
      }
    } else {
      // Force mode. If we were pinned (tree mode), release fx/fy and kick
      // the simulation. Otherwise this is a no-op layout-wise — visibility
      // changes above are enough for a pure filter update.
      const wasTreeOrPinned = currentMode === 'tree';
      currentMode = 'force';
      if (wasTreeOrPinned) {
        nodeSel.each((d) => { d.fx = null; d.fy = null; });
        fileHullGroup?.selectAll('*').remove();
        simulation?.alpha(0.5).restart();

        // Auto-fit after force simulation has time to settle
        if ($autoFitView) {
          if (autoFitTimer) clearTimeout(autoFitTimer);
          autoFitTimer = setTimeout(() => { autoFitTimer = null; fitView(); }, 1200);
        }
      }
    }

    // Hulls last: `hullsEnabled` reads `currentMode`, which the branch above
    // is what sets. Immediate rather than throttled — a filter change is a
    // user action, and a region that keeps its old outline for a tenth of a
    // second after its nodes vanish is visible as a glitch.
    hullNodeIds = new Set<string>();
    for (const id of plan.visibleNodeIds) hullNodeIds.add(id);
    if (dimOpacity > 0) for (const id of plan.dimmedNodeIds) hullNodeIds.add(id);
    drawHulls(true);

    // Immediate for the same reason, and it is the *only* publish tree mode
    // gets: the simulation is stopped there, so no tick handler will follow
    // up. The tree branch above writes each node's final `d.x`/`d.y` inside
    // the transition's attribute callback, which has already run by here, so
    // these are the settled positions rather than the ones being animated
    // away from.
    publishOverview(true);
  }

  /** Hover-only dimming. BFS from the hovered node up to `$hoverDepth`
   *  hops — everything outside the reachable set is dimmed. */
  function highlightConnections(d: D3Node) {
    const depth = $hoverDepth;
    const links = $graphData.links;

    // Build adjacency list for efficient BFS
    const adj = new Map<string, string[]>();
    for (const l of links) {
      const src = sourceId(l); const tgt = targetId(l);
      if (!adj.has(src)) adj.set(src, []);
      if (!adj.has(tgt)) adj.set(tgt, []);
      adj.get(src)!.push(tgt);
      adj.get(tgt)!.push(src);
    }

    // BFS: collect all nodes reachable within `depth` hops
    const reachable = new Set<string>([d.id]);
    let frontier = [d.id];
    for (let deg = 1; deg <= depth; deg++) {
      const next: string[] = [];
      for (const nid of frontier) {
        for (const neighbor of (adj.get(nid) || [])) {
          if (!reachable.has(neighbor)) {
            reachable.add(neighbor);
            next.push(neighbor);
          }
        }
      }
      frontier = next;
    }

    // Dim everything outside the reachable set
    nodeSel.classed('dimmed', (n) => !reachable.has(n.id));
    const edgeReachable = (l: D3Link) => reachable.has(sourceId(l)) && reachable.has(targetId(l));
    linkSel.classed('dimmed', (l) => !edgeReachable(l));
    linkLabelSel.classed('dimmed', (l) => !edgeReachable(l));
    orderBadgeSel.classed('dimmed', (l) => !edgeReachable(l));
  }

  /** Hover-only dimming by *membership* rather than by reachability
   *  (UI-054): everything sharing the hovered node's folder stays lit.
   *
   *  Shares `resetHighlight` and the `.dimmed` class with the connections
   *  path deliberately — both are hover-scoped probes that clear on
   *  mouseout, and giving them separate visual treatments would make the
   *  canvas say two things where the user asked one question. */
  function highlightGroup(d: D3Node) {
    const members = groupMemberIds($graphData.nodes, d, folderKeyOf);
    // A ghost is in no group. Lighting up every other ghost would invent a
    // grouping that does not exist, so the honest answer is to light nothing
    // — and leaving the canvas untouched says that more clearly than dimming
    // the entire graph would.
    if (members.size === 0) return;
    nodeSel.classed('dimmed', (n) => !members.has(n.id));
    const edgeInGroup = (l: D3Link) => members.has(sourceId(l)) && members.has(targetId(l));
    linkSel.classed('dimmed', (l) => !edgeInGroup(l));
    linkLabelSel.classed('dimmed', (l) => !edgeInGroup(l));
    orderBadgeSel.classed('dimmed', (l) => !edgeInGroup(l));
  }

  /** Dispatch a hover to whichever question the user is asking. Module level
   *  falls back to connections: every node there already *is* a folder, so
   *  "what else lives here" has no answer to give. */
  function highlightHover(d: D3Node) {
    if ($hoverMode === 'group' && get(graphLevel) !== 'module') highlightGroup(d);
    else highlightConnections(d);
  }

  function resetHighlight() {
    nodeSel.classed('dimmed', false);
    linkSel.classed('dimmed', false);
    linkLabelSel.classed('dimmed', false);
    orderBadgeSel.classed('dimmed', false);
  }

  // --- Overview panel (UI-083) ---
  //
  // Two publishes, deliberately separate, because they change at completely
  // different rates and for different reasons. The viewport is one small
  // object written on a zoom or a resize — user-driven, so at most a few
  // hundred a second and each one worth honouring immediately. The dots are
  // up to a render budget's worth of objects rebuilt off the simulation,
  // which fires ~60 times a second for as long as the layout is settling, and
  // a panel a couple of frames behind the canvas during a settle is
  // indistinguishable from one that isn't.

  /** Same throttle as the folder hulls, for the same reason: this reads every
   *  drawn node on a tick handler that has to stay cheap. */
  const OVERVIEW_INTERVAL_MS = 100;
  let lastOverviewPublish = 0;

  /** Where the viewport is, for the overview's box. Takes the transform as an
   *  argument rather than reading it back off the element, because the zoom
   *  handler already has it and `d3.zoomTransform` during a transition
   *  returns the interpolated value a frame late. */
  function publishViewport(t: d3.ZoomTransform | { k: number; x: number; y: number }): void {
    if (!container) return;
    canvasViewport.set({ k: t.k, x: t.x, y: t.y, w: container.clientWidth, h: container.clientHeight });
  }

  /**
   * The drawn nodes as flat world-space dots.
   *
   * Position, radius and fill come from the same `encoding` the canvas circles
   * were drawn with, so the panel is a picture of *this* graph rather than a
   * second opinion about it — the reader is meant to recognise one in the
   * other, and a panel with its own size rule would defeat that.
   *
   * `display: none` is the same skip `visibleNodeExtents` makes: a filtered
   * node is not on the canvas, so the overview claiming it would put the box
   * around a region holding nothing.
   */
  function publishOverview(immediate = false): void {
    if (!nodeSel) { overviewDots.set([]); return; }
    const now = performance.now();
    if (!immediate && now - lastOverviewPublish < OVERVIEW_INTERVAL_MS) return;
    lastOverviewPublish = now;

    const dots: OverviewDot[] = [];
    nodeSel.each(function (d: D3Node) {
      if ((this as SVGGElement).style.display === 'none') return;
      if (d.x == null || d.y == null || !isFinite(d.x) || !isFinite(d.y)) return;
      dots.push({
        x: d.x,
        y: d.y,
        r: encoding.radius(d),
        fill: encoding.fill(d),
        opacity: encoding.fillOpacity(d),
      });
    });
    overviewDots.set(dots);
  }

  /** Centre the viewport on a world point, keeping the current scale. The
   *  overview's click and drag both land here. No transition: a drag has to
   *  track the pointer, and a 500ms ease per pointermove would queue up
   *  behind itself and lag the box off the cursor. */
  export function panTo(wx: number, wy: number): void {
    if (!svg || !zoom || !container) return;
    const t = d3.zoomTransform(svgEl);
    const next = centreTransform(wx, wy, t.k, container.clientWidth, container.clientHeight);
    svg.call(zoom.transform, d3.zoomIdentity.translate(next.x, next.y).scale(next.k));
  }

  // --- Public methods for controls ---
  export function zoomIn() { svg.transition().call(zoom.scaleBy, 1.5); }
  export function zoomOut() { svg.transition().call(zoom.scaleBy, 0.67); }
  export function resetZoom() { svg.transition().call(zoom.transform, d3.zoomIdentity); }

  /** Fit the viewport to the currently visible nodes with padding. */
  /** World-space extents of every rendered node, including its labels.
   *
   *  The old approach approximated each node as `getNodeSize(d) + 20`, which
   *  is the circle plus a fixed pad. Name labels are centre-anchored and
   *  extend far wider than that — a 24-character filename at 11px overhangs
   *  the circle by ~65px each side — so fitting to the approximation left
   *  labels cut off at the viewport edge even though every circle was
   *  inside it. 11 nodes clipped at 1600x1000, 23 at 1280x800 (UI-022).
   *
   *  getBBox() on the node's <g> returns the union of circle and labels in
   *  the group's own coordinates, i.e. offsets from the node centre, which
   *  is exactly what we need to add to (d.x, d.y). One layout read per node
   *  per fit — cheap, and only on an explicit fit. */
  function visibleNodeExtents(): { minX: number; minY: number; maxX: number; maxY: number } | null {
    if (!nodeSel) return null;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    let seen = 0;
    nodeSel.each(function (d: D3Node) {
      const el = this as SVGGElement;
      if (el.style.display === 'none') return;
      if (d.x == null || d.y == null || !isFinite(d.x) || !isFinite(d.y)) return;
      let left: number, top: number, right: number, bottom: number;
      try {
        const bb = el.getBBox();
        left = d.x + bb.x; top = d.y + bb.y;
        right = left + bb.width; bottom = top + bb.height;
      } catch {
        // getBBox throws on a node that isn't rendered yet; fall back to the
        // circle plus a pad rather than dropping it from the fit entirely.
        const r = getNodeSize(d) + 20;
        left = d.x - r; top = d.y - r; right = d.x + r; bottom = d.y + r;
      }
      if (left < minX) minX = left;
      if (top < minY) minY = top;
      if (right > maxX) maxX = right;
      if (bottom > maxY) maxY = bottom;
      seen++;
    });
    return seen > 0 ? { minX, minY, maxX, maxY } : null;
  }

  export function fitView() {
    if (!nodeSel || !svg || !zoom) return;
    const ext = visibleNodeExtents();
    if (!ext) return;
    const { minX, minY, maxX, maxY } = ext;
    // Read the canvas size at fit time — a width captured earlier goes stale
    // as soon as a side panel is collapsed or resized.
    const w = container.clientWidth;
    const h = container.clientHeight;
    const bboxW = Math.max(1, maxX - minX);
    const bboxH = Math.max(1, maxY - minY);
    const margin = 40;
    const scale = Math.min((w - margin * 2) / bboxW, (h - margin * 2) / bboxH, 4);
    const safeScale = Math.max(0.1, scale);
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const tx = w / 2 - cx * safeScale;
    const ty = h / 2 - cy * safeScale;
    svg.transition().duration(500)
      .call(zoom.transform, d3.zoomIdentity.translate(tx, ty).scale(safeScale));
  }

  /** Fit the viewport so the widest tree level fills the screen width.
   *  Vertical overflow is expected and handled by panning. */
  export function fitWidth() {
    if (!nodeSel || !svg || !zoom) return;
    const ext = visibleNodeExtents();
    if (!ext) return;
    const { minX, minY, maxX, maxY } = ext;
    const w = container.clientWidth;
    const h = container.clientHeight;
    const bboxW = Math.max(1, maxX - minX);
    const margin = 40;
    const scale = Math.min((w - margin * 2) / bboxW, 4);
    const safeScale = Math.max(0.1, scale);
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const tx = w / 2 - cx * safeScale;
    const ty = h / 2 - cy * safeScale;
    svg.transition().duration(500)
      .call(zoom.transform, d3.zoomIdentity.translate(tx, ty).scale(safeScale));
  }

  export function toggleViewMode() {
    viewMode.update((m) => m === 'graph' ? 'tree' : 'graph');
  }

  /**
   * The nodes and links the plan actually puts on screen.
   *
   * Dimmed nodes count as drawn — they render at reduced opacity rather than
   * being removed, so dropping them here would delete the diff and search
   * context the dimming exists to provide.
   *
   * Over the draw ceiling this is empty, which is what turns the overflow
   * card from a cover over a fully-built canvas into an actual refusal to
   * build one.
   */
  function renderSetFor(
    plan: DisplayPlan,
    data: import('../types/graph').GraphData,
  ): import('../types/graph').GraphData {
    const empty = { nodes: [], links: [], files: data.files, modules: data.modules };
    if (plan.overflow) return empty;
    // Same definition the ceiling is measured against — see `drawnIdsOf`.
    // If these two ever disagree the gate is charging for a different set
    // than the one being built, which is the bug UI-061 was about.
    const keep = drawnIdsOf(plan);
    if (keep.size === 0) return empty;
    const nodes = data.nodes.filter((n) => keep.has(n.id));
    // Both endpoint checks matter: `visibleLinkKeys` is computed against the
    // visible set, and a dimmed endpoint can leave a key whose other end is
    // filtered out. A link to a node that was never built renders as a line
    // into empty space.
    const links = data.links.filter(
      (l) => plan.visibleLinkKeys.has(linkKeyFor(l)) && keep.has(sourceId(l)) && keep.has(targetId(l)),
    );
    return { nodes, links, files: data.files, modules: data.modules };
  }

  function teardownGraph() {
    if (autoFitTimer) { clearTimeout(autoFitTimer); autoFitTimer = null; }
    simulation?.stop();
    simulation = null as any;
    if (svg) {
      svg.on('.zoom', null);
      svg.on('click', null);
      svg.selectAll('*').remove();
    }
    // An empty frame is what folds the overview away. Without this, tearing
    // the canvas down over the draw ceiling would leave the panel showing the
    // graph that is no longer there — the one picture guaranteed to be wrong.
    overviewDots.set([]);
    nodeSel = null as any;
    linkSel = null as any;
    linkLabelSel = null as any;
    orderBadgeSel = null as any;
    fileHullGroup = null as any;
    initialized = false;
  }

  function initGraph(data: import('../types/graph').GraphData) {
    canvasColors = canvasChrome();
    encoding = get(nodeEncoding);
    rebuildLabelOffsets(data);
    const w = container.clientWidth;
    const h = container.clientHeight;

    // Reset positional state so node objects shared across selections
    // don't carry stale fx/fy or vx/vy from a previous render.
    //
    // UI-053: the reset seeds from the folder tree rather than clearing x/y.
    // Cleared coordinates hand the starting arrangement to d3's default
    // phyllotaxis spiral, which is ordered by array index — an ordering that
    // carries no structural information and changes whenever the node set
    // does. Since a force layout converges near where it began, that made
    // the settled picture partly an artefact of array order, and made the
    // same repo look different on consecutive loads.
    const seeder = makeSeeder(data.nodes, w, h);
    data.nodes.forEach((n) => {
      const p = seeder.position(n);
      n.x = p.x; n.y = p.y;
      (n as any).vx = 0; (n as any).vy = 0;
      n.fx = null; n.fy = null;
    });

    svg = d3.select(svgEl).attr('width', w).attr('height', h);
    g = svg.append('g');
    fileHullGroup = g.append('g').attr('class', 'file-hulls');

    zoom = d3.zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.1, 4])
      .on('zoom', (event) => {
        g.attr('transform', event.transform);
        publishViewport(event.transform);
      });
    svg.call(zoom);
    svg.call(zoom.transform, d3.zoomIdentity);

    svg.on('click', (event) => {
      if (event.target === svgEl) selectedNode.set(null);
    });

    // `markerUnits: userSpaceOnUse` is the load-bearing attribute: without it
    // SVG scales the head by the line's stroke-width, and `positionLinks`
    // would be trimming to a rim the arrow then overshoots by its own growth.
    // `refX = ARROW_LEN` puts the tip — not the base — at the line's end,
    // which is the point `arrowHeadPoint` computed. See viewmodels/linkGeometry.ts.
    svg.append('defs').selectAll('marker').data(['arrow']).join('marker')
      .attr('id', 'arrow').attr('viewBox', '0 -5 10 10').attr('refX', ARROW_LEN)
      .attr('refY', 0).attr('markerUnits', 'userSpaceOnUse')
      .attr('markerWidth', ARROW_LEN).attr('markerHeight', ARROW_LEN).attr('orient', 'auto')
      .append('path').attr('fill', canvasColors.arrowFill).attr('d', 'M0,-4L10,0L0,4');

    simulation = d3.forceSimulation<D3Node>(data.nodes)
      .force('link', d3.forceLink<D3Node, D3Link>(data.links).id((d) => d.id).distance(120))
      .force('charge', d3.forceManyBody().strength(-400))
      .force('center', d3.forceCenter(w / 2, h / 2))
      // Weak pull toward the centre on both axes. forceCenter only
      // translates the centroid — it does not stop a disconnected node from
      // drifting arbitrarily far under charge repulsion, and nothing else
      // pulls it back. With ~40 file nodes that produced a 2580x2719 span
      // whose outliers were all zero-degree files (vite.config.ts, main.ts,
      // the synthetic root), which drove auto-fit into its 0.1 zoom floor
      // and left the real cluster unreadably small. Weak enough that
      // connected structure still dominates the layout.
      .force('x', d3.forceX(w / 2).strength(0.05))
      .force('y', d3.forceY(h / 2).strength(0.05))
      .force('collision', d3.forceCollide().radius(collisionRadius()))
      // UI-052. Everything above is edge-driven or global; this is the only
      // force that knows two nodes live in the same folder. Strength is a
      // user setting, and `cohesionStrengthFor` zeroes it at Module level
      // where each node already is a folder. See utils/forceCohesion.ts.
      .force('cohesion', forceFolderCohesion()
        .strength(cohesionStrengthFor(get(graphLevel), get(folderCohesion))));

    linkSel = g.append('g').attr('class', 'links-group').selectAll('line').data(data.links).join('line')
      .attr('class', 'link')
      .attr('stroke', (d) => linkStrokeColour(d))
      .attr('stroke-width', (d) => linkStrokeWidth(d.weight))
      .attr('stroke-dasharray', (d) => linkStrokeDashArray(d))
      .attr('stroke-opacity', 0.6)
      .attr('marker-end', 'url(#arrow)');
    linkSel.append('title').text((d) => {
      if (!d.breakdown || d.weight == null) return d.kind;
      const parts = Object.entries(d.breakdown)
        .sort((a, b) => b[1] - a[1])
        .map(([k, n]) => `${n} × ${k}`);
      // Singular matters here now: UI-058 puts merged edges of weight 1 on
      // screen routinely, where before they only appeared in fully collapsed
      // views alongside plenty of plurals.
      const noun = d.weight === 1 ? 'relationship' : 'relationships';
      return `${d.kind} — ${d.weight} underlying ${noun} (${parts.join(', ')})`;
    });

    const linkLabelGroup = g.append('g').attr('class', 'link-labels');
    linkLabelSel = linkLabelGroup.selectAll('g').data(data.links).join('g').attr('class', 'link-label-container');
    linkLabelSel.append('rect').attr('class', 'link-label-bg').attr('rx', 3).attr('ry', 3)
      .attr('fill', canvasColors.linkLabelBg).attr('fill-opacity', canvasColors.linkLabelBgOpacity);
    linkLabelSel.append('text').attr('class', 'link-label')
      .text((d) => {
        const base = d.order ? `${d.order}. ${d.kind}` : d.kind;
        const withWeight = d.weight && d.weight > 1 ? `${base} (${d.weight})` : base;
        return `${withWeight}${bindingSuffix(d)}`;
      })
      .attr('fill', (d) => LINK_COLORS[d.kind_raw] || '#888')
      .attr('font-size', '9px').attr('text-anchor', 'middle').attr('dominant-baseline', 'middle')
      .attr('pointer-events', 'none');
    linkLabelSel.each(function () {
      const text = d3.select(this).select('text');
      const bbox = (text.node() as SVGTextElement).getBBox();
      d3.select(this).select('rect')
        .attr('x', -bbox.width / 2 - 4).attr('y', -bbox.height / 2 - 2)
        .attr('width', bbox.width + 8).attr('height', bbox.height + 4);
    });

    const orderedLinks = data.links.filter((d) => d.order != null);
    const orderBadgesGroup = g.append('g').attr('class', 'order-badges');
    orderBadgeSel = orderBadgesGroup.selectAll('g').data(orderedLinks).join('g').attr('class', 'order-badge-container');
    orderBadgeSel.append('circle').attr('r', 8).attr('fill', '#FF9800').attr('stroke', canvasColors.orderBadgeStroke).attr('stroke-width', 1.5);
    orderBadgeSel.append('text').attr('text-anchor', 'middle').attr('dominant-baseline', 'central')
      .attr('fill', canvasColors.orderBadgeText).attr('font-size', '8px').attr('font-weight', '700').attr('pointer-events', 'none')
      .text((d) => d.order!);

    nodeSel = g.append('g').attr('class', 'nodes-group').selectAll('g').data(data.nodes).join('g')
      .attr('class', (d) => 'node' + (d.tags?.includes('trait_impl') ? ' trait-impl' : '') + (d.tags?.includes('ghost') ? ' ghost' : ''))
      .call(d3.drag<SVGGElement, D3Node>()
        .on('start', (event, d) => { if (!event.active) simulation.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
        .on('drag', (event, d) => { d.fx = event.x; d.fy = event.y; })
        .on('end', (event, d) => { if (!event.active) simulation.alphaTarget(0); d.fx = null; d.fy = null; }))
      .on('click', (event, d) => onNodeClick(event, d))
      .on('dblclick', (event, d) => onNodeDoubleClick(event, d))
      .on('mouseover', (_, d) => {
        highlightHover(d);
        if (!$hoverLocked) hoveredNode.set(d);
      })
      .on('mouseout', () => {
        resetHighlight();
        if (!$hoverLocked) hoveredNode.set(null);
      });

    nodeSel.append('circle')
      .attr('r', (d) => encoding.radius(d))
      .attr('fill', (d) => encoding.fill(d))
      .attr('fill-opacity', (d) => encoding.fillOpacity(d))
      .attr('stroke', canvasColors.nodeStroke).attr('stroke-width', 2);
    nodeSel.append('text').attr('class', 'kind-label')
      .attr('text-anchor', 'middle').attr('dominant-baseline', 'central')
      .attr('font-size', '8px').attr('font-weight', '700')
      .attr('fill', (d) => encoding.labelInk(d))
      .attr('pointer-events', 'none')
      .text((d) => KIND_CODES[d.kind_raw] || '??');
    nodeSel.append('text').attr('class', 'name-label')
      .attr('dy', (d) => encoding.radius(d) + 12).attr('text-anchor', 'middle')
      .attr('font-size', '11px').attr('fill', canvasColors.nameLabelFill).attr('pointer-events', 'none')
      .text((d) => d.name);

    // A rebuild in the middle of an arrival window keeps its rings, and a
    // rebuild under a marked set keeps that too — a scope or level change is
    // exactly when the reader is relying on the marks still being there.
    applyArrivals();
    applyMarks();

    attachTickHandler();
    // Seed the overview off the layout seed rather than waiting for the first
    // tick to clear the throttle. The previous graph's publish can be less
    // than an interval old, and a panel still showing the graph before last
    // for a tenth of a second is worse than one showing the pre-settle
    // arrangement of the right one.
    publishOverview(true);
  }

  /** Re-apply theme-derived colours to an already-rendered canvas.
   *  Cheaper and less disruptive than a full re-render, and it keeps the
   *  force simulation's positions rather than re-settling the layout. */
  function restyleCanvasChrome(): void {
    canvasColors = canvasChrome();
    if (!svg || !g) return;
    svg.selectAll('defs marker path').attr('fill', canvasColors.arrowFill);
    g.selectAll('.link-label-bg')
      .attr('fill', canvasColors.linkLabelBg)
      .attr('fill-opacity', canvasColors.linkLabelBgOpacity);
    g.selectAll('.order-badge-container circle').attr('stroke', canvasColors.orderBadgeStroke);
    g.selectAll('.order-badge-container text').attr('fill', canvasColors.orderBadgeText);
    g.selectAll('.node circle').attr('stroke', canvasColors.nodeStroke);
    g.selectAll('.node .name-label').attr('fill', canvasColors.nameLabelFill);
    // The severity ramp is per-mode — the light theme gets its own validated
    // steps rather than an inversion of the dark ones — so a theme change is
    // an encoding change, not just a chrome change. `restyleEncoding` also
    // owns the kind-label ink now: it is chosen against the node's own fill,
    // not from the theme, so re-applying `canvasColors.kindLabelFill` here
    // would put white back on a pale ramp step.
    if (nodeSel) {
      encoding = get(nodeEncoding);
      restyleEncoding();
    }
    // Hull fill, stroke and label ink are all theme-derived and baked onto
    // attributes at join time, so they go stale on a theme swap exactly the
    // way the rest of the canvas chrome does (UI-009).
    drawHulls(true);
  }

  function attachTickHandler() {
    // Auto-fit when the force layout actually settles.
    //
    // A fixed timer can't work here: the simulation keeps spreading nodes
    // after the timer fires, so the graph outgrows whatever was fitted. The
    // only two auto-fit triggers used to live in applyDisplayPlan, and the
    // force-mode one was gated on having just come *from* tree mode — so
    // selecting a scope in graph mode, the most common path of all, never
    // fitted at all. Namespaced so it doesn't clobber another 'end' handler.
    simulation.on('end.autofit', () => {
      if (get(autoFitView) && currentMode === 'force') fitView();
      // The throttle can swallow the last tick's worth of movement, so the
      // outline would keep a shape the nodes have already left. Redraw once
      // against the final positions (UI-055). The overview is throttled the
      // same way and goes stale the same way.
      drawHulls(true);
      publishOverview(true);
    });

    // Force-mode positioning. In tree mode the simulation is stopped, so
    // this tick handler doesn't fire — `applyDisplayPlan` positions edges
    // explicitly in that case.
    simulation.on('tick', () => {
      positionLinks(
        linkSel,
        (d) => (d.source as D3Node).x!, (d) => (d.source as D3Node).y!,
        (d) => (d.target as D3Node).x!, (d) => (d.target as D3Node).y!,
      );
      linkLabelSel.attr('transform', (d) => {
        const sx = (d.source as D3Node).x!;
        const sy = (d.source as D3Node).y!;
        const tx = (d.target as D3Node).x!;
        const ty = (d.target as D3Node).y!;
        const mx = (sx + tx) / 2;
        const my = (sy + ty) / 2;
        const off = labelPerpOffset(d, sx, sy, tx, ty);
        return `translate(${mx + off.dx},${my + off.dy})`;
      });
      orderBadgeSel.attr('transform', (d) => {
        // Keep the order badge 25 % from the visual tail, which flips with
        // the edge direction for reversed labels.
        const src = d.source as D3Node;
        const tgt = d.target as D3Node;
        const [hx, hy, tx, ty] = isReversed(d)
          ? [tgt.x!, tgt.y!, src.x!, src.y!]
          : [src.x!, src.y!, tgt.x!, tgt.y!];
        return `translate(${hx * 0.75 + tx * 0.25},${hy * 0.75 + ty * 0.25})`;
      });
      nodeSel.attr('transform', (d) => `translate(${d.x},${d.y})`);
      drawHulls();
      publishOverview();
    });
  }

  /**
   * Incremental graph update: preserves surviving nodes in place, fades in
   * new nodes, fades out removed nodes, and re-joins links. The simulation
   * keeps running — just its nodes/links arrays are swapped. This is what
   * fires on a live-reload data change so the user sees relationships
   * appear/disappear without the whole graph resetting.
   */
  function updateGraph(data: import('../types/graph').GraphData) {
    canvasColors = canvasChrome();
    if (!simulation || !g || !nodeSel) return;
    // Before the join: the size domain is taken from the nodes in play, so
    // adding or removing nodes can rescale every surviving circle too.
    encoding = get(nodeEncoding);
    rebuildLabelOffsets(data);

    // --- Preserve positions from surviving nodes ---
    const oldPositions = new Map<string, { x: number; y: number; vx: number; vy: number; fx: number | null; fy: number | null }>();
    nodeSel.each((d: D3Node) => {
      oldPositions.set(d.id, {
        x: d.x ?? 0, y: d.y ?? 0,
        vx: (d as any).vx ?? 0, vy: (d as any).vy ?? 0,
        fx: d.fx ?? null, fy: d.fy ?? null,
      });
    });

    // Transfer positions to new node objects (collapseGraph creates fresh objects).
    const w = container.clientWidth;
    const h = container.clientHeight;
    const seeder = makeSeeder(data.nodes, w, h);
    for (const n of data.nodes) {
      const old = oldPositions.get(n.id);
      if (old) {
        n.x = old.x; n.y = old.y;
        (n as any).vx = old.vx; (n as any).vy = old.vy;
        n.fx = old.fx; n.fy = old.fy;
      } else if (!Number.isFinite(n.x) || !Number.isFinite(n.y)) {
        // New node: seed it in its folder's wedge rather than at a random
        // offset from the centre (UI-053). A live-reload that adds a file
        // should drop it next to its siblings, and the same reload twice
        // should put it in the same place.
        const p = seeder.position(n);
        n.x = p.x; n.y = p.y;
      }
      // Else: absent from the DOM but already carrying coordinates — a node
      // a filter removed and the user just brought back. Since UI-065 the
      // build set tracks the filters, so this is now the common case, and
      // re-seeding it would make toggling a filter shuffle the layout. The
      // node objects outlive the elements; their positions are the record.
    }

    // --- Update simulation data (keeps running, doesn't restart) ---
    simulation.nodes(data.nodes);
    const linkForce = simulation.force('link') as d3.ForceLink<D3Node, D3Link>;
    if (linkForce) linkForce.links(data.links);
    // Gentle reheat so new nodes settle without disrupting existing ones.
    simulation.alpha(0.3).restart();

    // --- Links: enter/update/exit ---
    linkSel = g.select('.links-group').selectAll<SVGLineElement, D3Link>('line')
      .data(data.links, (d) => `${sourceId(d)}->${targetId(d)}|${d.kind_raw}`)
      .join(
        (enter) => enter.append('line')
          .attr('class', 'link')
          .attr('stroke', (d) => linkStrokeColour(d))
          .attr('stroke-width', (d) => linkStrokeWidth(d.weight))
          .attr('stroke-dasharray', (d) => linkStrokeDashArray(d))
          .attr('stroke-opacity', 0)
          .attr('marker-end', 'url(#arrow)')
          .call((sel) => sel.transition().duration(400).attr('stroke-opacity', 0.6)),
        (update) => update,
        (exit) => exit.transition().duration(300).attr('stroke-opacity', 0).remove(),
      );

    // --- Link labels: simplified re-join (recreate — they're cheap) ---
    g.select('.link-labels').selectAll('*').remove();
    linkLabelSel = g.select('.link-labels').selectAll('g').data(data.links).join('g').attr('class', 'link-label-container');
    linkLabelSel.append('rect').attr('class', 'link-label-bg').attr('rx', 3).attr('ry', 3)
      .attr('fill', canvasColors.linkLabelBg).attr('fill-opacity', canvasColors.linkLabelBgOpacity);
    linkLabelSel.append('text').attr('class', 'link-label')
      .text((d) => {
        const base = d.order ? `${d.order}. ${d.kind}` : d.kind;
        const withWeight = d.weight && d.weight > 1 ? `${base} (${d.weight})` : base;
        return `${withWeight}${bindingSuffix(d)}`;
      })
      .attr('fill', (d) => LINK_COLORS[d.kind_raw] || '#888')
      .attr('font-size', '9px').attr('text-anchor', 'middle').attr('dominant-baseline', 'middle')
      .attr('pointer-events', 'none');
    linkLabelSel.each(function () {
      const text = d3.select(this).select('text');
      const bbox = (text.node() as SVGTextElement).getBBox();
      d3.select(this).select('rect')
        .attr('x', -bbox.width / 2 - 4).attr('y', -bbox.height / 2 - 2)
        .attr('width', bbox.width + 8).attr('height', bbox.height + 4);
    });

    // --- Order badges: re-join ---
    g.select('.order-badges').selectAll('*').remove();
    const orderedLinks = data.links.filter((d) => d.order != null);
    orderBadgeSel = g.select('.order-badges').selectAll('g').data(orderedLinks).join('g').attr('class', 'order-badge-container');
    orderBadgeSel.append('circle').attr('r', 8).attr('fill', '#FF9800').attr('stroke', canvasColors.orderBadgeStroke).attr('stroke-width', 1.5);
    orderBadgeSel.append('text').attr('text-anchor', 'middle').attr('dominant-baseline', 'central')
      .attr('fill', canvasColors.orderBadgeText).attr('font-size', '8px').attr('font-weight', '700').attr('pointer-events', 'none')
      .text((d) => d.order!);

    // --- Nodes: enter/update/exit ---
    const nodeG = g.select('.nodes-group');
    nodeSel = nodeG.selectAll<SVGGElement, D3Node>('g')
      .data(data.nodes, (d) => d.id)
      .join(
        (enter) => {
          const g = enter.append('g')
            .attr('class', (d) => 'node' + (d.tags?.includes('trait_impl') ? ' trait-impl' : '') + (d.tags?.includes('ghost') ? ' ghost' : ''))
            .attr('transform', (d) => `translate(${d.x ?? w / 2},${d.y ?? h / 2})`)
            .attr('opacity', 0)
            .call(d3.drag<SVGGElement, D3Node>()
              .on('start', (event, d) => { if (!event.active) simulation.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
              .on('drag', (event, d) => { d.fx = event.x; d.fy = event.y; })
              .on('end', (event, d) => { if (!event.active) simulation.alphaTarget(0); d.fx = null; d.fy = null; }))
            .on('click', (event, d) => onNodeClick(event, d))
            .on('dblclick', (event, d) => onNodeDoubleClick(event, d))
            .on('mouseover', (_, d) => { highlightHover(d); if (!$hoverLocked) hoveredNode.set(d); })
            .on('mouseout', () => { resetHighlight(); if (!$hoverLocked) hoveredNode.set(null); });

          g.append('circle')
            .attr('r', (d) => encoding.radius(d))
            .attr('fill', (d) => encoding.fill(d))
            .attr('fill-opacity', (d) => encoding.fillOpacity(d))
            .attr('stroke', canvasColors.nodeStroke).attr('stroke-width', 2);
          g.append('text').attr('class', 'kind-label')
            .attr('text-anchor', 'middle').attr('dominant-baseline', 'central')
            .attr('font-size', '8px').attr('font-weight', '700')
            .attr('fill', (d) => encoding.labelInk(d))
            .attr('pointer-events', 'none')
            .text((d) => KIND_CODES[d.kind_raw] || '??');
          g.append('text').attr('class', 'name-label')
            .attr('dy', (d) => encoding.radius(d) + 12).attr('text-anchor', 'middle')
            .attr('font-size', '11px').attr('fill', canvasColors.nameLabelFill).attr('pointer-events', 'none')
            .text((d) => d.name);

          g.transition().duration(400).attr('opacity', 1);
          return g;
        },
        (update) => update, // surviving nodes keep their DOM + position
        // Removed outright rather than faded out. `.transition().remove()`
        // only removes if the transition reaches its end, and a transition
        // is driven by requestAnimationFrame — which does not run in a
        // background tab and does not survive being interrupted. That was
        // survivable while this join only ran on scope and level changes;
        // since UI-065 the exit set is whatever the filters just excluded,
        // so a stranded exit means stale nodes sitting on the canvas that
        // the plan has already stopped accounting for. A 300 ms fade is not
        // worth a correctness hole that only appears when the tab is not
        // being looked at.
        (exit) => exit.remove(),
      );

    // Surviving nodes kept their DOM, so they still carry the radii and fills
    // of the previous encoding. Re-apply across the whole selection — the
    // rescale above is invisible otherwise.
    restyleEncoding();

    // Re-attach tick handler with fresh selections.
    attachTickHandler();
  }

  onMount(() => {
    // 0. Theme: restyle the live canvas when the theme changes. `applyTheme`
    //    swaps the :root custom properties, but the canvas bakes colours onto
    //    SVG attributes at join time, so without this the graph keeps the
    //    previous theme's chrome until the next rebuild (UI-009).
    unsubscribers.push(activeTheme.subscribe(() => restyleCanvasChrome()));

    // 0b. Encoding (UI-014). Changing what size or fill *means* is a restyle,
    //     not a re-layout — the graph is the same graph, so switching channel
    //     must not re-settle the force simulation and throw away the reading
    //     the user was in the middle of. When the encoding changed because the
    //     DATA changed, the graphData subscription below rebuilds the DOM
    //     anyway and this restyle is a cheap no-op on the way there.
    unsubscribers.push(nodeEncoding.subscribe((enc) => {
      encoding = enc;
      if (!initialized || !nodeSel) return;
      restyleEncoding();
    }));

    // 0c. Folder cohesion (UI-052). A layout change, not a restyle — but it
    //     moves the nodes already on screen rather than rebuilding them, so
    //     it reheats the simulation instead of re-joining the DOM. Both
    //     inputs are watched: the level usually arrives with new data and
    //     rebuilds anyway, but the incremental update path reuses the
    //     existing simulation, where nothing else would re-read the strength.
    unsubscribers.push(folderCohesion.subscribe(() => applyCohesion()));
    unsubscribers.push(graphLevel.subscribe(() => applyCohesion()));

    // 0c-bis. Marks are a pure overlay for the same reason the hulls below
    //     are: nothing about the plan, the layout or the filters depends on
    //     them, so a mark is a ring appearing and never a re-settle. Watched
    //     rather than applied at the click site because the toolbar's
    //     "Clear marks" and `x` on the canvas write the same store.
    unsubscribers.push(markedPaths.subscribe(() => {
      if (initialized) applyMarks();
    }));

    // 0d. Folder hulls (UI-055). Pure overlay: it reads positions the
    //     simulation already produced and draws behind everything, so it
    //     neither re-settles the layout nor touches the display plan.
    unsubscribers.push(showFolderHulls.subscribe(() => {
      if (initialized) drawHulls(true);
    }));
    // Same overlay-only redraw for the tier count (UI-070): it changes which
    // outlines exist, never which nodes are drawn or where they sit.
    unsubscribers.push(hullDepth.subscribe(() => {
      if (initialized) drawHulls(true);
    }));

    // 1. Lifecycle AND rendering, from one signal.
    //
    //    These were two subscribers, and the DOM was built from `graphData`:
    //    every node in the collapsed scope, whether or not the plan would
    //    draw it. Filtering then changed only `display`, never the build, so
    //    a large scope paid the full cost of joining and simulating
    //    thousands of nodes before anything hid them — pinning entity level
    //    on a big scope froze the tab outright (UI-065). The gate the
    //    overflow card describes never prevented any of that work; it only
    //    hid the result afterwards.
    //
    //    The build set is now the drawn set, so a filter that narrows the
    //    view narrows the work. Reading `graphData` here is safe because
    //    `displayPlan` derives from it and therefore fires after it is
    //    current. The reverse — reading the plan from a `graphData`
    //    subscriber — would see the *previous* plan, which is exactly the
    //    ordering hazard this file's header comment was written about.
    unsubscribers.push(displayPlan.subscribe((plan) => {
      const render = renderSetFor(plan, get(graphData));
      console.log(`[GraphView] plan sub: mode=${plan.mode} visible=${plan.visibleNodeIds.size} render=${render.nodes.length} overflow=${plan.overflow ? plan.overflow.drawn : 'none'} initialized=${initialized}`);
      // Only a reload can produce an *arrival*. A scope or level change
      // brings new ids too, but the user asked for those (UI-066).
      const reloadDriven = get(liveReloading) || get(refreshing);
      if (render.nodes.length > 0) {
        if (initialized && nodeSel) {
          // Check how many node IDs survived — if most are the same, this
          // is an incremental update (live reload, or a filter that kept
          // most of the view) and we can patch the existing DOM without
          // tearing everything down. If the IDs are largely different
          // (level switch, scope change), full rebuild.
          const oldIds = new Set<string>();
          nodeSel.each((d: D3Node) => oldIds.add(d.id));
          const newIds = new Set(render.nodes.map((n) => n.id));
          const surviving = [...oldIds].filter((id) => newIds.has(id)).length;
          const isIncremental = oldIds.size > 0 && surviving / oldIds.size > 0.3;

          if (reloadDriven) markArrivals(newcomers(oldIds, newIds));

          if (isIncremental) {
            console.log(`[GraphView] updateGraph (incremental: ${surviving}/${oldIds.size} survived)`);
            updateGraph(render);
          } else {
            console.log('[GraphView] teardownGraph + initGraph (structural change)');
            teardownGraph();
            initialized = true;
            initGraph(render);
          }
        } else {
          if (initialized) teardownGraph();
          initialized = true;
          console.log('[GraphView] initGraph with', render.nodes.length, 'nodes');
          initGraph(render);
        }
        // Styling still comes from the plan: dimming, selection, tree
        // pinning. What changed is that the elements it styles are only
        // ever the ones the plan asked for.
        applyDisplayPlan(plan);
      } else if (initialized) {
        console.log(`[GraphView] teardownGraph (${plan.overflow ? 'over the draw ceiling' : 'nothing to draw'})`);
        teardownGraph();
      }
    }));

    // 2b. Re-apply plan when dim opacity changes (slider interaction).
    unsubscribers.push(diffDimOpacity.subscribe(() => {
      if (initialized) {
        const plan = get(displayPlan);
        applyDisplayPlan(plan);
      }
    }));

    // 3. Cosmetic toggles — direct DOM mutations, no plan involvement.
    unsubscribers.push(showLabels.subscribe((v) => {
      if (initialized) nodeSel?.selectAll('.name-label').style('display', v ? '' : 'none');
    }));
    unsubscribers.push(showKindLabels.subscribe((v) => {
      if (initialized) nodeSel?.selectAll('.kind-label').style('display', v ? '' : 'none');
    }));
    unsubscribers.push(showLinkLabels.subscribe(() => {
      // Re-run the same predicate rather than blanket show/hide, or turning
      // labels on would resurrect the suppressed dominant kind.
      if (initialized) {
        linkLabelSel?.style('display', (d: D3Link) =>
          linkLabelDisplay(d, lastLinkVisible ? lastLinkVisible(d) : null));
      }
    }));

    // 4. Display-search (within-view highlight). Standalone because it's a
    //    view-only overlay — including it in displayPlan would couple the
    //    plan to visibleNodeIds and introduce a feedback loop.
    unsubscribers.push(displaySearchMatchIds.subscribe((ids) => {
      if (initialized && nodeSel) {
        nodeSel.classed('search-display-match', (d: D3Node) => ids.has(d.id));
      }
    }));

    // Seed viewport width immediately so the first tree layout wraps correctly.
    viewportWidth.set(container.clientWidth);

    const onResize = () => {
      const w = container.clientWidth;
      const h = container.clientHeight;
      viewportWidth.set(w);
      if (!initialized) return;
      svg.attr('width', w).attr('height', h);
      // The transform is unchanged but the rectangle it maps into is not, so
      // the box's size is stale until this runs — visibly, since collapsing a
      // side panel is exactly when the canvas gets wider.
      publishViewport(d3.zoomTransform(svgEl));
      simulation?.force('center', d3.forceCenter(w / 2, h / 2));
      if (currentMode === 'force') simulation?.alpha(0.3).restart();
    };
    window.addEventListener('resize', onResize);
    unsubscribers.push(() => window.removeEventListener('resize', onResize));
  });

  onDestroy(() => {
    unsubscribers.forEach((u) => u());
    simulation?.stop();
    if (arrivalTimer) clearTimeout(arrivalTimer);
  });
</script>

<div class="graph-container" class:pane-focused={$focusedPane === 'graph'}
  data-pane="graph" bind:this={container}>
  <svg bind:this={svgEl}></svg>
  <slot />
</div>

<style>
  .graph-container {
    flex: 1;
    position: relative;
    overflow: hidden;
    min-width: 0;
    height: 100%;
  }

  /* The canvas is the default pane, so its ring is the quietest of the five:
     inset, one pixel, and drawn over the SVG rather than around it. */
  .graph-container.pane-focused::after {
    content: '';
    position: absolute;
    inset: 0;
    pointer-events: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  :global(.graph-container svg) {
    width: 100%;
    height: 100%;
    background: var(--bg-body);
  }

  /* Folder hulls (UI-055). `pointer-events: none` on the whole group is
     load-bearing, not tidiness: the background-click handler deselects only
     when `event.target === svgEl`, and a hull covers most of the canvas, so
     a hittable outline would silently break click-to-deselect everywhere it
     reaches. It also costs the outline a <title> tooltip — the label carries
     the name instead. */
  :global(.folder-hull) { pointer-events: none; }
  :global(.hull-label) {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.12em;
  }
  /* A parent region's name is the heading over the regions inside it
     (UI-070). Wider tracking as well as more size: two nested outlines cross,
     and the reader has to tell at a glance which name belongs to which. It
     stays clear of the 11px entity labels in the other direction, so a region
     name still cannot be mistaken for a node's. */
  :global(.folder-hull.parent .hull-label) {
    font-size: 13px;
    letter-spacing: 0.2em;
  }

  :global(.node) { cursor: pointer; }
  :global(.node circle) { stroke: var(--text); stroke-width: 2px; }
  :global(.node.trait-impl circle) { stroke: #4CAF50; stroke-width: 3px; stroke-dasharray: 4,2; }
  :global(.node.ghost circle) { stroke-dasharray: 4,2; opacity: 0.5; }
  :global(.node.ghost .name-label) { opacity: 0.6; }
  :global(.node.ghost .kind-label) { opacity: 0.6; }
  :global(.node.selected circle) { stroke: var(--accent); stroke-width: 4px; }
  :global(.node.dimmed) { opacity: 0.2; }
  :global(.node.search-match circle) { stroke: #FFD54F; stroke-width: 4px; filter: drop-shadow(0 0 4px rgba(255, 213, 79, 0.8)); }
  :global(.node.search-neighbor) { opacity: 0.55; }
  :global(.node.search-neighbor circle) { stroke-width: 1.5px; }
  /* Display-search: cyan, distinct from gold so a node can be both a
     dataset match AND a display-search hit without visual collision.
     Display-search wins on stroke (applied after) but both glows stack. */
  :global(.node.search-display-match circle) { stroke: #4DD0E1; stroke-width: 4px; filter: drop-shadow(0 0 5px rgba(77, 208, 225, 0.9)); }
  :global(.node.search-match.search-display-match circle) { stroke: #4DD0E1; filter: drop-shadow(0 0 4px rgba(255, 213, 79, 0.6)) drop-shadow(0 0 5px rgba(77, 208, 225, 0.9)); }
  :global(.node.search-display-match) { opacity: 1 !important; }
  /* Arrival mark (UI-066): a node that appeared on a live reload wears this
     ring for 30s. Magenta because every other stroke on the canvas is
     already spoken for — gold and cyan are the two searches, green/red/amber
     are the diff statuses, and `var(--accent)` is the selection. A separate
     element rather than the node's own stroke, so a new node that is *also*
     selected, searched or diff-coloured keeps saying all of those things.

     Written as `circle.arrival-ring` and placed last so it outranks the
     `.node circle` and `.node.selected circle` rules above, which would
     otherwise repaint it. */
  :global(.node circle.arrival-ring) {
    fill: none;
    stroke: #E040FB;
    stroke-width: 3px;
    stroke-dasharray: none;
    filter: drop-shadow(0 0 4px rgba(224, 64, 251, 0.7));
    animation: arrival-pulse 1.8s ease-in-out infinite;
  }

  @keyframes arrival-pulse {
    0%, 100% { stroke-opacity: 0.95; }
    50%      { stroke-opacity: 0.3; }
  }

  /* The pulse is the part that catches the eye across a busy canvas, so
     without it the ring has to hold still and stay legible on its own. */
  @media (prefers-reduced-motion: reduce) {
    :global(.node circle.arrival-ring) {
      animation: none;
      stroke-opacity: 0.95;
    }
  }

  /* The marked ring. `var(--accent)` is the selection's colour on purpose,
     where the arrival ring went out of its way to avoid every other stroke:
     a mark *is* a selection, and borrowing the hue is what says the two
     belong to the same family rather than inventing a sixth meaning for a
     sixth colour. Dashed and thinner is what separates them — the solid
     4px stroke is the one subject, the dashes are the set. Placed after the
     `.node.selected circle` rule for the same specificity reason as above. */
  :global(.node circle.mark-ring) {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2px;
    stroke-dasharray: 3, 3;
  }

  :global(.link.dimmed) { opacity: 0.1; }
  :global(.link-label-container.dimmed) { opacity: 0.1; }
  :global(.order-badge-container.dimmed) { opacity: 0.1; }
</style>
