/**
 * Why the canvas is blank, when it is (UI-064).
 *
 * Three states put nothing on screen and only two of them explain
 * themselves. "No scope picked" has the empty-state card, and "too much to
 * draw" has the overflow card (UI-062). The third — *the scope produced
 * nodes and every one of them is filtered out of sight* — was silent: the
 * DOM held the nodes, the stats bar read `Shown: 0`, and the canvas was
 * white. That is how the diff-filter bug hid for as long as it did.
 *
 * The distinction that matters here is between what is **built** and what is
 * **seen**. `drawCeiling` measures the built set, because that is what costs
 * render time. This module measures the seen set, because that is what the
 * user is looking at — and a dimmed node counts as seen only when the dim
 * leaves something visible. `diffDimOpacity` defaults to 0, where "dimmed"
 * means `display: none`, so the two sets differ precisely in the case worth
 * reporting.
 *
 * Pure, and separate from `displayPlan` for the same reason `drawCeiling` is:
 * it is a decision worth testing on its own, and its only import is a type.
 */

import type { DisplayPlan } from './displayPlan';

/** What to tell the user when the canvas is blank but the scope is not. */
export interface BlankCanvas {
  /** Nodes the scope produced — all of them filtered out of sight. */
  built: number;
}

/**
 * The plan's *seen* count: visible nodes, plus dimmed ones only when the dim
 * actually leaves them on screen.
 */
export function seenNodeCount(plan: DisplayPlan): number {
  const dimmed = plan.dimOpacity > 0 ? plan.dimmedNodeIds.size : 0;
  return plan.visibleNodeIds.size + dimmed;
}

/**
 * Whether the canvas is blank for a reason the user can act on.
 *
 * `built` is the node count of the graph the plan was computed against —
 * post-collapse, so it is the number of things that *could* have been drawn.
 *
 * Returns null when some other card already owns the screen:
 *
 * - nothing built ⇒ no scope is selected, and the empty-state card says so;
 * - `overflow` ⇒ the ceiling gated it, and the overflow card says so.
 *
 * Both of those are already explained, and stacking a second explanation on
 * top of an existing one is its own kind of unhelpful.
 */
export function blankCanvasReason(plan: DisplayPlan, built: number): BlankCanvas | null {
  if (built <= 0) return null;
  if (plan.overflow) return null;
  if (seenNodeCount(plan) > 0) return null;
  return { built };
}
