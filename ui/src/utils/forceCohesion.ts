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

/**
 * What the innermost group is (UI-103).
 *
 * `folder` is the shipped reading and the default. `file` moves the innermost
 * group one level down, so a region is the unit an author actually composed
 * rather than the directory they filed it in — which is what lets
 * `regionTraffic`'s inside-against-crossing counts answer "is this file
 * cohesive, or a junk drawer?" without inventing a concept or making a new
 * claim about the code.
 *
 * The vocabulary trap this feature was reported through is now gone: the
 * coarse aggregation level used to be called *module*, which read as the
 * language construct and made folder grouping look like a third option
 * beside it. It is called `'folder'` throughout — `collapseGraph`'s
 * `scopeIdFor` derives it as `dirname(file_path)` and `folderKeyOf` below
 * deliberately agrees with it — so the two are visibly the same grain.
 */
export type GroupGrain = 'folder' | 'file';

export const GROUP_GRAINS: readonly GroupGrain[] = ['folder', 'file'];

/** Plural, because the control names what the regions on the canvas *are*,
 *  not what one node belongs to. */
export const GROUP_GRAIN_LABELS: Record<GroupGrain, string> = {
  folder: 'Folders',
  file: 'Files',
};

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
 * Folder level, so the force and the folder aggregation agree on what a
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
  return stripDotSlash(dir);
}

/** The half of `normalizeDir` that is not about directories, so `fileKeyOf`
 *  can normalise the same spelling without inheriting the `.` → `''` rule,
 *  which is meaningless for a file. */
function stripDotSlash(path: string): string {
  return path.startsWith('./') ? path.slice(2) : path;
}

/**
 * The file itself as a group key (UI-103).
 *
 * Normalised exactly as `folderKeyOf` normalises the directory, and for the
 * same reason one level down: AN-015 emits `./ui/src/stores/graph.ts` and
 * `ui/src/stores/graph.ts` from a single analyze run, and compared as raw
 * strings that is one file arriving as two regions with two centroids.
 *
 * Ghosts return `null` on exactly the same terms — they are not in the tree at
 * any grain — which is what lets `groupChainOf` test the pair once.
 */
export function fileKeyOf(node: D3Node): string | null {
  const p = node.file_path;
  if (!p) return null;
  return stripDotSlash(p);
}

/**
 * Where the node's own folder sits in the chain at this grain (UI-113).
 *
 * 0 at folder grain, 1 at file grain, because `groupChainOf` prepends the file
 * and nothing else. Derived here rather than counted by the caller so that the
 * chain and the tier the weighting exempts can never disagree — the same
 * "exactly one definition of a group" rule `groupChainOf` exists to keep.
 *
 * A node with no group has no folder tier either, but the answer is still 0/1
 * rather than null: `initialize` never reaches the weighting for an empty
 * chain, so there is no case for a caller to handle.
 */
export function folderTierDepth(grain: GroupGrain): number {
  return grain === 'file' ? 1 : 0;
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
 * The innermost group a node belongs to at this grain, or `null` when it is in
 * none.
 *
 * What the hover highlight and the link force need: one key to compare for
 * equality. `groupChainOf` is the same answer with everything above it.
 */
export function groupKeyOf(node: D3Node, grain: GroupGrain): string | null {
  return grain === 'file' ? fileKeyOf(node) : folderKeyOf(node);
}

/**
 * A node's group and every group above it, innermost first — **the** single
 * definition of a group (UI-103).
 *
 * `f grouping`'s promise is that the force, the seed, the outlines, the hover
 * membership and the traffic counts all read one answer, so a highlight can
 * never light a different set from the region it is drawn inside. This is that
 * answer once the reader is allowed to choose a grain; `folderKeyOf` and
 * `ancestorChainOf` remain the folder implementation it delegates to rather
 * than a second definition anyone calls directly.
 *
 * The file is an **extra** tier, not one taken from the folders: the ancestor
 * chain keeps its own `limit`, so the folder reach a reader has tuned is
 * unchanged at either grain and only the per-tick pair count grows — nodes × 5
 * rather than nodes × 4 in the worst case, still linear.
 *
 * A file holding one drawn entity needs no special case. `initialize` already
 * drops any tier of fewer than two members before normalising, and hands its
 * share to the tiers that can act — so such a node is pulled by its folder at
 * full strength, which is the "lone file in a folder of its own" rule one
 * level down.
 *
 * `limit` defaults to the force's cap; the outlines and the traffic counts
 * pass `Infinity` for the reason `ancestorChainOf` gives.
 */
export function groupChainOf(
  node: D3Node,
  grain: GroupGrain,
  limit: number = MAX_TIERS,
): string[] {
  const folder = folderKeyOf(node);
  const file = fileKeyOf(node);
  // Ghost. Not in the tree, so in no group at any grain — a shared key would
  // collect every unrelated stdlib reference into one phantom region and drag
  // it across the canvas. The same exemption `folderHulls`, `regionTraffic`
  // and `hoverHighlight` each document. Both keys read the one `file_path`, so
  // they are null together and one test covers the pair.
  if (folder === null || file === null) return [];
  const chain = ancestorChainOf(folder, limit);
  return grain === 'file' ? [file, ...chain] : chain;
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
 * The node's own innermost group (`depth === 0`) is deliberately exempt. That
 * pull is UI-052's promise, and a scope narrowed to a single folder must keep
 * behaving as it did rather than losing its cohesion to a rule about
 * ancestors.
 *
 * `folderDepth` exempts the node's own **folder** from `ANCESTOR_DECAY` as
 * well, wherever the grain has put it (UI-113).
 *
 * The decay prices a tier for being an *ancestor* — one step further from the
 * group the node was actually written into. At file grain that description
 * stops fitting the folder: `groupChainOf` prepends the file, the folder
 * lands at depth 1, and it collects a discount meant for the subtree *above*
 * it while still being a group the node is directly in. Because `initialize`
 * normalises a node's tiers to sum to one, that is not a small trim — the
 * folder's share of the pull falls from 100% to 18%, folders stop being
 * spatially coherent, and `MAX_FOREIGN_SHARE` then correctly refuses to
 * outline them. Measured on this repo's `ui/src` at Medium cohesion: five
 * folder regions at folder grain, one at file grain.
 *
 * That was the shipped behaviour and UI-103 recorded it as an acceptable
 * re-basing, which it was while the reader could only see one grain at a
 * time. It is not acceptable as the answer to "draw files *and* the folders
 * around them", which is what `hullDepth: 2` at file grain promises.
 *
 * The coverage term is deliberately still applied. It is a different rule
 * answering a different question — a tier holding most of the graph has the
 * graph's centroid whatever depth it sits at — and dropping it too was
 * measured as strictly worse: seven file regions lost to buy no extra folder.
 *
 * At folder grain `folderDepth` is 0, the `depth === 0` branch has already
 * returned, and this is byte-identical to the pre-UI-113 function.
 */
export function tierWeightFor(
  depth: number,
  size: number,
  total: number,
  folderDepth: number = 0,
): number {
  if (depth === 0) return 1;
  if (total <= 0) return 0;
  const decay = depth === folderDepth ? 1 : ANCESTOR_DECAY ** depth;
  return decay * (1 - size / total);
}

/**
 * Strength for the current view.
 *
 * Inert at Folder level, where every node already *is* a folder: pulling
 * folder circles toward the centroid of their parent directory would be a
 * second, coarser grouping layered on top of the one the nodes already
 * express, and the reader has no way to tell the two apart.
 */
export function cohesionStrengthFor(level: GraphLevel, choice: CohesionLevel): number {
  if (level === 'folder') return 0;
  return COHESION_STRENGTH[choice];
}

/**
 * The grain actually in force at the level being drawn (UI-103).
 *
 * File grain is an Entity-level reading and collapses to `folder` everywhere
 * else. At File level every node already *is* a file, so every file region
 * would hold exactly one member, fall under `MIN_HULL_MEMBERS` and draw
 * nothing — a control that silently does something is worse than one that is
 * plainly unavailable. At Folder level the force is inert anyway.
 *
 * Deliberately shaped like `cohesionStrengthFor`: the stored choice is what
 * the reader picked and is never overwritten, so returning to Entity level
 * restores the grain they were reading at rather than the fallback.
 */
export function groupGrainFor(level: GraphLevel, choice: GroupGrain): GroupGrain {
  return level === 'entity' ? choice : 'folder';
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
 *
 * `chainOf` is injected rather than imported, for the reason `folderHulls`
 * gives about its own `keysOf`: it keeps the grain out of this module and
 * keeps the force reading the same definition of a group as everything else.
 * The default is the folder grain, so a caller that has no opinion gets the
 * pre-UI-103 force exactly.
 *
 * `folderDepthOf` says where in that chain the node's own folder sits, so the
 * weighting can exempt it from `ANCESTOR_DECAY` (UI-113). It travels beside
 * `chainOf` rather than being inferred from it because this module still has
 * no opinion about what a group key *is* — a caller passing a chain from
 * somewhere other than `groupChainOf` (UI-059's detected communities, when it
 * lands) says which tier is the folder the same way. Callers derive it with
 * `folderTierDepth(grain)` so the pair cannot drift; the default pairs with
 * the default `chainOf`.
 *
 * A **function**, read at `initialize`, for the same reason `chainOf` is one:
 * a grain change re-initializes this force in place rather than rebuilding it
 * (`GraphView.applyGrain`), so a depth captured at construction would go stale
 * against a chain that had already moved — the folder exemption would land on
 * the file tier, or on nothing.
 */
export function forceFolderCohesion(
  chainOf: (node: D3Node) => string[] = (node) => groupChainOf(node, 'folder'),
  folderDepthOf: () => number = () => 0,
): FolderCohesionForce {
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
      const chain = chainOf(ns[i]);
      // Empty = ghost. `groupChainOf` explains why they are in no group at any
      // tier; the consequence here is that they are also out of the coverage
      // denominator, since a tier holding every *placed* node holds the whole
      // tree whatever else is on the canvas.
      if (chain.length === 0) { chains[i] = []; continue; }
      placed++;
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
    // Read once per initialize, not per tier: the grain cannot change midway
    // through a pass, and this is inside the hot path's setup.
    const folderDepth = folderDepthOf();
    const weightAt = (chain: number[], d: number): number =>
      (groupSize[chain[d]] < 2 ? 0 : tierWeightFor(d, groupSize[chain[d]], placed, folderDepth));
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
