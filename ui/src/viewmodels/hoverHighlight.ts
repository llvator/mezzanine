/**
 * UI-054 — what a hover answers.
 *
 * The canvas has always answered exactly one question on hover: what is
 * reachable from this node within N relationship hops. That is the
 * *relationship* question. The question a reader asks when trying to find
 * structure in a hairball is the *membership* one — "what else lives here?"
 * — and until UI-055 there was no way to ask it at all short of collapsing
 * the whole view to file or module level and losing every entity.
 *
 * This module owns the membership half. The BFS half stays in GraphView,
 * where it has the adjacency it needs.
 *
 * Read affordance, not a fix: it lets a reader interrogate the tangle. It
 * does not untangle anything, and it is not a substitute for the cohesion
 * force or the hulls.
 */

import type { D3Node } from '../types/graph';

/** What hovering a node lights up. */
export type HoverMode = 'connections' | 'group';

export const HOVER_MODES: readonly HoverMode[] = ['connections', 'group'];

export const HOVER_MODE_LABELS: Record<HoverMode, string> = {
  connections: 'Links',
  group: 'Folder',
};

export const HOVER_MODE_TITLES: Record<HoverMode, string> = {
  connections: 'Highlight what this node connects to, N hops out',
  group: 'Highlight everything in the same folder',
};

/**
 * Ids of the nodes sharing `target`'s group, including `target` itself.
 *
 * Returns an empty set when the target belongs to no group — a ghost has no
 * place in the tree, and lighting up every other ghost would invent a
 * grouping that does not exist. An empty set means "highlight nothing",
 * which is the honest answer, and the caller renders it as such.
 *
 * `keyOf` is injected for the same reason `computeFolderHulls` takes it:
 * when UI-059 decides whether a group can come from detected coupling
 * instead of the folder tree, this is a one-argument change rather than a
 * rewrite. It also keeps the highlight and the outline agreeing on what a
 * region is — a hover that lit a different set from the hull it is drawn
 * inside would be worse than no hover at all.
 */
export function groupMemberIds(
  nodes: D3Node[],
  target: D3Node,
  keyOf: (n: D3Node) => string | null,
): Set<string> {
  const key = keyOf(target);
  if (key === null) return new Set();
  const ids = new Set<string>();
  for (const n of nodes) {
    if (keyOf(n) === key) ids.add(n.id);
  }
  return ids;
}
