/**
 * UI-102 — link strength and rest distance as functions of how much of the
 * folder tree the two endpoints share.
 *
 * The other half of UI-052. That ticket gave the simulation a force that
 * knows what a folder is, and left untouched the force it has to argue with:
 * one distance and d3's default strength for every edge, folder or no folder.
 * For a node with one or two edges d3's default evaluates to `1 / min(deg,
 * deg)` = 1.0 and biases the whole correction onto that node, so it is pinned
 * at the rest distance from its neighbour while cohesion offers at most
 * `0.35 * alpha` — 0.05 at the shipped default — to pull it home. Folder
 * grouping then visibly works on nodes with no edges and fails on nodes with
 * them, which is the shape of the report UI-102 came from.
 *
 * `forceCohesion`'s header rejects path-weighted links as a *replacement* for
 * cohesion, and is right: weighting only reshapes pairs that already have an
 * edge, so two files in the same folder that never reference each other stay
 * scattered. That is an argument about what makes groups appear. This is
 * about what tears them apart afterwards, and the two are complementary —
 * cohesion assembles a folder, this stops an edge towing members back out.
 *
 * Store-free and DOM-free for the same reason as `forceCohesion`: the caller
 * owns which simulation these are attached to, which keeps the arithmetic
 * unit-testable without a canvas.
 */

import type { D3Link, D3Node } from '../types/graph';

/**
 * Rest distance for an edge whose endpoints share their whole folder path.
 *
 * Deliberately the pre-UI-102 uniform distance. Intra-folder layout was never
 * the complaint, and a fix that also retuned it would make every existing
 * grouping measurement incomparable with the ones taken before it.
 */
export const KIN_DISTANCE = 120;

/**
 * Rest distance for an edge with no kinship left — a ghost at one end, or two
 * folders far enough apart in the tree that `KINSHIP_DECAY` has run out.
 *
 * The single most effective of the three constants, and the one whose reason
 * is least obvious. Weakening a crossing edge only decides how *hard* it
 * argues; the rest distance decides what it is arguing *for*. Left at 120 a
 * weakened edge still wants the node on top of its foreign neighbour and
 * merely takes longer to get it there. Moved out, the same edge is satisfied
 * by a node sitting at the near rim of its own folder, facing the folder it
 * talks to — which is the arrangement a reader wants anyway, and the reason
 * this raises separation between folders as a side effect rather than at the
 * expense of it.
 *
 * Measured on `ringWorld` in `scripts/graph-grouping.test.ts` — four sibling
 * folders, one strayed leaf each, settled under the real forces —
 * holding the other two constants: 240 leaves the stray ratio at 1.44 and 320
 * takes it to 1.36, while the mean intra-folder distance as a share of all
 * pairs *improves* at both. The cost is a canvas about 20% wider, which
 * auto-fit absorbs.
 */
export const STRANGER_DISTANCE = 320;

/**
 * Share of d3's default strength an edge with no kinship left keeps.
 *
 * Not zero. A crossing edge still has to mean something: charge is `-400` and
 * the only other restraint on a subtree is the `0.05` centring pair, so an
 * inter-folder link worth nothing lets the parts drift until auto-fit reaches
 * its zoom floor and the whole graph is unreadably small — the failure
 * UI-022 documented from the other direction.
 *
 * Measured, this is the *weakest* of the three levers: dropping it from 0.25
 * to 0.08 moved the stray ratio by 0.01–0.06 at every decay tried, because
 * the kinship term is already doing most of the scaling by the time an edge
 * is between folders far enough apart to reach the floor. 0.15 is chosen as
 * the middle of a range that barely matters, which is the honest reason to
 * prefer the safer end of it.
 */
export const STRANGER_STRENGTH = 0.15;

/**
 * Per level of separation, the share of kinship that survives.
 *
 * Deliberately the same number as `forceCohesion`'s `ANCESTOR_DECAY`, and for
 * the same reason: kinship in the tree should fade smoothly with distance up
 * it, not fall off a cliff at the first differing segment. One constant for
 * both is also a claim worth being able to make — the force that assembles a
 * folder and the force that decides how hard an edge may pull out of it
 * disagree about nothing.
 *
 * A gentler 0.6 was tried first, on the argument that two levels of
 * separation is *siblings* — the most common edge in a real repo — and that
 * pricing the common case low would weaken the whole graph. Measured, that
 * argument was backwards: at 0.6 the stray ratio only reached 1.51 against a
 * 1.78 baseline, and 0.45 took it to 1.36 while *improving* folder tightness
 * rather than spending it. Going further to 0.3 bought another 0.05 and made
 * a sibling edge nearly indistinguishable from an unrelated one, which is the
 * flattening this whole module exists to avoid.
 */
const KINSHIP_DECAY = 0.45;

/**
 * Edges between two folders in the directory tree.
 *
 * The number of levels you climb from one to their common ancestor, plus the
 * levels back down to the other. Same folder is 0. Siblings are 2 —
 * `ui/src/stores` to `ui/src` to `ui/src/components` — whether they sit two
 * levels down or six. A folder and its own child are 1.
 *
 * A ghost (`null` key — external or stdlib, no `file_path`) is `Infinity`
 * from everything, including another ghost. Ghosts are already outside every
 * group in `forceCohesion` and outside every hull; a stdlib reference with
 * many callers would otherwise tow each of them out of its own folder, which
 * is this ticket's failure with a phantom at one end.
 */
export function folderTreeDistance(a: string | null, b: string | null): number {
  if (a === null || b === null) return Infinity;
  if (a === b) return 0;
  const sa = a === '' ? [] : a.split('/');
  const sb = b === '' ? [] : b.split('/');
  let shared = 0;
  while (shared < sa.length && shared < sb.length && sa[shared] === sb[shared]) shared++;
  return (sa.length - shared) + (sb.length - shared);
}

/**
 * How closely two folders are related, in [0, 1]. 1 is the same folder.
 *
 * Continuous rather than a same-folder boolean, and that is the whole design.
 * A boolean says `ui/src/stores → ui/src/components` and `ui/src/stores →
 * docs/adr` are the same kind of edge, which is exactly the flattening
 * UI-069 taught the cohesion force not to do: the first pair sits inside a
 * subtree the layout is already trying to hold together, and weakening it as
 * hard as the second would pull that subtree apart in the name of grouping.
 *
 * Measured on *tree distance* rather than on the shared prefix as a fraction
 * of path depth, which was the first thing tried here and is wrong in the
 * direction that matters. Shared-over-depth calls `ui/src/stores` and
 * `ui/src/components` 0.67 related while calling `a` and `b` at the repo
 * root 0 related — it reads deep siblings as near-kin and shallow siblings as
 * strangers, when the tree says both pairs are siblings. Since almost every
 * edge in a real repo is between deep siblings, that form left the common
 * case at 0.75 strength and the fix with nothing to do.
 */
export function folderKinship(a: string | null, b: string | null): number {
  const sep = folderTreeDistance(a, b);
  return sep === Infinity ? 0 : KINSHIP_DECAY ** sep;
}

/** Linear interpolation from the stranger end to the kin end. Kinship 1 must
 *  return exactly the pre-UI-102 constant, which is why the kin value is the
 *  base and the stranger value the offset rather than the other way round. */
const lerpByKinship = (kin: number, kinValue: number, strangerValue: number): number =>
  kinValue + (strangerValue - kinValue) * (1 - kin);

export interface LinkFolderWeights {
  /** `d3.forceLink().distance` accessor. */
  distance(link: D3Link): number;
  /** `d3.forceLink().strength` accessor — d3's own default, scaled by how
   *  much folder the endpoints share. Takes the accessor's third argument
   *  because the degree normalisation is a property of the whole link set. */
  strength(link: D3Link, i: number, links: D3Link[]): number;
  /** The kinship the two accessors agree on, exposed for tests and for the
   *  probe rather than recomputed by either. */
  kinshipOf(link: D3Link): number;
}

/**
 * Both endpoints as nodes.
 *
 * d3 rewrites `source`/`target` from id strings to node objects in
 * `forceLink.initialize`, *before* it calls the strength and distance
 * accessors, so by the time either runs the objects are there. A link whose
 * ends are still strings has not been through a simulation, which for these
 * accessors means there is no node to read a `file_path` off — scored as a
 * stranger pair rather than crashing, since that is also what a ghost gets.
 */
function endpointsOf(link: D3Link): [D3Node | null, D3Node | null] {
  const s = typeof link.source === 'object' ? link.source : null;
  const t = typeof link.target === 'object' ? link.target : null;
  return [s, t];
}

/**
 * Build the two accessors.
 *
 * `keyOf` is injected rather than imported, for the reason UI-055 records for
 * `computeFolderHulls`: a `.ts`-less import of `forceCohesion` resolves under
 * Vite and not under bare Node, which breaks the unit suite. GraphView passes
 * `folderKeyOf`, so the force, the hulls and this cannot drift apart, and
 * UI-059's question about non-folder groupings changes one argument.
 *
 * ### Why the degree count is recomputed here
 *
 * Handing `forceLink` a `.strength()` accessor *discards* its default,
 * `1 / min(deg(source), deg(target))`, silently — d3 only applies that when
 * no strength was supplied. A flat strength in its place makes every hub
 * receive the same correction from each of its fifty edges and tear itself
 * apart. So the default is reproduced and then scaled, and the only thing
 * this changes about the simulation is the folder term.
 *
 * Counted the way d3 counts, which is not the way `viewmodels/linkDegrees`
 * counts: d3 bumps both ends of a self-link, `linkDegrees` bumps one, because
 * it is answering "how connected to *others*" for the size channel. Matching
 * d3 matters more than sharing code here — the point of the number is to
 * reproduce a specific formula.
 *
 * The count is cached against the links array it was built from. d3 calls the
 * strength accessor once per link per `initialize`, so recounting inside it
 * would be quadratic on every `links()` swap, and the incremental path swaps
 * on every filter toggle.
 */
export function linkFolderWeights(keyOf: (n: D3Node) => string | null): LinkFolderWeights {
  let countedFor: readonly D3Link[] | null = null;
  let degree = new Map<string, number>();

  const degreeFor = (links: D3Link[]): Map<string, number> => {
    if (links === countedFor) return degree;
    countedFor = links;
    degree = new Map<string, number>();
    const bump = (id: string) => degree.set(id, (degree.get(id) ?? 0) + 1);
    for (const l of links) {
      const [s, t] = endpointsOf(l);
      if (s) bump(s.id);
      if (t) bump(t.id);
    }
    return degree;
  };

  const kinshipOf = (link: D3Link): number => {
    const [s, t] = endpointsOf(link);
    if (!s || !t) return 0;
    return folderKinship(keyOf(s), keyOf(t));
  };

  return {
    kinshipOf,
    distance: (link) => lerpByKinship(kinshipOf(link), KIN_DISTANCE, STRANGER_DISTANCE),
    strength: (link, _i, links) => {
      const deg = degreeFor(links);
      const [s, t] = endpointsOf(link);
      // A link d3 has not resolved contributes to no count, so `min` would be
      // 0 and the strength infinite. 1 is d3's own answer for a link whose
      // endpoints have one edge each, which is what an unresolved pair is.
      const min = Math.min(deg.get(s?.id ?? '') ?? 1, deg.get(t?.id ?? '') ?? 1);
      const base = 1 / Math.max(1, min);
      return base * lerpByKinship(kinshipOf(link), 1, STRANGER_STRENGTH);
    },
  };
}
