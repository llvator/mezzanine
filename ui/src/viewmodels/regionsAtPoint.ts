/**
 * UI-071 — which region the pointer is standing in.
 *
 * The outline and its name (UI-055) tell a reader where a region is, but only
 * if they can see the whole shape. Zoomed into a corner of a big graph the
 * boundary and the label are both off screen, and the canvas goes back to
 * saying nothing about where you are. Since UI-070 the honest answer is also
 * plural: with nested tiers the pointer is inside several regions at once,
 * and "you are in `stores`" hides the more useful half of the sentence — that
 * `stores` sits inside `src`, which sits inside `ui`.
 *
 * So this returns the whole enclosing stack rather than one winner, and
 * leaves it to the caller whether to show all of it. The *tightest* region is
 * the last element, which is the one a focus gesture should act on: it is the
 * most specific claim the reader can be making by pointing there.
 *
 * Pure and DOM-free for the same reason `computeFolderHulls` is: the caller
 * owns the zoom transform and decides when to ask. Handing this a world-space
 * point rather than a pointer event is what keeps it testable without a
 * browser.
 */

import { polygonContains } from 'd3';
import type { FolderHull } from './folderHulls';

/**
 * The regions containing `(x, y)`, widest first.
 *
 * Widest-first *is* outermost-first wherever the regions genuinely nest: a
 * parent holds every node its children hold, and `computeFolderHulls` drops a
 * parent that holds exactly what one child holds, so an ancestor is always
 * strictly larger than its descendant. That is the same fact the hulls' own
 * paint order relies on, and reusing it keeps the reading order on screen
 * identical to the drawing order underneath it.
 *
 * Two regions that merely overlap — neither one's ancestor — can both contain
 * the point too, and then the order is only "bigger first". That is a
 * tolerable answer and not a claim of ancestry: the caller renders the stack
 * as a trail, and the paths on it are what the reader checks. Inventing a
 * hierarchy by splitting paths here would be the one thing this module and
 * `computeFolderHulls` have both refused to do, because what counts as a
 * group is UI-059's decision and not this file's.
 *
 * The hit test uses the hull polygon, while the canvas strokes a
 * Catmull-Rom curve through it — so the drawn edge bulges a pixel or two
 * outside what this reports. Testing the polygon is the conservative side of
 * that difference: it never claims a region the reader cannot see they are
 * in.
 */
export function regionsAtPoint(
  hulls: readonly FolderHull[],
  x: number,
  y: number,
): FolderHull[] {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return [];
  const hits = hulls.filter(
    (h) => h.points.length >= 3 && polygonContains(h.points, [x, y]),
  );
  // Sorted here rather than trusted from the input: the caller's array is in
  // paint order today, and a hover trail that silently depended on that would
  // break the day the drawing order changes for a drawing reason.
  hits.sort((a, b) => b.size - a.size || (a.path < b.path ? -1 : 1));
  return hits;
}

/**
 * The region a click at this point should act on: the tightest one.
 *
 * Separate from the trail because it is a different question — the trail is
 * what the reader is told, this is what a gesture does — and because the
 * answer has to agree with what the canvas does under the pointer. The
 * innermost hull is painted last and therefore takes the event, so a focus
 * gesture routed through the DOM and one routed through this function land on
 * the same region.
 */
export function tightestRegion(trail: readonly FolderHull[]): FolderHull | null {
  return trail.length === 0 ? null : trail[trail.length - 1];
}

/** Whether two trails name the same regions in the same order — the test that
 *  keeps a pointer move that changed nothing from re-rendering the card. */
export function sameRegions(a: readonly FolderHull[], b: readonly FolderHull[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((h, i) => h.path === b[i].path);
}
