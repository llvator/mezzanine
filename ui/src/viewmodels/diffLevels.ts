/**
 * How wide a diff draws (UI-088).
 *
 * The diff used to be a *node* filter and nothing else: it decided which
 * entities survived, and the canvas then drew every pre-existing edge that
 * happened to run between two of them. So the lines on screen were not the
 * lines that changed, and the lines that changed were mostly not on screen —
 * measured on a Hybris repo, 37 of 153 drawn edges had actually moved, while
 * 128 new ones were invisible because one end was not itself edited.
 *
 * Three rungs, narrowest first. Each answers a different question, and the
 * ladder is ordered so that moving down always removes and moving up always
 * adds:
 *
 * | rung            | nodes                            | edges          |
 * |-----------------|----------------------------------|----------------|
 * | `edits`         | what you edited                  | changed only   |
 * | `rewiring`      | + far ends of the changed edges  | changed only   |
 * | `neighbourhood` | + everything one hop away        | all            |
 *
 * This module decides membership and nothing else — it takes ids, not nodes,
 * so it can be unit-tested without a graph, a store, or a browser. Which
 * nodes count as *edited* is `displayPlan`'s call, because that question needs
 * the scope rollup and the unknown-file rule; by the time it gets here the
 * answer is a set.
 */

/** One head-graph edge, reduced to what the rungs care about. */
export interface LevelEdge {
  src: string;
  tgt: string;
  /** True when the diff reported this edge as appeared. A disappeared edge
   *  is never in this list: it has no line in the head graph to describe. */
  changed: boolean;
}

export type DiffLevel = 'edits' | 'rewiring' | 'neighbourhood';

export const DIFF_LEVELS: readonly DiffLevel[] = ['edits', 'rewiring', 'neighbourhood'];

export function isDiffLevel(v: unknown): v is DiffLevel {
  return typeof v === 'string' && (DIFF_LEVELS as readonly string[]).includes(v);
}

/**
 * Which half of the seed the ladder starts from (UI-109).
 *
 * A second axis, and deliberately not a fourth rung. The rungs are *widths*
 * and are totally ordered; new code and pre-existing code are siblings —
 * neither contains the other — so dropping them into the ladder would break
 * the one property that makes it legible.
 *
 * It narrows `edits` *before* `planDiffLevel` runs, which is what makes the
 * composition worth having: `new` + `neighbourhood` draws what the new code
 * plugs into, a reading neither control can produce alone. Filtering after the
 * ladder would be incoherent — the far end of a changed edge is context, and
 * context has no business being judged by the facet that chose the seed.
 *
 * `all` is the default and reproduces the pre-UI-109 picture exactly.
 */
export type DiffSeedFacet = 'all' | 'new' | 'existing';

/** Which side of the change one edit is on. The facets are these two plus the
 *  option of not choosing, which is what ties the type to the control. */
export type EditKind = Exclude<DiffSeedFacet, 'all'>;

export const SEED_FACETS: readonly DiffSeedFacet[] = ['all', 'new', 'existing'];

export function isSeedFacet(v: unknown): v is DiffSeedFacet {
  return typeof v === 'string' && (SEED_FACETS as readonly string[]).includes(v);
}

/**
 * The seed `planDiffLevel` starts from, once the facet has had its say.
 *
 * Takes the edits already sided — `diffVerdict.editKind` is what decides that,
 * for the same reason it decides membership: the question needs the scope
 * rollup and the unknown-file rule, and neither belongs here.
 *
 * At `all` the result is every key, which is the identity this whole change
 * rests on: a reader who never touches the control gets the picture the ladder
 * drew before it existed.
 */
export interface FacetSplit {
  /** The edits the facet keeps. What the rungs grow from. */
  seed: Set<string>;
  /**
   * The edits it left out — empty at `all`.
   *
   * Carried rather than discarded because `rewiring` does not only grow from
   * the seed: it scans every changed edge and takes both ends, which is how it
   * catches a swap that no node filter can see. That clause is right, and it
   * silently undoes the facet — measured on this repo, `existing` at
   * `rewiring` drew 1,571 of the 1,576 nodes `all` drew, because every edge
   * touching one of the 688 new entities recruited it straight back. So the
   * rung is told what the reader excluded, and declines to recruit it.
   */
  excluded: Set<string>;
}

export function splitEdits(
  facet: DiffSeedFacet,
  edits: ReadonlyMap<string, EditKind>,
): FacetSplit {
  const seed = new Set<string>();
  const excluded = new Set<string>();
  for (const [id, kind] of edits) {
    if (facet === 'all' || facet === kind) seed.add(id);
    else excluded.add(id);
  }
  return { seed, excluded };
}

export interface LevelPlan {
  /** Nodes the rung draws. */
  visible: Set<string>;
  /**
   * The part of `visible` the rung recruited rather than the seed (UI-112).
   *
   * A rung above `edits` earns its keep by drawing code the reader did not
   * touch — the far end of a changed edge, or anything one hop out — and the
   * flat `visible` set said nothing about which was which. On screen that
   * reads as "widening the ladder edited more code": the neighbourhood rung
   * can multiply the node count several times over, and every one of them is
   * drawn exactly like the handful that changed.
   *
   * Membership is still the rung's decision — this only says how each drawn
   * node earned its place, so the canvas can weight the two differently.
   * Empty at `edits`, where every drawn node is an edit by construction.
   */
  context: Set<string>;
  /** Candidates the rung left out. Dimmed rather than hidden, so the Rest
   *  slider can fade them back as context. */
  dimmed: Set<string>;
  /** True when only edges the diff reported as changed may be drawn. False
   *  at `neighbourhood`, where untouched wiring is the point. */
  changedEdgesOnly: boolean;
}

/**
 * Which nodes each rung draws.
 *
 * `candidates` is everything that passed the non-diff filters — kind,
 * language, file, scope. The rungs only ever narrow it, so a node hidden by a
 * kind filter cannot reappear because a changed edge points at it. `edits` is
 * the seed; it is intersected with `candidates` here rather than being
 * trusted, because the two are computed from different questions.
 *
 * `excluded` is the other half of a split seed (UI-109) — the edits the facet
 * left out, and the one thing `rewiring` may not recruit. Empty by default, so
 * a caller that does not split gets the ladder exactly as UI-088 built it.
 */
export function planDiffLevel(
  level: DiffLevel,
  candidates: ReadonlySet<string>,
  edits: ReadonlySet<string>,
  links: readonly LevelEdge[],
  excluded: ReadonlySet<string> = new Set(),
): LevelPlan {
  const visible = new Set<string>();
  for (const id of edits) {
    if (candidates.has(id)) visible.add(id);
  }
  // Snapshotted before the rungs grow `visible`, because that is exactly what
  // separates the seed from what the rung recruited. The seed the *facet*
  // chose, not `edits` as passed in: an edit the reader excluded is not on
  // screen, and if a rung recruits it back it is context like any other.
  const seeded = new Set(visible);

  if (level === 'rewiring') {
    // Both ends of a changed edge, not just the far end from a seed node.
    // That looks over-broad and is the whole point: an entity that swaps one
    // call for another has identical source and identical metrics, so the
    // diff calls it an *impact* and it is never in `edits`. The edge is the
    // only witness there is, and dropping it because neither end qualified
    // would lose exactly the change this rung is named for.
    //
    // The one exception is an end the reader explicitly excluded. An edge that
    // is changed *because a new entity is on it* says nothing about the
    // existing half, and letting it through here is what made the facet look
    // like it had no effect above the narrowest rung. A swap between two
    // entities that are in neither half still comes through, in both facets:
    // it belongs to neither, and this rung is the only place it is ever drawn.
    for (const l of links) {
      if (!l.changed) continue;
      if (excluded.has(l.src) || excluded.has(l.tgt)) continue;
      if (candidates.has(l.src)) visible.add(l.src);
      if (candidates.has(l.tgt)) visible.add(l.tgt);
    }
  } else if (level === 'neighbourhood') {
    // One hop from the seed, computed against the seed rather than against
    // the growing set — otherwise each added neighbour would recruit its own
    // neighbours and the rung would walk the whole component.
    const seed = new Set(visible);
    for (const l of links) {
      if (seed.has(l.src) && candidates.has(l.tgt)) visible.add(l.tgt);
      if (seed.has(l.tgt) && candidates.has(l.src)) visible.add(l.src);
    }
  }

  const dimmed = new Set<string>();
  for (const id of candidates) {
    if (!visible.has(id)) dimmed.add(id);
  }

  const context = new Set<string>();
  for (const id of visible) {
    if (!seeded.has(id)) context.add(id);
  }

  return { visible, context, dimmed, changedEdgesOnly: level !== 'neighbourhood' };
}
