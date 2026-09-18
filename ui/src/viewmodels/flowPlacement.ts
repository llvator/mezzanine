/**
 * UI-146 — the Flow view's layout.
 *
 * `flowLayers` says which layer each node is in. This turns that into
 * coordinates: one column per layer, upstream on the left, and within a column
 * an order chosen to keep the lines between columns from crossing each other
 * more than they have to.
 *
 * ## Left is what everything else stands on
 *
 * Column 0 depends on nothing else drawn; each column to the right stands on
 * the ones to its left. So a change propagates rightwards — the flux — while
 * every arrow head points left, at the dependency it names. The canvas draws
 * the axis caption for exactly this reason: a layered picture whose arrows all
 * run backwards is misread by default, and the fix is a label, not a
 * reversal. Flipping the arrows would make the *picture* readable by making
 * every arrow lie about what an edge means.
 *
 * ## Ordering within a column
 *
 * One barycentre pass, left to right: a node sits at the average height of the
 * dependencies it points at, all of which are in columns already placed —
 * guaranteed, because a cross-layer edge always runs from a higher layer to a
 * lower one, and the ones that do not are inside a cycle and share a column.
 *
 * One pass and not the usual sweep-until-stable. This is a hover-speed
 * recompute on a canvas already capped at the render budget, and the second
 * pass buys a crossing count nobody is measuring. Ties break on the id, so
 * the same graph lays out the same way twice — the determinism the rest of
 * this codebase is built on.
 *
 * Pure and store-free (`npm run test:flowplace`).
 */

import { flowLayers, type FlowEdge, type FlowReading } from './flowLayers.ts';

/** How far apart the columns and rows sit, by the density control the tree
 *  view already exposes. Columns are wider than rows are tall because a node's
 *  label runs horizontally — at `compact` the labels of adjacent columns
 *  overlap otherwise, which is the one way this layout becomes unreadable. */
const SPACING = {
  compact: { column: 220, row: 34 },
  normal: { column: 300, row: 46 },
  spacious: { column: 400, row: 62 },
} as const;

export type FlowDensity = keyof typeof SPACING;

/** One column, so the canvas can caption it. */
export interface FlowRung {
  layer: number;
  /** Column centre, in the same origin-centred space as the positions. */
  x: number;
  /** Nodes in this column. */
  count: number;
}

export interface FlowPlacement {
  positions: Map<string, { x: number; y: number }>;
  axis: FlowRung[];
  reading: FlowReading;
}

export function emptyPlacement(): FlowPlacement {
  return {
    positions: new Map(),
    axis: [],
    reading: { standing: new Map(), layers: [], cycles: [] },
  };
}

/** The four plan fields the Flow view owns. Named so `displayPlan.compute` can
 *  spread one call instead of carrying four conditionals — that function is
 *  already the largest in the codebase, and a fourth mode should cost it as
 *  close to nothing as a fourth mode can. */
export interface FlowFields {
  mode: 'force' | 'flow';
  treePositions: Map<string, { x: number; y: number }>;
  flowAxis: FlowRung[];
  flowCycleIds: Set<string>;
}

/**
 * Force mode's layout answer, or Flow's. `on` is the one decision.
 *
 * `edgesOf` is a thunk, and `nodes` arrives as the Set the caller already
 * holds, so force mode pays nothing at all for a mode it is not in — not a
 * spread, not a pass over the link list. That is what lets `compute` call this
 * unconditionally, which is the whole point: an `if` there would put the cost
 * back on the largest function in the codebase.
 */
export function flowFields(
  on: boolean,
  nodes: ReadonlySet<string>,
  edgesOf: () => readonly FlowEdge[],
  density: FlowDensity,
): FlowFields {
  if (!on) {
    return { mode: 'force', treePositions: new Map(), flowAxis: [], flowCycleIds: new Set() };
  }
  const placed = flowPlacement([...nodes], edgesOf(), density);
  return {
    mode: 'flow',
    treePositions: placed.positions,
    flowAxis: placed.axis,
    flowCycleIds: new Set(placed.reading.cycles.flat()),
  };
}

export function flowPlacement(
  nodes: readonly string[],
  edges: readonly FlowEdge[],
  density: FlowDensity = 'normal',
): FlowPlacement {
  const reading = flowLayers(nodes, edges);
  if (reading.layers.length === 0) return emptyPlacement();
  return { ...place(reading.layers, targetIndex(edges), SPACING[density]), reading };
}

type Spacing = (typeof SPACING)[FlowDensity];

/** What each node depends on, by id. */
function targetIndex(edges: readonly FlowEdge[]): Map<string, string[]> {
  const targetsOf = new Map<string, string[]>();
  for (const e of edges) {
    const list = targetsOf.get(e.source);
    if (list) list.push(e.target);
    else targetsOf.set(e.source, [e.target]);
  }
  return targetsOf;
}

/**
 * A column's order: each node at the average height of the dependencies it
 * points at, all of which are in columns already placed.
 *
 * A node with nothing placed below it has no barycentre to honour, and sorts
 * to the middle rather than to an end. Parking every such node at the top
 * would stack a column's cycle members and its isolated nodes into one corner,
 * which reads as a cluster the graph does not have.
 */
function orderColumn(
  column: readonly string[],
  targetsOf: Map<string, string[]>,
  heightOf: Map<string, number>,
): string[] {
  const scored = column.map((id) => ({ id, bary: barycentre(id, targetsOf, heightOf) }));
  scored.sort((a, b) => a.bary - b.bary || a.id.localeCompare(b.id));
  return scored.map((s) => s.id);
}

function barycentre(
  id: string,
  targetsOf: Map<string, string[]>,
  heightOf: Map<string, number>,
): number {
  let sum = 0;
  let seen = 0;
  for (const t of targetsOf.get(id) ?? []) {
    const h = heightOf.get(t);
    if (h === undefined) continue;
    sum += h;
    seen++;
  }
  return seen === 0 ? 0 : sum / seen;
}

/** One column per layer, left to right, each ordered against the ones already
 *  placed to its left. */
function place(
  layers: readonly string[][],
  targetsOf: Map<string, string[]>,
  gap: Spacing,
): { positions: Map<string, { x: number; y: number }>; axis: FlowRung[] } {
  const positions = new Map<string, { x: number; y: number }>();
  const axis: FlowRung[] = [];
  // Where each dependency sits in its own column, as an offset from that
  // column's centre. Comparable across columns of different sizes, which a raw
  // row index would not be — a column of two and a column of forty would
  // otherwise pull their dependants to wildly different heights.
  const heightOf = new Map<string, number>();
  const xOrigin = ((layers.length - 1) * gap.column) / 2;
  for (let layer = 0; layer < layers.length; layer++) {
    const x = layer * gap.column - xOrigin;
    const yOrigin = ((layers[layer].length - 1) * gap.row) / 2;
    orderColumn(layers[layer], targetsOf, heightOf).forEach((id, row) => {
      positions.set(id, { x, y: row * gap.row - yOrigin });
      heightOf.set(id, row * gap.row - yOrigin);
    });
    axis.push({ layer, x, count: layers[layer].length });
  }
  return { positions, axis };
}
