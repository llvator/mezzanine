/**
 * UI-052 — folder cohesion, a d3 force that pulls each node toward the
 * centroid of the nodes sharing its parent directory.
 *
 * UI-069 — and toward the centroid of each *ancestor* directory above that,
 * at a decaying share of the same strength. See `ancestorChainOf`.
 *
 * The simulation previously had no notion of structure at all: one link
 * distance and one charge for every node, so the only thing that could bring
 * two entities together was an edge between them. That makes the *layout*
 * answer a different question than the reader is asking. "What lives
 * together" is not "what calls what", and on a full-repo graph the second
 * question alone produces a hairball with no discernible parts.
 *
 * Why not weight the existing links by path proximity instead — the obvious
 * cheaper fix? Because it only reshapes pairs that already have an edge. Two
 * files in the same folder that never reference each other stay scattered,
 * and those are exactly the pairs whose co-location is the group signal. A
 * folder cohesive by topic rather than by calls would not cluster at all.
 * An attraction that does not depend on edges existing is the thing that
 * makes groups appear.
 *
 * Store-free and DOM-free on purpose: the caller owns when it runs and what
 * strength it runs at, which keeps this unit testable and keeps GraphView
 * from growing another decision.
 */

import type { D3Node, GraphLevel } from '../types/graph';

/** d3 writes velocities onto the node objects; `D3Node` doesn't declare
 *  them (GraphView reaches them through `as any`). Narrowed here rather
 *  than widening the shared type, which would move the `npm run check`
 *  baseline for no gain. */
type SimNode = D3Node & { vx?: number; vy?: number };

export type CohesionLevel = 'off' | 'low' | 'medium' | 'high';

export const COHESION_LEVELS: readonly CohesionLevel[] = ['off', 'low', 'medium', 'high'];

/** Velocity-nudge coefficients, applied as `(centroid - position) * k * alpha`.
 *
 *  `off` is exactly 0 rather than a small number: the force must be provably
 *  inert at that setting, so the layout at `off` is the pre-UI-052 layout and
 *  not a nearly-identical one. */
export const COHESION_STRENGTH: Record<CohesionLevel, number> = {
  off: 0,
  low: 0.05,
  medium: 0.15,
  high: 0.35,
};

export const COHESION_LABELS: Record<CohesionLevel, string> = {
  off: 'Off',
  low: 'Low',
  medium: 'Med',
  high: 'High',
};

/**
 * The group a node belongs to: the directory holding its file.
 *
 * Deliberately the same derivation `collapseGraph`'s `scopeIdFor` uses for
 * Module level, so the force and the module aggregation agree on what a
 * group *is*. A file at the repo root gets `''` — the root is a real folder
 * and its files really do belong together.
 *
 * Ghosts (external/stdlib references, `file_path === ''`) return null. They
 * are not in the tree, and giving them a shared key would collect every
 * unrelated stdlib reference into one phantom group and drag it across the
 * canvas.
 */
export function folderKeyOf(node: D3Node): string | null {
  const p = node.file_path;
  if (!p) return null;
  const i = p.lastIndexOf('/');
  return normalizeDir(i < 0 ? '' : p.slice(0, i));
}

/**
 * `./ui/src/stores` and `ui/src/stores` are the same directory.
 *
 * One analyze run emits both spellings — AN-015, still open. Measured on this
 * repo's served payload, five directories appear under both, and the biggest
 * split is `ui/src/stores`: 744 entities spelled one way and 85 the other.
 * Compared as raw strings that is two groups for one folder, which is not a
 * cosmetic flaw here — it is two centroids pulling one directory apart, two
 * hulls carrying the same name, and a hover that lights a tenth of the folder
 * it was asked about.
 *
 * Normalising at the consumer follows what `utils/refPaths.ts` already does
 * with the same payload for the same reason: a UI should be defensive about
 * the shapes it is handed, and this is not the root fix.
 */
function normalizeDir(dir: string): string {
  if (dir === '.') return '';
  return dir.startsWith('./') ? dir.slice(2) : dir;
}

/**
 * How much of a node's pull each tier above its own folder receives, per
 * level (UI-069).
 *
 * The flat force treats `ui/src/stores` and `ui/src/components` as two
 * unrelated atoms — as unrelated as `ui/src/stores` and `docs/adr` — because
 * `folderKeyOf` is one string compared for equality. Nothing has ever told
 * the simulation that the first two share a parent, so nothing keeps a
 * subtree together once the folder centroids start drifting.
 *
 * 0.45 per level keeps a node's own folder dominant while leaving a real
 * subtree enough pull to hold together. Leaf dominance is not a taste
 * preference: UI-055 only draws a region once its group is spatially
 * distinct, so an ancestor strong enough to blur the leaves would take the
 * outlines away with it.
 *
 * Raising it does not buy much. Measured on this repo's `src` scope at 0.45,
 * 0.8 and 1.0, the settled kinship ratio moved by less than the run-to-run
 * spread of the measurement at the time — the layout there is dominated by
 * the links, and the tier that would express the subtree holds most of the
 * canvas and is priced down by `tierWeightFor` accordingly.
 */
const ANCESTOR_DECAY = 0.45;

/**
 * Tiers a node is pulled by, counting its own folder as the first.
 *
 * A cap rather than the full path depth, because the fourth ancestor is
 * already down to 9% of the pull and each tier costs a pass over the node set
 * per tick. The tail is arithmetic nobody can see.
 */
const MAX_TIERS = 4;

/**
 * A folder and its ancestors, nearest first.
 *
 * `ancestorChainOf('ui/src/stores')` is
 * `['ui/src/stores', 'ui/src', 'ui']` — the node is pulled by its own folder,
 * then by the subtree its folder sits in, then by the top-level directory.
 * Siblings share the second entry, cousins only the third, and that
 * difference is the whole feature: kinship in the tree becomes proximity on
 * the canvas without anything being wired between the groups.
 *
 * Why not an invisible spring between group centroids, which is the other
 * obvious way to say the same thing? Because d3 links act on nodes, so it
 * needs a phantom centroid node in `simulation.nodes()` — which then
 * participates in charge and collision, and corrupts the array the DOM join
 * and the hull's `hullNodeIds` are keyed on. This says it entirely inside the
 * force, and nothing outside the module has to know.
 *
 * The root directory is never a tier: `ancestorChainOf('ui')` is `['ui']`,
 * not `['ui', '']`. Which of the remaining tiers deserve any pull is
 * `tierWeight`'s question, not this one.
 *
 * `limit` defaults to the force's cap. The outlines pass `Infinity`: a region
 * has to hold *every* node beneath it or its name is a lie, and a truncated
 * chain gives `ui` the files under `ui/scripts` while denying it the ones
 * under `ui/src/stores`. UI-070 decides which tiers to *draw* separately,
 * from how far above its members each one sits.
 */
export function ancestorChainOf(key: string, limit: number = MAX_TIERS): string[] {
  const chain = [key];
  let cur = key;
  while (chain.length < limit) {
    const i = cur.lastIndexOf('/');
    if (i < 0) break;
    cur = cur.slice(0, i);
    chain.push(cur);
  }
  return chain;
}

/**
 * How much of an ancestor tier's decayed pull survives, given the share of
 * the graph it holds.
 *
 * A tier's centroid says something about its members only in so far as there
 * are non-members for them to be distinguished from. A directory holding
 * every drawn node has the whole graph's centroid, so pulling toward it is a
 * second, weaker `forceCenter` wearing a folder's name — a contraction that
 * looks like structure and carries none. Scaling by the share of the graph a
 * tier *excludes* prices that in continuously, and a tier holding everything
 * prices out to exactly zero.
 *
 * This replaced a rule that stripped only the drawn set's exact common
 * ancestor, which was too literal to survive first contact with a real repo:
 * on `ui`, the `ui/src` tier held every node but the handful under
 * `ui/scripts`, so it was 95% a centering force, kept its full weight, and
 * measurably *loosened* the leaf folders it stole that weight from — the
 * kinship ratio went from 0.845 to 0.927, the wrong way. Coverage catches
 * that case and the exact-ancestor case with one rule.
 *
 * The node's own folder (`depth === 0`) is deliberately exempt. That pull is
 * UI-052's promise, and a scope narrowed to a single folder must keep
 * behaving as it did rather than losing its cohesion to a rule about
 * ancestors.
 */
export function tierWeightFor(depth: number, size: number, total: number): number {
  if (depth === 0) return 1;
  if (total <= 0) return 0;
  return ANCESTOR_DECAY ** depth * (1 - size / total);
}

/**
 * Strength for the current view.
 *
 * Inert at Module level, where every node already *is* a folder: pulling
 * module circles toward the centroid of their parent directory would be a
 * second, coarser grouping layered on top of the one the nodes already
 * express, and the reader has no way to tell the two apart.
 */
export function cohesionStrengthFor(level: GraphLevel, choice: CohesionLevel): number {
  if (level === 'module') return 0;
  return COHESION_STRENGTH[choice];
}

export interface FolderCohesionForce {
  (alpha: number): void;
  initialize(nodes: D3Node[]): void;
  strength(): number;
  strength(value: number): FolderCohesionForce;
}

/**
 * Build the force. Group membership is resolved once per `initialize` (which
 * d3 re-runs whenever `simulation.nodes()` is set), and each tick is two
 * linear passes — accumulate centroids, then apply. Recomputing a node's
 * group centroid per node per tick would be quadratic, which at the 400-node
 * render budget is the difference between a smooth settle and a stutter.
 *
 * With UI-069 those passes are over (node, tier) pairs rather than nodes, so
 * the cost is nodes × depth with depth capped at `MAX_TIERS` — still linear,
 * and bounded by a constant multiple of the old cost.
 */
export function forceFolderCohesion(): FolderCohesionForce {
  let nodes: SimNode[] = [];
  /**
   * Per-node tier lists, flattened. Node `i` owns
   * `tierGroup[tierStart[i] … tierStart[i + 1])` with matching `tierWeight`.
   *
   * Flat arrays rather than an array of arrays per node: this is walked twice
   * per tick, and the nested form allocates a cursor per node per pass.
   */
  let tierStart: number[] = [0];
  let tierGroup: number[] = [];
  let tierWeight: number[] = [];
  let groupSize: number[] = [];
  let sumX: number[] = [];
  let sumY: number[] = [];
  let k = 0;

  const force = ((alpha: number): void => {
    if (k <= 0 || groupSize.length === 0) return;

    for (let g = 0; g < groupSize.length; g++) { sumX[g] = 0; sumY[g] = 0; }
    for (let i = 0; i < nodes.length; i++) {
      const x = nodes[i].x ?? 0;
      const y = nodes[i].y ?? 0;
      // A node counts toward its own folder's centroid *and* every ancestor's
      // — an ancestor's centroid is the centroid of its whole subtree, which
      // is what makes siblings converge on a shared point.
      for (let t = tierStart[i]; t < tierStart[i + 1]; t++) {
        sumX[tierGroup[t]] += x;
        sumY[tierGroup[t]] += y;
      }
    }

    const step = k * alpha;
    for (let i = 0; i < nodes.length; i++) {
      const node = nodes[i];
      const x = node.x ?? 0;
      const y = node.y ?? 0;
      let dx = 0;
      let dy = 0;
      // Weights were normalised at initialize, so this sums to one pull of
      // magnitude `step` however many tiers the node has. That is what keeps
      // the off/low/medium/high ladder meaning what a reader already tuned it
      // to mean.
      for (let t = tierStart[i]; t < tierStart[i + 1]; t++) {
        const g = tierGroup[t];
        dx += (sumX[g] / groupSize[g] - x) * tierWeight[t];
        dy += (sumY[g] / groupSize[g] - y) * tierWeight[t];
      }
      if (dx === 0 && dy === 0) continue;
      node.vx = (node.vx ?? 0) + dx * step;
      node.vy = (node.vy ?? 0) + dy * step;
    }
  }) as FolderCohesionForce;

  force.initialize = (ns: D3Node[]): void => {
    nodes = ns as SimNode[];
    groupSize = [];
    sumX = [];
    sumY = [];

    // Pass one: every node's chain, and how many nodes each tier holds.
    // Sizes cannot be known until the whole set is walked, and pass two needs
    // them to decide which tiers are worth pulling toward.
    const index = new Map<string, number>();
    const chains = new Array<number[]>(ns.length);
    let placed = 0;
    for (let i = 0; i < ns.length; i++) {
      const key = folderKeyOf(ns[i]);
      // null = ghost. Not in the tree, so in no group at any tier — giving
      // them a shared key would collect every unrelated stdlib reference into
      // one phantom group and drag it across the canvas. They are also out of
      // the coverage denominator: a tier holding every *placed* node holds
      // the whole tree, whatever else is on the canvas.
      if (key === null) { chains[i] = []; continue; }
      placed++;
      const chain = ancestorChainOf(key);
      const groups = new Array<number>(chain.length);
      for (let d = 0; d < chain.length; d++) {
        let g = index.get(chain[d]);
        if (g === undefined) {
          g = groupSize.length;
          index.set(chain[d], g);
          groupSize.push(0);
          sumX.push(0);
          sumY.push(0);
        }
        groups[d] = g;
        groupSize[g]++;
      }
      chains[i] = groups;
    }

    // Pass two: drop the tiers that cannot pull, and normalise what is left.
    //
    // A tier of one has its own centroid and would only pull itself toward
    // where it already is; a tier holding the whole tree prices out to zero
    // through `tierWeightFor`. Dropping both *before* normalising rather than
    // skipping them per tick is what hands their share to the tiers that can
    // act: a lone file in a folder of its own is then pulled by its parent at
    // full strength instead of losing that share to arithmetic.
    tierStart = new Array<number>(ns.length + 1);
    tierGroup = [];
    tierWeight = [];
    const weightAt = (chain: number[], d: number): number =>
      (groupSize[chain[d]] < 2 ? 0 : tierWeightFor(d, groupSize[chain[d]], placed));
    for (let i = 0; i < ns.length; i++) {
      tierStart[i] = tierGroup.length;
      const chain = chains[i];
      let total = 0;
      for (let d = 0; d < chain.length; d++) total += weightAt(chain, d);
      if (total === 0) continue;
      for (let d = 0; d < chain.length; d++) {
        const w = weightAt(chain, d);
        if (w === 0) continue;
        tierGroup.push(chain[d]);
        tierWeight.push(w / total);
      }
    }
    tierStart[ns.length] = tierGroup.length;
  };

  force.strength = ((value?: number) => {
    if (value === undefined) return k;
    k = value;
    return force;
  }) as FolderCohesionForce['strength'];

  return force;
}
