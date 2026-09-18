/**
 * The spec pane's second channel onto the code canvas: a *highlight*, not a
 * filter.
 *
 * The filter (`crossFilterPaths`) answers "show me only what this owns" by
 * deleting everything else. That is the right answer to that question and the
 * wrong answer to a different one, which readers kept asking with the same
 * gesture: *where does this Feature live in the code I am already looking at*.
 * Filtering to it destroys the context the question is about — you learn what
 * the Feature contains and lose what it sits next to, which is usually the
 * thing you opened the split view to see.
 *
 * So the same `cr:` claims are computed twice and spent differently. Both
 * legs go through `pathsClaim` on file paths rather than ids, for the reason
 * `f.spec_pairing.filter` gives: `collapseGraph` mints fresh ids for File and
 * Module rollups, and `file_path` is what survives the collapse, so one
 * predicate answers for an entity, its file circle and its module circle.
 *
 * The functions are here rather than in the store because what is worth
 * pinning down is set arithmetic — which sources contribute, and what an
 * empty contribution means — and none of it needs a browser to demonstrate.
 * See `scripts/spec-highlight.test.ts`.
 */

import { pathsClaim } from '../utils/refPaths.ts';

/** The fields `claimedNodeIds` reads. Deliberately not `D3Node`: the answer
 *  depends on two of its forty-odd fields, and saying so is what lets the
 *  test build a subject in one line. */
export interface Claimable {
  id: string;
  file_path: string;
}

/**
 * Which spec entities are lighting the canvas up right now.
 *
 * The union of what is pinned and what is under the pointer, and the union
 * is the point: pinning one entity and then sweeping the pane compares the
 * two, which is the gesture "does this Functionality touch the same code as
 * the one I pinned" is made of. Intersecting or letting hover replace the
 * pins would each collapse that back to one answer at a time.
 *
 * A hover that is already pinned contributes nothing new — a `Set` is doing
 * the deduplication, so passing over a pinned entity does not make it light
 * up twice or, worse, appear to change what is lit.
 */
export function highlightSources(
  pinned: ReadonlySet<string>,
  hovered: string | null,
): string[] {
  const sources = new Set(pinned);
  if (hovered) sources.add(hovered);
  return [...sources];
}

/**
 * The code entities a set of declared paths claims, as ids.
 *
 * `null` and `[]` mean the same thing here, and that is the one place this
 * differs from the filter — deliberately. For the filter the distinction is
 * load-bearing: `null` is "no filter" and `[]` is "the selection declares no
 * code", which must empty the canvas so that missing `cr:` coverage becomes
 * visible. A highlight has no such power. Nothing is hidden either way, so
 * "highlight nothing" is the honest rendering of both, and the pane says
 * *why* it is nothing with its ◌/○/✖ mark rather than by leaving the canvas
 * in a state the reader has to interpret.
 */
export function claimedNodeIds(
  paths: readonly string[] | null,
  nodes: readonly Claimable[],
): Set<string> {
  const ids = new Set<string>();
  if (!paths || paths.length === 0) return ids;
  for (const node of nodes) {
    if (pathsClaim(paths, node.file_path)) ids.add(node.id);
  }
  return ids;
}

/**
 * Add or drop a pin.
 *
 * Separate from the filter's `specSelection` even though both are sets of
 * spec ids, because they answer to different gestures and must be able to
 * disagree: the whole arrangement this exists for is one entity filtering the
 * canvas while another is pinned onto what is left. Merging them would make
 * the second pin silently widen the filter.
 */
export function togglePin(pinned: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(pinned);
  if (!next.delete(id)) next.add(id);
  return next;
}
