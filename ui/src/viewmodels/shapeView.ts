/**
 * One folder's own graph, turned into a drawing (UI-108).
 *
 * The canvas has two ways of showing code and neither draws the thing
 * `FolderShape` scores. The force graph shows every file in scope at once,
 * which past a few hundred nodes is the hairball this whole measure exists
 * to describe. Focusing a file gives a readable picture but flattens every
 * relationship to the same weight, so *which* edge is the violation — the
 * one question a shape verdict raises — is exactly what it cannot say.
 *
 * This module answers three questions about a `FolderPicture`, and nothing
 * else:
 *
 *   - **Which nodes are on screen**, as a resolver `collapseGraph` can take:
 *     the folder's immediate children, each subfolder as one circle, plus
 *     the files one hop outside that touch it. Everything else is dropped.
 *   - **Where each one sits**: inside children in rows by level, top to
 *     bottom, with outsiders in gutters either side. The layout IS the
 *     claim — a shape you can read has a reading order, and a force
 *     simulation deliberately has none.
 *   - **How each edge reads**: step / skip / back / entry / breach / exit,
 *     off the picture rather than re-derived, so the drawing and the number
 *     beside it cannot disagree.
 *
 * Pure — a picture in, plain data out, no stores and no DOM — for the reason
 * `noteScope` and `diffLevels` are: the interesting failure is a drawing
 * that *lies* (marks a clean edge as a violation, or puts a breach where
 * there is none), and that is assertable without a browser.
 *
 * **Known limit: the picture is repo-wide, the canvas is not.** The engine
 * computes doors and boundary traffic over every file it analysed, because
 * "how many ways in does this folder have" is a fact about everything
 * outside it. The canvas can only draw what the reader's analysis scope
 * loaded. Narrow the scope to the folder itself and the verdict still
 * counts outsiders the drawing cannot show — the folder's own children and
 * every edge between them stay exact, but the gutters thin out. Worth
 * knowing before reading a sparse boundary as a clean one; the fix, if it
 * proves to matter, is for the view to widen the scope rather than for this
 * module to quietly recount.
 *
 *   npm run test:shape-view
 */

import type {
  D3Node,
  FolderPicture,
  GraphLevel,
  OutsideEdge,
  PictureChild,
} from '../types/graph';
import type { GrainOf, ScopeOf } from './collapseGraph';

/** Every reading an edge can carry in this view. The first three are
 *  internal to the folder, the last three cross its boundary. */
export type ShapeEdgeVerdict = 'step' | 'skip' | 'back' | 'entry' | 'breach' | 'exit';

/** How bad each reading is, worst first. Two picture edges can collapse
 *  onto one drawn line — several files inside a subfolder breached by the
 *  same outsider — and the drawn line has to carry the worst of them, or a
 *  breach hides behind an entry that happens to share its endpoints. */
const SEVERITY: Record<ShapeEdgeVerdict, number> = {
  back: 0,
  skip: 1,
  breach: 2,
  entry: 3,
  exit: 4,
  step: 5,
};

/** Whether an edge is a defect. The exits are the interesting case: a
 *  folder depending outward is what a folder is for, and colouring those as
 *  problems would mark almost every line on screen. */
export function isViolation(verdict: ShapeEdgeVerdict): boolean {
  return verdict === 'back' || verdict === 'skip' || verdict === 'breach';
}

/** Layout spacing, in the same world units the force layout uses. */
export interface ShapeLayoutOptions {
  /** Vertical distance between levels. */
  rowGap: number;
  /** Horizontal distance between siblings on one row. */
  colGap: number;
  /** How far the outsider gutters sit from the widest inside row. */
  gutterGap: number;
  /** Vertical distance between stacked outsiders. */
  gutterRowGap: number;
}

export const DEFAULT_SHAPE_LAYOUT: ShapeLayoutOptions = {
  rowGap: 150,
  colGap: 190,
  gutterGap: 260,
  gutterRowGap: 60,
};

// ------------------------------------------------------------------
//  Which nodes are drawn
// ------------------------------------------------------------------

/** Is `path` inside `folder`? Compared on a separator boundary, or
 *  `src/parsed` reads as being inside `src/parse`. Mirrors `is_inside` in
 *  `analyzer/folder_shape.rs`. */
export function isInside(folder: string, path: string): boolean {
  if (folder === '') return true;
  return path.length > folder.length && path.startsWith(`${folder}/`);
}

/** The immediate child of `folder` that `path` sits in: the subfolder one
 *  level down, or the file itself when it lives in the folder direct. */
export function childHolding(folder: string, path: string): string | null {
  if (!isInside(folder, path)) return null;
  const start = folder === '' ? 0 : folder.length + 1;
  const i = path.indexOf('/', start);
  return i < 0 ? path : path.slice(0, i);
}

/**
 * The resolver pair that turns the entity graph into this picture.
 *
 * Everything inside the folder collapses to the immediate child holding it —
 * which for a nested file is an *ancestor* directory, and the reason
 * `ScopeOf` had to become injectable at all. Everything one hop outside
 * stays a file, so a breach names the file to go and change rather than the
 * folder it happens to live in. Everything else answers `null` and leaves
 * the canvas, which is the filtering half of the view.
 */
export function shapeResolvers(picture: FolderPicture): { grainOf: GrainOf; scopeOf: ScopeOf } {
  const folder = picture.folder;
  const kindOf = new Map(picture.children.map((c) => [c.path, c.kind]));
  const outsiders = new Set(picture.outside.map((o) => o.outside));

  return {
    grainOf: (node: D3Node): GraphLevel => {
      const child = node.file_path ? childHolding(folder, node.file_path) : null;
      // A subfolder draws as one circle, so its files are at folder grain;
      // a file directly inside the folder is its own circle.
      if (child !== null) return kindOf.get(child) === 'folder' ? 'folder' : 'file';
      return 'file';
    },
    scopeOf: (node: D3Node): string | null => {
      if (!node.file_path) return null; // A ghost has no folder to be in.
      const child = childHolding(folder, node.file_path);
      if (child !== null) return kindOf.has(child) ? child : null;
      return outsiders.has(node.file_path) ? node.file_path : null;
    },
  };
}

// ------------------------------------------------------------------
//  Where each node sits
// ------------------------------------------------------------------

export interface ShapePlacement {
  /** Scope path → world position. Keyed by path rather than node id, since
   *  `collapseGraph` mints fresh ids on every republish and a placement has
   *  to survive that — the same reason marks and expansions are paths. */
  positions: Map<string, { x: number; y: number }>;
  /** How many levels the inside graph has. */
  depth: number;
}

/**
 * Rows by level for the folder's own children, gutters either side for the
 * traffic across its boundary.
 *
 * Incoming outsiders go left and outgoing right, so the drawing reads
 * left-to-right as "who needs this folder → the folder → what it needs",
 * while the folder's own hierarchy reads top-to-bottom. One outsider doing
 * both goes left: the question this view is for is who reaches in.
 *
 * Deterministic — every row and gutter is sorted by path — because the
 * layout is asserted in tests and because a picture that reshuffles on an
 * unrelated republish is one the reader has to re-read.
 */
export function shapePlacement(
  picture: FolderPicture,
  options: ShapeLayoutOptions = DEFAULT_SHAPE_LAYOUT,
): ShapePlacement {
  const positions = new Map<string, { x: number; y: number }>();

  const byLevel = new Map<number, PictureChild[]>();
  for (const child of picture.children) {
    const row = byLevel.get(child.level) ?? [];
    row.push(child);
    byLevel.set(child.level, row);
  }
  const levels = [...byLevel.keys()].sort((a, b) => a - b);

  let widest = 0;
  for (const level of levels) {
    const row = byLevel.get(level)!.slice().sort((a, b) => a.path.localeCompare(b.path));
    widest = Math.max(widest, (row.length - 1) * options.colGap);
    const offset = ((row.length - 1) * options.colGap) / 2;
    row.forEach((child, i) => {
      positions.set(child.path, {
        x: i * options.colGap - offset,
        y: level * options.rowGap,
      });
    });
  }

  const incoming = new Set<string>();
  const outgoing = new Set<string>();
  for (const edge of picture.outside) {
    (edge.verdict === 'exit' ? outgoing : incoming).add(edge.outside);
  }
  // Left wins a tie: an outsider that both needs the folder and is needed by
  // it belongs on the side this view is about.
  for (const path of incoming) outgoing.delete(path);

  const gutterX = widest / 2 + options.gutterGap;
  const centre = ((levels.length - 1) * options.rowGap) / 2;
  const place = (paths: Set<string>, x: number) => {
    const sorted = [...paths].sort();
    const offset = ((sorted.length - 1) * options.gutterRowGap) / 2;
    sorted.forEach((path, i) => {
      positions.set(path, { x, y: centre + i * options.gutterRowGap - offset });
    });
  };
  place(incoming, -gutterX);
  place(outgoing, gutterX);

  return { positions, depth: levels.length };
}

// ------------------------------------------------------------------
//  How each edge reads
// ------------------------------------------------------------------

/** The two circles a boundary edge actually runs between: the outsider, and
 *  the *child* the inside file collapses into. */
function boundaryEnds(edge: OutsideEdge): [string, string] {
  return edge.verdict === 'exit' ? [edge.child, edge.outside] : [edge.outside, edge.child];
}

const pairKey = (from: string, to: string) => `${from}->${to}`;

/**
 * Every drawn line's reading, keyed by the scope paths at its ends.
 *
 * Read off the picture and never recomputed from the graph on screen. The
 * levels, the loops and the doors were all decided by the pass that produced
 * the verdict, and a second derivation here would be free to disagree with
 * the number the panel is printing — the failure `ShapeBlocker` is
 * centralised to avoid.
 */
export function shapeEdgeVerdicts(picture: FolderPicture): Map<string, ShapeEdgeVerdict> {
  const out = new Map<string, ShapeEdgeVerdict>();
  const worst = (from: string, to: string, verdict: ShapeEdgeVerdict) => {
    const key = pairKey(from, to);
    const existing = out.get(key);
    if (existing === undefined || SEVERITY[verdict] < SEVERITY[existing]) out.set(key, verdict);
  };
  for (const edge of picture.edges) worst(edge.from, edge.to, edge.verdict);
  for (const edge of picture.outside) {
    const [from, to] = boundaryEnds(edge);
    worst(from, to, edge.verdict);
  }
  return out;
}

/** The reading for one drawn line, or `null` for a line the picture does not
 *  describe — which should not happen, and reads as "no claim" rather than
 *  as "fine" if it does. */
export function verdictFor(
  verdicts: ReadonlyMap<string, ShapeEdgeVerdict>,
  from: string,
  to: string,
): ShapeEdgeVerdict | null {
  return verdicts.get(pairKey(from, to)) ?? null;
}

/**
 * The palette. Violations are the only saturated colours on screen, so the
 * three things worth acting on are the three things that stand out; the
 * readings that are fine sit back in greys and a muted blue.
 *
 * Held here beside `VERDICT_TEXT` rather than in `types/graph.ts` with
 * `LINK_COLORS`, because that table maps *relationship kinds* and this one
 * deliberately overrides it — filing them together would invite a reader to
 * think one extends the other.
 */
export const SHAPE_EDGE_COLORS: Record<ShapeEdgeVerdict, string> = {
  back: '#e94560',   // a loop: the one defect that leaves no reading order
  skip: '#f0883e',   // a level jumped
  breach: '#d29922', // an outsider past the door
  step: '#8b949e',   // the shape you want, and so the quietest thing here
  entry: '#58a6ff',  // arriving where it should
  exit: '#6e7681',   // leaving, which is never a defect
};

/** Legend order: the three defects first, worst first, then the three
 *  readings that are fine. A key sorted by badness is one a reader can stop
 *  reading halfway down. */
export const SHAPE_VERDICT_ORDER: readonly ShapeEdgeVerdict[] = [
  'back',
  'skip',
  'breach',
  'step',
  'entry',
  'exit',
];

/** One line of plain English per reading, for the legend and the tooltip.
 *  The panel must not restate the thresholds — those live in `Thresholds`
 *  and the engine is what compares against them. */
export const VERDICT_TEXT: Record<ShapeEdgeVerdict, string> = {
  step: 'Steps one level down — the shape a reader can follow.',
  skip: 'Skips a level. This is what holds the folder at `tangled`: an edge you have to hold in your head while following the rest.',
  back: 'Runs inside a dependency loop, so it has no direction to read. Nothing above `cyclic` is reachable until the loop is broken.',
  entry: 'Arrives from outside at one of the folder\'s doors — what a folder with a facade looks like.',
  breach: 'Reaches from outside past the door into the interior. This is what holds `entry concentration` down.',
  exit: 'Leaves the folder. Never a defect here — depending outward is what a folder is for.',
};
