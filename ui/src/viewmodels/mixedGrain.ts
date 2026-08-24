/**
 * Which grain each node is drawn at, as a function of how far it is from what
 * the reader is looking at (UI-104).
 *
 * The level control spends the render budget uniformly: 400 entities or
 * nothing. That is the wrong shape for the question a graph is best at
 * answering — *where does this change sit in the system* — because the answer
 * requires reaching outward and reaching outward is exactly what blows the
 * budget. A ring plan spends the budget where the reader is looking: the
 * focus and its immediate neighbours stay entities, and everything further
 * out folds into the file or the directory that holds it.
 *
 * This module decides GRAIN and nothing else. Like `diffLevels`, it takes ids
 * and paths rather than a store or a canvas, so the rings can be tested
 * without a graph, a simulation or a browser. `collapseGraph` is what turns
 * the answer into nodes.
 *
 * Two properties the rest of the feature leans on:
 *
 *   - **A scope is atomic.** Rings are drawn over a graph, but a picture is
 *     drawn over a tree, and the two disagree: a file can easily hold one
 *     entity the focus calls and another nothing reaches. Drawing the first
 *     as a circle while the second is folded into a directory rollup would
 *     put an entity on screen *and* a Module node that contains it — the same
 *     code twice, once inside the other. `promote` is the rule that stops it.
 *
 *   - **Distance is measured on the raw entity graph**, before any collapse.
 *     It has to be: the grain is what decides the collapse, so asking a
 *     collapsed graph how far anything is would be circular.
 */

import type { D3Link, D3Node, GraphLevel } from '../types/graph';
import { moduleOf } from './collapseGraph.ts';

/** Coarseness, ascending. `entity` is the finest thing there is. */
const RANK: Record<GraphLevel, number> = { entity: 0, file: 1, module: 2 };

/** The finer of two grains. */
function finer(a: GraphLevel, b: GraphLevel): GraphLevel {
  return RANK[a] <= RANK[b] ? a : b;
}

/**
 * Grain per hop from the focus. Index is the hop count, so `rings[0]` is the
 * focus itself.
 *
 * The LAST entry is the rest of the graph: it covers every ring beyond it and
 * every node the walk never reached at all. That is deliberate rather than a
 * fallback — "the rest" is a real part of the picture and the reader asked to
 * see it, so it has to be *drawn* at some grain rather than dropped. It is
 * also why the array is never empty.
 */
export type RingGrains = readonly [GraphLevel, ...GraphLevel[]];

/**
 * The default shape: the focus and its immediate callers/callees stay
 * readable, the next hop is one circle per file, and the rest of the repo is
 * one circle per directory.
 *
 * Two hops of entities is the widest that reliably fits `RENDER_BUDGET` on
 * this repo; three is a hairball again on any node with real fan-in, which is
 * the case the reader most wants to look at.
 */
export const DEFAULT_RINGS: RingGrains = ['entity', 'entity', 'file', 'module'];

/**
 * The rings a reach and an outer grain describe: `reach` hops of entities
 * around the focus, then everything else.
 *
 * Two controls, and the level buttons keep their meaning rather than gaining
 * a fourth entry — with a focus set, Module means *module out there*, and the
 * reach says how far "here" extends. Deliberately no graded file ring in
 * between: it would be a ring the reader never asked for and cannot see in
 * either control, and the promise that the level button says what the rest of
 * the graph is drawn as is worth more than the gentler gradient.
 */
export function ringsFor(reach: number, outer: GraphLevel): RingGrains {
  const hops = Math.max(0, Math.floor(reach));
  return [...Array(hops + 1).fill('entity'), outer] as unknown as RingGrains;
}

/**
 * Every entity under `path` — the seed a focused *scope* resolves to.
 *
 * The focus is a path and never an id, for the reason `f.visual_scopes`'
 * marks are: a ring plan's whole job is to survive the level change it
 * causes, and `collapseGraph` mints fresh ids on every one of those. It also
 * means a reader can focus a File or Module rollup and have the rings open it
 * — where seeding from the selected node would delete the node that seeded
 * the plan the moment the plan drew.
 *
 * Matching is on a segment boundary, so focusing `ui/src` does not take
 * `ui/srcgen` with it.
 */
export function seedFromPath(nodes: readonly D3Node[], path: string): Set<string> {
  const seed = new Set<string>();
  const prefix = path.endsWith('/') ? path : `${path}/`;
  for (const n of nodes) {
    if (!n.file_path) continue;
    if (n.file_path === path || n.file_path.startsWith(prefix)) seed.add(n.id);
  }
  return seed;
}

/** The id at one end of a link, whether or not d3 has already replaced it
 *  with the node object it points at. */
function endId(end: D3Link['source'] | D3Link['target']): string {
  return typeof end === 'object' ? (end as D3Node).id : (end as string);
}

/**
 * Hops from `seed` to every node it can reach, capped at `maxHops`.
 *
 * Undirected on purpose. A ring is "how near is this to what I am reading",
 * and a function that calls the focus is exactly as near as one the focus
 * calls — direction is a question the reader asks with the direction filters,
 * and answering it here as well would make the two disagree.
 *
 * Exported for the tests, which are much clearer about the walk than about
 * the grains derived from it.
 */
export function hopDistances(
  links: readonly D3Link[],
  seed: ReadonlySet<string>,
  maxHops: number,
): Map<string, number> {
  const dist = new Map<string, number>();
  for (const id of seed) dist.set(id, 0);
  if (maxHops <= 0 || seed.size === 0) return dist;

  // Built once. Walking `links` inside the frontier loop is the quadratic
  // shape that makes this unaffordable on a real repo's edge count.
  const adj = new Map<string, string[]>();
  const join = (a: string, b: string) => {
    const at = adj.get(a);
    if (at) at.push(b);
    else adj.set(a, [b]);
  };
  for (const l of links) {
    const s = endId(l.source);
    const t = endId(l.target);
    if (s === t) continue;
    join(s, t);
    join(t, s);
  }

  let frontier = Array.from(seed);
  for (let d = 1; d <= maxHops && frontier.length > 0; d++) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const nb of adj.get(id) ?? []) {
        if (dist.has(nb)) continue;
        dist.set(nb, d);
        next.push(nb);
      }
    }
    frontier = next;
  }
  return dist;
}

export interface RingPlan {
  /** The grain each node id is drawn at. Total over the nodes passed in. */
  grainById: Map<string, GraphLevel>;
  /** How many nodes the picture will hold once collapsed — distinct scopes,
   *  counting a ghost at entity grain as one. The caller needs this to say
   *  what a ring costs BEFORE drawing it, which is the promise
   *  `f.aggregation`'s expansion makes and this has to keep. */
  drawnCount: number;
}

/**
 * Assign a grain to every node, from its distance to the focus.
 *
 * `seed` is entity ids, never nodes: when the reader has a File or Module
 * rollup selected the caller resolves it to the entities inside first, and
 * that resolution needs the scope rules this module deliberately does not
 * import.
 */
export function planRingGrain(
  nodes: readonly D3Node[],
  links: readonly D3Link[],
  seed: ReadonlySet<string>,
  rings: RingGrains,
): RingPlan {
  const asked = askedGrain(nodes, hopDistances(links, seed, rings.length - 1), rings);
  const { fileGrain, modGrain } = promoteScopes(nodes, asked);

  const grainById = new Map<string, GraphLevel>();
  const scopes = new Set<string>();
  for (const n of nodes) {
    const g = n.file_path ? clampToScope(n, fileGrain, modGrain) : asked.get(n.id)!;
    grainById.set(n.id, g);
    // A ghost is in no scope at any grain — the same exemption `f.grouping`
    // holds. It is one circle when the picture is drawing entities at all, and
    // `collapseGraph` drops it otherwise, so it counts only in that case.
    if (n.file_path) scopes.add(g === 'entity' ? n.id : g === 'file' ? n.file_path : moduleOf(n));
    else if (g === 'entity') scopes.add(n.id);
  }

  return { grainById, drawnCount: scopes.size };
}

/** What each node asks for on its own, before any scope has a say. */
function askedGrain(
  nodes: readonly D3Node[],
  dist: ReadonlyMap<string, number>,
  rings: RingGrains,
): Map<string, GraphLevel> {
  const last = rings.length - 1;
  const asked = new Map<string, GraphLevel>();
  for (const n of nodes) {
    const d = dist.get(n.id);
    asked.set(n.id, d === undefined ? rings[last] : rings[Math.min(d, last)]);
  }
  return asked;
}

/**
 * The finest grain any member of each scope asked for.
 *
 * This is the atomicity rule: a file with one entity in reach is an open file,
 * not a half-open one, and a module holding an open file is an open module.
 */
function promoteScopes(
  nodes: readonly D3Node[],
  asked: ReadonlyMap<string, GraphLevel>,
): { fileGrain: Map<string, GraphLevel>; modGrain: Map<string, GraphLevel> } {
  const fileGrain = new Map<string, GraphLevel>();
  const modGrain = new Map<string, GraphLevel>();
  for (const n of nodes) {
    if (!n.file_path) continue;
    const f = fileGrain.get(n.file_path);
    const g = asked.get(n.id)!;
    fileGrain.set(n.file_path, f === undefined ? g : finer(f, g));
  }
  for (const n of nodes) {
    if (!n.file_path) continue;
    const mod = moduleOf(n);
    const m = modGrain.get(mod);
    const g = fileGrain.get(n.file_path)!;
    modGrain.set(mod, m === undefined ? g : finer(m, g));
  }
  return { fileGrain, modGrain };
}

/**
 * The grain a node actually draws at, once its scopes have had their say.
 *
 * A module nobody reached stays one circle however its files were labelled; a
 * module with anything in reach is opened, and then each file inside draws at
 * its own grain — which for a file out of reach is one File circle, never a
 * second Module overlapping the first.
 */
function clampToScope(
  node: D3Node,
  fileGrain: ReadonlyMap<string, GraphLevel>,
  modGrain: ReadonlyMap<string, GraphLevel>,
): GraphLevel {
  if (modGrain.get(moduleOf(node)) === 'module') return 'module';
  return fileGrain.get(node.file_path) === 'entity' ? 'entity' : 'file';
}

/**
 * The plan as the resolver `collapseGraph` takes.
 *
 * A node the plan never saw falls to the outermost ring rather than to
 * Entity: an unknown node appearing at full detail is how a budget gets blown
 * by something nobody chose to look at.
 */
export function grainFromPlan(plan: RingPlan, rings: RingGrains) {
  const outer = rings[rings.length - 1];
  return (node: D3Node): GraphLevel => plan.grainById.get(node.id) ?? outer;
}
