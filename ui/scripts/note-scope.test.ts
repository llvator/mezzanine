/**
 * The markdown scope closure — a document's links survive a narrow scope.
 *
 * The case that motivated it cannot be clicked through quickly: the VS Code
 * extension scopes the graph to the active file on every editor switch, and a
 * `.md` file holds exactly one entity whose every relationship leaves the
 * file. What the reader saw was an absence — one circle, and a Details pane
 * with no Relationships section — which is also what a genuinely unconnected
 * note looks like, so the two have to be told apart in a test.
 *
 *   npm run test:notescope
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { noteLinkNeighbours } from '../src/viewmodels/noteScope.ts';
import type { D3Node, D3Link, GraphData } from '../src/types/graph.ts';

/** Enough of a node to answer the closure; the rest is inert here. */
function node(id: string, tags: string[], file_path = `${id}.md`): D3Node {
  return {
    id, original_id: id, name: id, qualified_name: id,
    kind: 'note', kind_raw: 'Note', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags, source_code: null, fields: [],
    impl_blocks: [], language: 'Markdown',
  } as D3Node;
}

function link(source: string, target: string): D3Link {
  return { source, target, kind: 'references', kind_raw: 'References', order: null } as D3Link;
}

function graph(nodes: D3Node[], links: D3Link[]): GraphData {
  return { nodes, links };
}

const MD = ['markdown'];

test('a scoped note brings in what it links to and what links to it', () => {
  const g = graph(
    [node('readme', MD), node('design', MD), node('inbound', MD)],
    [link('readme', 'design'), link('inbound', 'readme')],
  );
  const added = noteLinkNeighbours(g, new Set(['readme'])).map((n) => n.id).sort();
  assert.deepEqual(added, ['design', 'inbound']);
});

test('one hop only — the neighbour of a neighbour stays out', () => {
  // Two hops off a hub document is most of a doc corpus, and the reader who
  // wants the corpus can select it.
  const g = graph(
    [node('a', MD), node('b', MD), node('c', MD)],
    [link('a', 'b'), link('b', 'c')],
  );
  assert.deepEqual(noteLinkNeighbours(g, new Set(['a'])).map((n) => n.id), ['b']);
});

test('nothing is returned for a scope that holds no note', () => {
  // The whole of a code-only graph: the scan must not cost anything there.
  const g = graph(
    [node('fn', [], 'src/lib.rs'), node('caller', [], 'src/main.rs')],
    [link('caller', 'fn')],
  );
  assert.deepEqual(noteLinkNeighbours(g, new Set(['caller'])), []);
});

test('a code entity at the other end of a note edge is not pulled in', () => {
  // Notes at both ends. A link to source code is a `cr:` ref rather than an
  // edge today, so this costs nothing — it is what stops the closure from
  // becoming "pull in everything adjacent" if that ever changes.
  const g = graph(
    [node('readme', MD), node('fn', [], 'src/lib.rs')],
    [link('readme', 'fn')],
  );
  assert.deepEqual(noteLinkNeighbours(g, new Set(['readme'])), []);
});

test('an unresolved note is a neighbour, a code ghost is not', () => {
  // Obsidian's ghost node — a link to a document that does not exist — is a
  // node the reader is meant to see. A code ghost stands for an external
  // symbol and has no file at all.
  const g = graph(
    [node('readme', MD), node('missing', ['markdown', 'unresolved']), node('Vec', ['ghost'], '')],
    [link('readme', 'missing'), link('readme', 'Vec')],
  );
  assert.deepEqual(noteLinkNeighbours(g, new Set(['readme'])).map((n) => n.id), ['missing']);
});

test('nodes already in scope are not returned again', () => {
  const g = graph(
    [node('a', MD), node('b', MD)],
    [link('a', 'b'), link('b', 'a')],
  );
  assert.deepEqual(noteLinkNeighbours(g, new Set(['a', 'b'])), []);
});

test('endpoints d3 has rewritten to node objects still resolve', () => {
  // `filterToSelection` runs against the full graph, which the simulation has
  // already mutated in place: after one tick `link.source` is a node, not an
  // id, and an id comparison against it matches nothing.
  const a = node('a', MD), b = node('b', MD);
  const g = graph([a, b], [{ source: a, target: b, kind: 'references', kind_raw: 'References', order: null } as unknown as D3Link]);
  assert.deepEqual(noteLinkNeighbours(g, new Set(['a'])).map((n) => n.id), ['b']);
});
