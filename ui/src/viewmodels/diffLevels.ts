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

export interface LevelPlan {
  /** Nodes the rung draws. */
  visible: Set<string>;
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
 */
export function planDiffLevel(
  level: DiffLevel,
  candidates: ReadonlySet<string>,
  edits: ReadonlySet<string>,
  links: readonly LevelEdge[],
): LevelPlan {
  const visible = new Set<string>();
  for (const id of edits) {
    if (candidates.has(id)) visible.add(id);
  }

  if (level === 'rewiring') {
    // Both ends of a changed edge, not just the far end from a seed node.
    // That looks over-broad and is the whole point: an entity that swaps one
    // call for another has identical source and identical metrics, so the
    // diff calls it an *impact* and it is never in `edits`. The edge is the
    // only witness there is, and dropping it because neither end qualified
    // would lose exactly the change this rung is named for.
    for (const l of links) {
      if (!l.changed) continue;
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

  return { visible, dimmed, changedEdgesOnly: level !== 'neighbourhood' };
}
