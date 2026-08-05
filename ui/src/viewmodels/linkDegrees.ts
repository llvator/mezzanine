/**
 * How many relationships each node has in the graph being drawn.
 *
 * The domain of the `degree` size channel (`nodeEncoding`), which answers
 * "how connected is this entity" from the picture itself rather than from a
 * precomputed metric. It is deliberately a property of the *view*: the count
 * follows the aggregation level and the open scopes, because `collapseGraph`
 * rebuilds the link set on every one of those changes.
 *
 * Its own module, not a function inside `nodeEncoding`, for one practical
 * reason: `nodeEncoding` imports `stores/quality` for the severity scores,
 * and that store reaches its siblings with extensionless imports Vite
 * resolves and bare Node does not — so nothing that imports it can be unit
 * tested. Nothing here imports a value at all, which is what makes
 * `npm run test:degree` possible.
 */

import type { D3Link, D3Node } from '../types/graph';

/** Either shape a link end can take. D3's force simulation rewrites
 *  `source`/`target` from id strings to the node objects themselves, in
 *  place, so which one arrives depends on whether the simulation has run —
 *  reading both is what keeps the count from collapsing to zero one tick
 *  after mount. */
function endId(e: string | D3Node): string {
  return typeof e === 'string' ? e : e.id;
}

/**
 * Links incident to each node, keyed by node id. Nodes with no links are
 * absent from the map; callers read a miss as 0.
 *
 * A self-link counts once, not twice: it says nothing about how connected a
 * node is to *others*, which is the question being asked. Parallel links
 * between the same pair each count — at file and module level
 * `collapseGraph` has already merged them into one weighted link, and at
 * entity level a call and a type-use between the same two entities are
 * genuinely two relationships.
 */
export function linkDegrees(links: D3Link[]): Map<string, number> {
  const deg = new Map<string, number>();
  const bump = (id: string) => deg.set(id, (deg.get(id) ?? 0) + 1);
  for (const l of links) {
    const s = endId(l.source);
    const t = endId(l.target);
    bump(s);
    if (t !== s) bump(t);
  }
  return deg;
}
