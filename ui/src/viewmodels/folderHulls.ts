/**
 * UI-055 — the outline and the name of each folder group.
 *
 * UI-052 and UI-053 made folders spatially coherent, but the grouping is
 * still only *implied* by position: a reader has to infer where one folder
 * ends and the next begins, and has no way to learn what a region is called
 * without reading the node labels inside it and spotting a shared prefix
 * that appears nowhere on the canvas.
 *
 * The outline is the smaller half of the fix. The label is the point — a
 * region marked `stores` states the grouping in words, which no shape or
 * colour can do.
 *
 * There was a previous attempt at hulls, withdrawn because they "clashed
 * with the strict hierarchical layout" of tree mode
 * (GraphView.svelte:497). That was correct at the time and is not a verdict
 * on hulls: an outline around an interleaved set encloses everyone else's
 * nodes too, so every hull overlaps every other and the drawing is worse
 * than nothing. The precondition was a layout where groups are actually
 * separate, which is what UI-052 supplies — hence the block on it.
 *
 * Pure and DOM-free: the caller supplies positions and radii and decides
 * when to run this. Same reason as `forceCohesion` — it keeps the geometry
 * testable and keeps GraphView from growing another decision.
 */

import { polygonHull, polygonContains } from 'd3';
import type { D3Node } from '../types/graph';

/** Below this, a group gets no hull.
 *
 *  Three is the floor `polygonHull` needs anyway (it returns null for fewer
 *  than 3 points, and for collinear ones), but the real reason is editorial:
 *  an outline around two circles reads as decoration on those two circles,
 *  not as a region. */
export const MIN_HULL_MEMBERS = 3;

/** Points sampled around each node when building the hull. Sampling the
 *  circle rather than using the centre is what makes the outline clear the
 *  node's own radius — a hull through the centres would cut every boundary
 *  node in half. */
const RING_POINTS = 8;

/** Gap between the widest node in a group and the outline, in px. Enough to
 *  clear the name label's first line without swallowing a neighbour. */
const DEFAULT_PAD = 20;

/**
 * How far past the group's median radius a member may sit and still shape
 * the outline.
 *
 * This is the difference between a hull that reads as a region and one that
 * reads as noise. A folder is rarely perfectly cohesive — one file gets
 * dragged across the canvas by an edge to somewhere else — and a hull is a
 * *convex* boundary, so a single outlier does not stretch the shape a little:
 * it drags the whole polygon across everything in between. Measured on this
 * repo's `ui` scope, five folders each with one or two strays produced five
 * overlapping sheets covering the entire graph, which is worse than drawing
 * nothing.
 *
 * Trimming to the core is the standard treatment, and it changes what the
 * outline claims: "this is where `stores` lives", not "these are all the
 * files in `stores`". The label is what makes that reading available, and a
 * stray file is still on the canvas — it just sits outside its region, which
 * is itself worth seeing.
 *
 * 2.5 medians is loose enough to keep a genuinely spread-out folder whole
 * and tight enough that one stray cannot claim the canvas.
 */
const OUTLIER_MEDIANS = 2.5;

/** Floor for the trim radius, so a very tight group doesn't reject members
 *  over a median of a handful of pixels. */
const OUTLIER_FLOOR = 120;

/**
 * Share of a hull's contents that may belong to other groups before the
 * hull is dropped.
 *
 * A convex outline around an interleaved group encloses its neighbours, and
 * an outline that contains mostly other folders' nodes is not describing a
 * region — it is asserting a grouping the layout does not have. Drawing it
 * anyway is worse than drawing nothing, because the reader believes it.
 *
 * This is what makes the feature self-regulating rather than a promise the
 * layout cannot keep: at low folder cohesion few groups are separable and
 * few outlines appear; raise cohesion and regions emerge as they earn it.
 * The alternative — always draw, and let the reader sort out five
 * overlapping sheets — was the first build, and it was unusable.
 *
 * 0.35 keeps a region that has a couple of foreign nodes drifting through it
 * and rejects one that is merely a lasso around the middle of the graph.
 */
const MAX_FOREIGN_SHARE = 0.35;

/**
 * Font sizes the two kinds of region name are drawn at, mirroring the
 * `.hull-label` rules in GraphView.
 *
 * World units, and that is what makes a pure module able to reason about
 * text at all: the names live inside the canvas's zoom transform, so they
 * scale with it and a collision in these coordinates is a collision on screen
 * at every magnification.
 */
const LEAF_LABEL_PX = 10;
const PARENT_LABEL_PX = 13;

/**
 * Width of one upper-case character as a share of the font size, tracking
 * included.
 *
 * Measured off the rendered canvas across six region names at both sizes:
 * 0.65 to 0.75. Taken at the top of that range deliberately. Over-estimating
 * separates two names that would just have cleared, which costs a few pixels
 * of lift nobody can see; under-estimating leaves the collision this exists
 * to prevent. The ux-probe's `region-names-stay-readable` check measures the
 * real boxes, so a font change that outgrows the estimate surfaces there
 * rather than silently.
 */
const CHAR_ADVANCE = 0.75;

/** Blank kept between two names that would otherwise touch. */
const LABEL_GAP = 4;

export interface LabelBox { x0: number; x1: number; y0: number; y1: number }

/**
 * The space a region's name occupies.
 *
 * `text-anchor: middle` centres it on `labelX`, and an upper-cased name has
 * no descenders, so the baseline at `labelY` is the bottom of the box.
 */
export function labelBoxOf(h: FolderHull): LabelBox {
  const size = h.hasChildren ? PARENT_LABEL_PX : LEAF_LABEL_PX;
  const half = (h.label.length * size * CHAR_ADVANCE) / 2;
  return { x0: h.labelX - half, x1: h.labelX + half, y0: h.labelY - size, y1: h.labelY };
}

/**
 * Lift any region name that lands on one already placed.
 *
 * Nesting makes this structural rather than unlucky. A parent's outline is
 * sampled from the same ring points as its children's and padded by the same
 * amount, so wherever the child holding the parent's topmost node is itself a
 * drawn region the two hulls share that vertex — and both names, anchored a
 * fixed six pixels above their own hull top, are drawn on the same line.
 * Measured on this repo's `src` scope: `SRC` and `SERVER` three pixels apart,
 * one name over the other and neither readable.
 *
 * Innermost first, so it is the *enclosing* name that moves. Geometrically it
 * is the cheaper move — a parent has open canvas above it where a child has
 * its parent's outline — and it is the right reading: a parent's name is the
 * heading over the regions inside it (UI-070), so it belongs above them and
 * not below.
 *
 * Only ever upward, which keeps UI-055's promise that a name sits outside the
 * shape it names. Pushed down, a name would be inside its own region, over
 * the nodes it is there to describe.
 */
function separateLabels(hulls: FolderHull[]): void {
  const placed: LabelBox[] = [];
  for (let i = hulls.length - 1; i >= 0; i--) {
    const h = hulls[i];
    let box = labelBoxOf(h);
    // Each pass clears the topmost name currently hit, so this one rises past
    // that one for good and a pass per already-placed name is the ceiling.
    // Rising can bring it under a name it did not touch before, which is why
    // this is a loop and not a single correction.
    for (let guard = 0; guard <= placed.length; guard++) {
      const ceiling = topOfHighestHit(box, placed);
      if (ceiling === null) break;
      const lift = box.y1 - (ceiling - LABEL_GAP);
      box = { ...box, y0: box.y0 - lift, y1: box.y1 - lift };
    }
    h.labelY = box.y1;
    placed.push(box);
  }
}

/** Top edge of the highest already-placed name this box runs into, or `null`
 *  when it runs into none — which is the whole answer `separateLabels` needs,
 *  since clearing the highest clears every other it was touching. */
function topOfHighestHit(box: LabelBox, placed: LabelBox[]): number | null {
  let top: number | null = null;
  for (const p of placed) {
    if (box.x1 <= p.x0 || box.x0 >= p.x1) continue;
    if (box.y1 <= p.y0 || box.y0 >= p.y1) continue;
    if (top === null || p.y0 < top) top = p.y0;
  }
  return top;
}

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

/**
 * Drop members far outside the group's core.
 *
 * Distances are measured from the *median* centre rather than the mean: the
 * mean is dragged by the very outliers this is trying to find, so a group
 * with one stray would recentre onto the stray and trim a legitimate member
 * on the far side instead.
 */
function trimOutliers(members: D3Node[]): D3Node[] {
  if (members.length <= MIN_HULL_MEMBERS) return members;
  const cx = median(members.map((n) => n.x!));
  const cy = median(members.map((n) => n.y!));
  const dists = members.map((n) => Math.hypot(n.x! - cx, n.y! - cy));
  const limit = Math.max(median(dists) * OUTLIER_MEDIANS, OUTLIER_FLOOR);
  const kept = members.filter((_, i) => dists[i] <= limit);
  return kept.length >= MIN_HULL_MEMBERS ? kept : members;
}

export interface FolderHull {
  /** Full directory path — the group key, and the tooltip. */
  path: string;
  /** What the outline is labelled with: the directory's own name. */
  label: string;
  size: number;
  /**
   * Whether another drawn region sits inside this one (UI-070).
   *
   * The caller styles a parent differently — outline-only, larger name — and
   * this is the whole of what it needs to know. Deliberately not a depth
   * number: depth would have to be counted in path segments, and this module
   * has no opinion about what a group key *is*.
   */
  hasChildren: boolean;
  /** Padded hull polygon, world coordinates. */
  points: [number, number][];
  /** Anchor for the label: the hull's topmost vertex, raised clear of it —
   *  and raised further still if a name already placed was sitting there.
   *  See `separateLabels`. */
  labelX: number;
  labelY: number;
}

/** `ui/src/stores` → `stores`. The root folder has no name of its own. */
export function basename(path: string): string {
  if (path === '') return '(root)';
  const i = path.lastIndexOf('/');
  return i < 0 ? path : path.slice(i + 1);
}

export interface HullOptions {
  /** Drawn radius of a node, so the outline can clear it. */
  radiusOf: (n: D3Node) => number;
  /**
   * Which groups a node belongs to, innermost first; empty puts it in none.
   *
   * A *chain* rather than one key since UI-070: a file in `ui/src/stores`
   * belongs to that region and to the `ui/src` region around it, and both
   * want an outline. The order is what tells this module which region is
   * inside which — element `d + 1` is the parent of element `d` — so it can
   * decide that a descendant is not a foreigner without ever splitting a
   * path.
   *
   * Injected rather than imported, which keeps this module free of any
   * opinion about *what* a group is. That is the question UI-059 exists to
   * answer: when grouping can come from detected coupling instead of the
   * folder tree, only this argument changes. The caller builds it from
   * `folderKeyOf` and `ancestorChainOf` today, so there is still exactly one
   * definition of a folder group and the force and the outline cannot drift
   * apart.
   */
  keysOf: (n: D3Node) => string[];
  /**
   * How many tiers of regions to draw, counted upward from the innermost.
   *
   * A limit on what is *drawn*, never on membership: every region holds every
   * node beneath it whatever this says, because a region that skipped its
   * deeper descendants would be an outline whose name promises more than the
   * shape contains. A tier is kept when it sits within `tiers` levels above
   * at least one of its members' own folders — so with `tiers: 2`, `ui` earns
   * an outline through the files in `ui/scripts` and then encloses the ones
   * under `ui/src/stores` as well.
   *
   * Getting this backwards was the first build: chains were truncated per
   * node, so `ui` was assembled out of `ui/scripts` alone, ended up smaller
   * than `ui/src`, and was painted on top of the region it visually contains.
   */
  tiers?: number;
  /** How a group key is displayed. Defaults to the path's last segment. */
  labelOf?: (key: string) => string;
  pad?: number;
}

/**
 * Hulls for every group with enough members, largest first.
 *
 * The ordering is painter's algorithm: a big hull drawn first cannot bury a
 * small one that happens to sit inside its bounding box.
 *
 * `nodes` should be the nodes currently *drawn* — a hull stretched to reach
 * a filtered-out node would enclose empty canvas, and the reader has no way
 * to tell that from a genuinely sparse folder.
 */
export function computeFolderHulls(nodes: D3Node[], opts: HullOptions): FolderHull[] {
  const { radiusOf, keysOf, labelOf = basename, pad = DEFAULT_PAD, tiers = Infinity } = opts;

  const groups = new Map<string, D3Node[]>();
  /** How far above its nearest member a region sits. 0 is a folder someone's
   *  file is actually in; 1 is the folder around that one. */
  const rise = new Map<string, number>();
  /** Which region encloses which, learned from the chains rather than from
   *  the paths. `null` means nothing above it was drawn. */
  const parentOf = new Map<string, string | null>();
  const chains = new Map<D3Node, string[]>();
  for (const n of nodes) {
    const chain = keysOf(n);
    // Empty = ghost. Not in the tree, so not in any region — the same
    // exemption `forceCohesion` makes, for the same reason.
    if (chain.length === 0) continue;
    if (!Number.isFinite(n.x) || !Number.isFinite(n.y)) continue;
    chains.set(n, chain);
    for (let d = 0; d < chain.length; d++) {
      const key = chain[d];
      const arr = groups.get(key);
      if (arr) arr.push(n); else groups.set(key, [n]);
      const seen = rise.get(key);
      if (seen === undefined || d < seen) rise.set(key, d);
      // A node whose chain stops early says nothing about what is above its
      // last tier, so a `null` never overwrites a known parent.
      const known = parentOf.get(key);
      if (known === undefined || known === null) parentOf.set(key, chain[d + 1] ?? null);
    }
  }

  const drawable = (key: string): boolean => (rise.get(key) ?? Infinity) < tiers;

  const childCount = new Map<string, number>();
  const redundant = new Set<string>();
  for (const [key, parent] of parentOf) {
    if (parent === null || !groups.has(parent)) continue;
    if (!drawable(key) || !drawable(parent)) continue;
    childCount.set(parent, (childCount.get(parent) ?? 0) + 1);
    // A parent holding exactly what one child holds is that child's outline
    // drawn twice under two names. Nothing is added and the reader has to
    // work out which of two nested shapes means what — the same editorial
    // rule as `MIN_HULL_MEMBERS`, one tier up. Marked here and skipped below.
    if (groups.get(parent)!.length === groups.get(key)!.length) redundant.add(parent);
  }

  const hulls: FolderHull[] = [];
  for (const [path, all] of groups) {
    if (!drawable(path)) continue;
    if (all.length < MIN_HULL_MEMBERS) continue;
    if (redundant.has(path)) continue;
    const members = trimOutliers(all);

    const pts: [number, number][] = [];
    for (const n of members) {
      const r = radiusOf(n) + pad;
      for (let i = 0; i < RING_POINTS; i++) {
        const a = (i / RING_POINTS) * Math.PI * 2;
        pts.push([n.x! + Math.cos(a) * r, n.y! + Math.sin(a) * r]);
      }
    }

    const hull = polygonHull(pts);
    if (!hull) continue;

    // Would this outline be mostly other people's nodes? If so it is a lasso
    // around the middle of the graph, not a region — drop it.
    //
    // A node *under* this region is not other people's: a parent's outline is
    // supposed to contain its children's nodes, and the chain is what says so
    // without this module having to split a path. Left as raw string
    // equality, this test rejected every ancestor outline by construction —
    // which is why the guard had to learn about ancestry rather than have its
    // threshold raised. The threshold is what makes the whole feature
    // self-regulating and it is unchanged.
    let foreign = 0;
    for (const n of nodes) {
      if (chains.get(n)?.includes(path)) continue;
      if (!Number.isFinite(n.x) || !Number.isFinite(n.y)) continue;
      if (polygonContains(hull, [n.x!, n.y!])) foreign++;
    }
    if (foreign / (foreign + members.length) > MAX_FOREIGN_SHARE) continue;

    let topX = hull[0][0];
    let topY = hull[0][1];
    for (const [x, y] of hull) {
      if (y < topY) { topY = y; topX = x; }
    }

    hulls.push({
      path,
      label: labelOf(path),
      // The group's real size, not the trimmed one — the outline is a
      // summary of where the folder lives, and a reader asking how big it is
      // wants the folder's answer, not the polygon's.
      size: all.length,
      hasChildren: (childCount.get(path) ?? 0) > 0,
      points: hull as [number, number][],
      labelX: topX,
      labelY: topY - 6,
    });
  }

  // Painter's algorithm, and with tiers it is load-bearing rather than a
  // nicety: a parent holds every node its children hold, so it is always the
  // larger, and drawing largest first is drawing outside-in. The path
  // tie-break keeps two regions of equal size from swapping places between
  // frames, which would flicker.
  hulls.sort((a, b) => b.size - a.size || (a.path < b.path ? -1 : 1));
  // After the sort, because the order is what decides which of two colliding
  // names holds its place and which one rises.
  separateLabels(hulls);
  return hulls;
}
