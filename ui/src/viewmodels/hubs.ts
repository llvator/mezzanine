/**
 * UI-056 — demoting the nodes that make the graph unreadable.
 *
 * Most hairballs are not caused by many nodes having many edges. They are
 * caused by a *few* that everything touches: a types module, a logger, a
 * string utility, a config accessor. Their edges reach every corner of the
 * graph by construction, so they cross every group boundary no matter where
 * the layout puts them, and their pull drags unrelated groups together —
 * actively undoing the separation UI-052 and UI-053 create.
 *
 * No layout fixes this. A node connected to two hundred others has no
 * position that avoids two hundred long edges. The clutter has to be removed
 * at the edge level, not solved geometrically.
 *
 * Pure, and store-free for the same reason as its siblings in this set.
 */

import type { D3Node } from '../types/graph';

export const HUB_COUNTS: readonly number[] = [3, 5, 10];

/** Default N. Five is where this repo's `ui` scope stops having obvious
 *  hubs; the control exists because that number is a property of the
 *  codebase, not of the tool. */
export const DEFAULT_HUB_COUNT = 5;

/**
 * The `n` most-depended-on nodes among those given.
 *
 * Ranked on **fan-in relative to the drawn node count**, not on raw fan-in.
 * A node with forty dependents is a hub in a sixty-node view and unremarkable
 * in a two-thousand-node one, and the whole point of this control is to be
 * usable at whatever scope the reader has chosen. Ranking on the raw number
 * would demote nothing on a small scope and the wrong things on a large one.
 *
 * In practice this is the same ordering as raw fan-in for a fixed node set —
 * the divisor is constant — so the ratio matters for the *threshold* below,
 * not for the sort. It is written this way so a future absolute cut-off has
 * the right quantity to hand.
 *
 * Nodes with no metrics are never candidates. A missing `fan_in` means the
 * analyzer had nothing to say, and treating unknown as zero would be
 * harmless here but treating it as high would silently hide real edges.
 */
export function rankHubs(nodes: D3Node[], n: number): string[] {
  if (n <= 0 || nodes.length === 0) return [];
  const scored = nodes
    .filter((d) => d.metrics?.fan_in != null && d.metrics.fan_in > 0)
    .map((d) => ({ id: d.id, share: d.metrics!.fan_in! / nodes.length, fanIn: d.metrics!.fan_in! }));

  scored.sort((a, b) => (b.share - a.share) || a.id.localeCompare(b.id));
  return scored.slice(0, n).map((s) => s.id);
}

/**
 * Names for the demoted set, in rank order, so the panel can say what it
 * took away.
 *
 * Naming them is not decoration. A control that silently removes a node's
 * edges leaves the reader believing nothing depends on it — the graph would
 * be lying, and there would be nothing on screen to correct it.
 *
 * Colliding names are qualified by their folder. This repo demotes both
 * `ui/src/stores/graph.ts` and `ui/src/types/graph.ts`, and a list reading
 * "graph.ts, scope.ts, serveMode.ts, endpoint.ts, graph.ts" is worse than
 * unhelpful — it reads as a bug in the tool, and it leaves the reader unable
 * to tell which of the two had its edges hidden. Only the colliding entries
 * are qualified; qualifying all of them would trade one unreadable list for
 * another.
 */
export function hubNames(nodes: D3Node[], ids: string[]): string[] {
  const byId = new Map(nodes.map((d) => [d.id, d]));
  const picked = ids.map((id) => ({ id, node: byId.get(id) }));

  const seen = new Map<string, number>();
  for (const { id, node } of picked) {
    const name = node?.name ?? id;
    seen.set(name, (seen.get(name) ?? 0) + 1);
  }

  return picked.map(({ id, node }) => {
    const name = node?.name ?? id;
    if ((seen.get(name) ?? 0) < 2 || !node?.file_path) return name;
    const dir = node.file_path.slice(0, node.file_path.lastIndexOf('/'));
    const parent = dir.slice(dir.lastIndexOf('/') + 1);
    return parent ? `${parent}/${name}` : name;
  });
}
