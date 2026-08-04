<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import * as d3 from 'd3';
  import type { D3Node, D3Link } from '../types/graph';
  import { LINK_COLORS, KIND_CODES } from '../types/graph';
  import {
    graphData, selectedNode, hoveredNode, hoverLocked, hoverDepth, viewMode,
    showLabels, showKindLabels, showLinkLabels, viewportWidth,
  } from '../stores/graph';
  import { displayPlan, displaySearchMatchIds, linkKeyFor, type DisplayPlan } from '../viewmodels/displayPlan';
  import { diffActive, diffStatusMap, diffSourceChangedMap, diffDimOpacity, DIFF_COLORS, normalizeEntityId } from '../stores/diff';
  import { autoFitView, activeTheme } from '../stores/settings';
  import { canvasChrome, type CanvasChrome } from '../utils/canvasChrome';
  import { drillIn } from '../stores/scope';
  import { isMoreChildThan } from '../utils/kindPriority';
  import { nodeEncoding } from '../stores/encoding';
  import type { NodeEncoding } from '../viewmodels/nodeEncoding';

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
    simulation?.force('collision', d3.forceCollide().radius(collisionRadius()));
    simulation?.alpha(0.2).restart();
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
      linkSel
        .attr('x1', (d) => (isReversed(d) ? (d.target as D3Node) : (d.source as D3Node)).x ?? 0)
        .attr('y1', (d) => (isReversed(d) ? (d.target as D3Node) : (d.source as D3Node)).y ?? 0)
        .attr('x2', (d) => (isReversed(d) ? (d.source as D3Node) : (d.target as D3Node)).x ?? 0)
        .attr('y2', (d) => (isReversed(d) ? (d.source as D3Node) : (d.target as D3Node)).y ?? 0);
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
        // Endpoints `1` = tail, `2` = arrow-head. Flip for reversed edges so
        // "inherited by"/"called by" labels read with the arrow, not against.
        linkSel
          .attr('x1', (d) => (isReversed(d) ? tx(d) : sx(d)))
          .attr('y1', (d) => (isReversed(d) ? ty(d) : sy(d)))
          .attr('x2', (d) => (isReversed(d) ? sx(d) : tx(d)))
          .attr('y2', (d) => (isReversed(d) ? sy(d) : ty(d)));
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

  function resetHighlight() {
    nodeSel.classed('dimmed', false);
    linkSel.classed('dimmed', false);
    linkLabelSel.classed('dimmed', false);
    orderBadgeSel.classed('dimmed', false);
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

  function teardownGraph() {
    if (autoFitTimer) { clearTimeout(autoFitTimer); autoFitTimer = null; }
    simulation?.stop();
    simulation = null as any;
    if (svg) {
      svg.on('.zoom', null);
      svg.on('click', null);
      svg.selectAll('*').remove();
    }
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
    data.nodes.forEach((n) => {
      n.x = undefined; n.y = undefined;
      (n as any).vx = 0; (n as any).vy = 0;
      n.fx = null; n.fy = null;
    });

    svg = d3.select(svgEl).attr('width', w).attr('height', h);
    g = svg.append('g');
    fileHullGroup = g.append('g').attr('class', 'file-hulls');

    zoom = d3.zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.1, 4])
      .on('zoom', (event) => g.attr('transform', event.transform));
    svg.call(zoom);
    svg.call(zoom.transform, d3.zoomIdentity);

    svg.on('click', (event) => {
      if (event.target === svgEl) selectedNode.set(null);
    });

    svg.append('defs').selectAll('marker').data(['arrow']).join('marker')
      .attr('id', 'arrow').attr('viewBox', '0 -5 10 10').attr('refX', 25)
      .attr('refY', 0).attr('markerWidth', 6).attr('markerHeight', 6).attr('orient', 'auto')
      .append('path').attr('fill', canvasColors.arrowFill).attr('d', 'M0,-5L10,0L0,5');

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
      .force('collision', d3.forceCollide().radius(collisionRadius()));

    linkSel = g.append('g').attr('class', 'links-group').selectAll('line').data(data.links).join('line')
      .attr('class', 'link')
      .attr('stroke', (d) => linkStrokeColour(d))
      .attr('stroke-width', (d) => {
        const wt = d.weight ?? 1;
        if (wt <= 1) return 1.5;
        return Math.min(1.5 + Math.log2(wt) * 0.9, 6);
      })
      .attr('stroke-dasharray', (d) => linkStrokeDashArray(d))
      .attr('stroke-opacity', 0.6)
      .attr('marker-end', 'url(#arrow)');
    linkSel.append('title').text((d) => {
      if (!d.breakdown || d.weight == null) return d.kind;
      const parts = Object.entries(d.breakdown)
        .sort((a, b) => b[1] - a[1])
        .map(([k, n]) => `${n} × ${k}`);
      return `${d.kind} — ${d.weight} underlying edges (${parts.join(', ')})`;
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
      .on('click', (_, d) => selectedNode.set(d))
      .on('dblclick', (event, d) => { event.stopPropagation(); drillInto(d); })
      .on('mouseover', (_, d) => {
        highlightConnections(d);
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

    attachTickHandler();
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
    });

    // Force-mode positioning. In tree mode the simulation is stopped, so
    // this tick handler doesn't fire — `applyDisplayPlan` positions edges
    // explicitly in that case.
    simulation.on('tick', () => {
      // x1/y1 = tail, x2/y2 = arrow-head. Flip when the selection is the
      // target so the passive-voice label ("inherited by"/"called by"/…)
      // and the arrow agree on direction.
      linkSel
        .attr('x1', (d) => isReversed(d) ? (d.target as D3Node).x! : (d.source as D3Node).x!)
        .attr('y1', (d) => isReversed(d) ? (d.target as D3Node).y! : (d.source as D3Node).y!)
        .attr('x2', (d) => isReversed(d) ? (d.source as D3Node).x! : (d.target as D3Node).x!)
        .attr('y2', (d) => isReversed(d) ? (d.source as D3Node).y! : (d.target as D3Node).y!);
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
    for (const n of data.nodes) {
      const old = oldPositions.get(n.id);
      if (old) {
        n.x = old.x; n.y = old.y;
        (n as any).vx = old.vx; (n as any).vy = old.vy;
        n.fx = old.fx; n.fy = old.fy;
      } else {
        // New node: place near center so it animates from a sensible origin.
        n.x = w / 2 + (Math.random() - 0.5) * 100;
        n.y = h / 2 + (Math.random() - 0.5) * 100;
      }
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
          .attr('stroke-width', (d) => {
            const wt = d.weight ?? 1;
            return wt <= 1 ? 1.5 : Math.min(1.5 + Math.log2(wt) * 0.9, 6);
          })
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
            .on('click', (_, d) => selectedNode.set(d))
            .on('dblclick', (event, d) => { event.stopPropagation(); drillInto(d); })
            .on('mouseover', (_, d) => { highlightConnections(d); if (!$hoverLocked) hoveredNode.set(d); })
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
        (exit) => exit.transition().duration(300).attr('opacity', 0).remove(),
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

    // 1. Lifecycle: rebuild the DOM whenever the underlying graph changes.
    //    The DOM is created with EVERY node/edge in `data`; visibility is
    //    decided downstream by `applyDisplayPlan`. This separation matters:
    //    D3's data join only needs to run once per dataset, not per filter
    //    change.
    unsubscribers.push(graphData.subscribe((data) => {
      console.log(`[GraphView] graphData sub: nodes=${data.nodes.length} initialized=${initialized}`);
      if (data.nodes.length > 0) {
        if (initialized && nodeSel) {
          // Check how many node IDs survived — if most are the same, this
          // is an incremental update (live reload) and we can patch the
          // existing DOM without tearing everything down. If the IDs are
          // largely different (level switch, scope change), full rebuild.
          const oldIds = new Set<string>();
          nodeSel.each((d: D3Node) => oldIds.add(d.id));
          const newIds = new Set(data.nodes.map((n) => n.id));
          const surviving = [...oldIds].filter((id) => newIds.has(id)).length;
          const isIncremental = oldIds.size > 0 && surviving / oldIds.size > 0.3;

          if (isIncremental) {
            console.log(`[GraphView] updateGraph (incremental: ${surviving}/${oldIds.size} survived)`);
            updateGraph(data);
          } else {
            console.log('[GraphView] teardownGraph + initGraph (structural change)');
            teardownGraph();
            initialized = true;
            initGraph(data);
          }
        } else {
          if (initialized) teardownGraph();
          initialized = true;
          console.log('[GraphView] initGraph with', data.nodes.length, 'nodes');
          initGraph(data);
        }
      } else if (initialized) {
        console.log('[GraphView] teardownGraph (empty data)');
        teardownGraph();
      }
    }));

    // 2. Rendering: every change that affects what's drawn (filters, level
    //    overrides, view mode, selection, search) flows into `displayPlan`
    //    and surfaces here as a single notification. No more multi-store
    //    coordination inside the View.
    unsubscribers.push(displayPlan.subscribe((plan) => {
      console.log(`[GraphView] displayPlan sub: initialized=${initialized} mode=${plan.mode} visibleNodes=${plan.visibleNodeIds.size}`);
      if (initialized) {
        applyDisplayPlan(plan);
        // DOM audit: count how many node elements are actually visible
        // after applying the plan. If this is 0 but visibleNodes > 0,
        // there's an ID mismatch between the plan and the DOM.
        if (nodeSel && plan.visibleNodeIds.size > 0) {
          let shown = 0, hidden = 0;
          nodeSel.each(function (d: D3Node) {
            if ((this as SVGGElement).style.display === 'none') hidden++;
            else shown++;
          });
          console.log(`[GraphView] DOM audit: ${shown} shown, ${hidden} hidden out of ${shown + hidden} DOM nodes`);
          if (shown === 0 && plan.visibleNodeIds.size > 0) {
            // Log sample IDs to diagnose the mismatch
            const planIds = [...plan.visibleNodeIds].slice(0, 3);
            const domIds: string[] = [];
            nodeSel.each((d: D3Node) => { if (domIds.length < 3) domIds.push(d.id); });
            console.warn(`[GraphView] ID MISMATCH — plan wants: ${planIds.join(', ')} | DOM has: ${domIds.join(', ')}`);
          }
        }
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
      simulation?.force('center', d3.forceCenter(w / 2, h / 2));
      if (currentMode === 'force') simulation?.alpha(0.3).restart();
    };
    window.addEventListener('resize', onResize);
    unsubscribers.push(() => window.removeEventListener('resize', onResize));
  });

  onDestroy(() => {
    unsubscribers.forEach((u) => u());
    simulation?.stop();
  });
</script>

<div class="graph-container" bind:this={container}>
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

  :global(.graph-container svg) {
    width: 100%;
    height: 100%;
    background: var(--bg-body);
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
  :global(.link.dimmed) { opacity: 0.1; }
  :global(.link-label-container.dimmed) { opacity: 0.1; }
  :global(.order-badge-container.dimmed) { opacity: 0.1; }
</style>
