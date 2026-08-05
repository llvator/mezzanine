/**
 * The marked set: the scopes a reader has picked out of the canvas to look at
 * together, and the one gesture that turns a picture of files into a picture
 * of the entities inside them.
 *
 * The canvas already answers "which files talk to each other" at File level
 * and "which entities talk to each other" at Entity level, and the only bridge
 * between them was `drillIn` — one node, whole view replaced. That is the
 * wrong shape for the question this exists for: a relationship runs *between*
 * files, so wanting to see it in detail means wanting two or three of them and
 * nothing else. Drilling into one end of an edge throws away the other.
 *
 * **Paths, never ids** — the same rule as `expandedScopes` and `hiddenFiles`,
 * and here it is load-bearing rather than merely convenient. `collapseGraph`
 * mints fresh ids on every level change, and the whole point of a mark is to
 * survive the level change it *causes*: mark three files, drill, and the
 * canvas that comes back is drawn from entity ids that did not exist when the
 * marks were made.
 *
 * A leaf module: no stores, no Svelte, so `stores/scope.ts` can prune with it
 * without closing an import cycle back through `stores/marks.ts`.
 */

import type { D3Node } from '../types/graph';

/**
 * The scope path a node stands for, or `null` when it stands for none.
 *
 * `file_path` answers for every node at every level, which is why nothing here
 * tests `kind_raw`: `collapseGraph` sets a File rollup's `file_path` to its own
 * path and a Module rollup's to its directory, and an entity carries the file
 * it was parsed from. So one field means "the narrowest scope this circle is
 * evidence of" whatever the canvas is currently drawing — and marking an
 * entity is a legitimate way to say "the file this lives in", which is what
 * makes the gesture available before the reader has collapsed anything.
 *
 * Two nodes have no path and cannot be marked. Ghosts (external and stdlib
 * references) are not code in this repo at all, so there is nothing to narrow
 * to. The root Module — the rollup for files sitting at the repo root — is
 * excluded because its path is `''`, which as a scope means the whole
 * repository: marking it and drilling would be a no-op dressed as a narrowing,
 * and `''` is also how a ghost's absent path is spelt, so the two cannot be
 * told apart downstream.
 */
export function markPathOf(node: D3Node): string | null {
  if (node.tags?.includes('ghost')) return null;
  return node.file_path ? node.file_path : null;
}

/** Flip one path in or out of the set, leaving the rest alone. */
export function toggleMarked(marks: ReadonlySet<string>, path: string): Set<string> {
  const next = new Set(marks);
  if (!next.delete(path)) next.add(path);
  return next;
}

/**
 * Every path the loaded graph is evidence of: each node's file, and the
 * directory holding it.
 *
 * The parent directory is what makes a Module mark survivable — a mark made at
 * Module level names a directory that no node's `file_path` will equal once
 * the view drops to File or Entity level, and pruning against files alone
 * would silently drop it exactly when the reader drilled in to use it.
 *
 * Computed from the *scoped entity* graph rather than the collapsed one, so
 * the answer does not depend on which level happens to be drawn.
 */
export function livePaths(nodes: readonly D3Node[]): Set<string> {
  const live = new Set<string>();
  for (const n of nodes) {
    if (!n.file_path) continue;
    live.add(n.file_path);
    const i = n.file_path.lastIndexOf('/');
    live.add(i >= 0 ? n.file_path.slice(0, i) : '');
  }
  return live;
}

/**
 * Drop marks for paths the graph no longer holds.
 *
 * Returns the *same set* when nothing changed, so a caller can hand it
 * straight back to a store without waking every subscriber on each scope
 * change. A mark on something off screen is invisible state: it would keep
 * widening a later drill by a path the reader can no longer see, or point one
 * at a file that has since been deleted.
 */
export function prunedMarks(
  marks: ReadonlySet<string>,
  live: ReadonlySet<string>,
): ReadonlySet<string> {
  const next = new Set([...marks].filter((p) => live.has(p)));
  return next.size === marks.size ? marks : next;
}
