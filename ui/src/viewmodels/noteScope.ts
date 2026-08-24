/**
 * A document's neighbourhood: the notes one link away from an in-scope note.
 *
 * Every other language nao parses puts several entities in a file, so a scope
 * narrower than the repo still leaves a graph behind — a class with its
 * methods, a file with its functions and the calls between them. Markdown does
 * not. The parser emits one Note per file, because a link addresses a whole
 * document, so *every* relationship a note has crosses the file boundary by
 * construction. Strict file scoping therefore cuts all of them at once: scope
 * to a single `.md` — which is what the VS Code extension does on every editor
 * switch — and the canvas draws one circle with nothing attached, while the
 * Details pane renders no Relationships section at all, because there is
 * nothing left to list.
 *
 * The file boundary is not the unit of meaning for a document, so the scope
 * closes over the unit that is: the note, plus what it links to and what links
 * to it. Same shape of argument as the Elevator closure in
 * `filterToSelection` — a spec's file split is an editing convenience, so
 * containment closes across it — and the same restraint: this widens the
 * picture, so it stays small enough to still be a narrowing.
 *
 * ONE hop, and never transitively. Two hops off a hub document is most of a
 * doc corpus, and a reader who wants the corpus can select it. Notes only at
 * both ends, which costs nothing today (a link to source code becomes a `cr:`
 * ref rather than an edge, per the parser) but keeps this from quietly
 * becoming a general "pull in everything adjacent" rule if that changes.
 *
 * Pure: no stores, no DOM — see `scripts/note-scope.test.ts`.
 */

import type { D3Node, D3Link, GraphData } from '../types/graph';

/** A real markdown document. Unresolved link targets qualify — Obsidian's
 *  ghost note is a node the reader is meant to see — but a code ghost, which
 *  carries no file and stands for an external symbol, does not. */
function isNote(node: D3Node | undefined): node is D3Node {
  return !!node && !!node.tags?.includes('markdown') && !node.tags.includes('ghost');
}

/** Link endpoints, which d3 rewrites from ids to node objects in place. */
function linkEnds(link: D3Link): [string, string] {
  return [
    typeof link.source === 'object' ? (link.source as D3Node).id : link.source,
    typeof link.target === 'object' ? (link.target as D3Node).id : link.target,
  ];
}

/**
 * The notes to add to a scope that already holds `included`.
 *
 * Returns only nodes not already in scope, so the caller can push them
 * straight onto its node list. Empty — allocating nothing beyond the id scan —
 * when the scope holds no note at all, which is every code-only graph.
 */
export function noteLinkNeighbours(full: GraphData, included: ReadonlySet<string>): D3Node[] {
  const byId = new Map(full.nodes.map((n) => [n.id, n]));

  const seeds = new Set<string>();
  for (const id of included) {
    if (isNote(byId.get(id))) seeds.add(id);
  }
  if (seeds.size === 0) return [];

  const added = new Map<string, D3Node>();
  for (const link of full.links) {
    const [src, tgt] = linkEnds(link);
    for (const [from, to] of [[src, tgt], [tgt, src]] as const) {
      if (!seeds.has(from) || included.has(to) || added.has(to)) continue;
      const other = byId.get(to);
      if (isNote(other)) added.set(to, other);
    }
  }
  return [...added.values()];
}
