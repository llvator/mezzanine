/**
 * The two scopes that both used to be spelled `''` in `collapseGraph`.
 *
 * At module level the repo ROOT directory is a scope like any other — the
 * files a checkout keeps at its top, which in a doc graph are its hub
 * documents (README, CLAUDE.md, CONTEXT.md). A ghost has no file at all and
 * belongs to no scope: an external symbol is not a place in the tree, and a
 * real graph carries thousands of them.
 *
 * They shared the empty string, and the edge merge tested `!srcScope` — so
 * every relationship of every root-level file was dropped along with the
 * ghosts' at both collapsed levels. The reader saw a circle with no lines and
 * a Details pane with no Relationships section, which is indistinguishable
 * from a file that genuinely depends on nothing.
 *
 *   npm run test:collapseroot
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { collapseGraph } from '../src/viewmodels/collapseGraph.ts';
import type { D3Link, D3Node, GraphData } from '../src/types/graph.ts';

function node(id: string, file_path: string, tags: string[] = []): D3Node {
  return {
    id, original_id: id, name: id, qualified_name: id,
    kind: 'note', kind_raw: 'Note', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags, source_code: null, fields: [],
    impl_blocks: [], language: 'Markdown',
  } as D3Node;
}

const link = (source: string, target: string): D3Link =>
  ({ source, target, kind: 'references', kind_raw: 'References', order: null } as D3Link);

const ends = (data: GraphData) =>
  data.links.map((l) => [
    typeof l.source === 'object' ? (l.source as D3Node).id : l.source,
    typeof l.target === 'object' ? (l.target as D3Node).id : l.target,
  ]);

/** A root-level document pointing into a folder, and one pointing back. */
const REPO: GraphData = {
  nodes: [node('readme', 'README.md'), node('design', 'docs/design.md')],
  links: [link('readme', 'design'), link('design', 'readme')],
};

test('a root-level file keeps its edges at module level', () => {
  const collapsed = collapseGraph(REPO, 'module');
  assert.deepEqual(collapsed.nodes.map((n) => n.original_id).sort(), ['', 'docs']);
  assert.deepEqual(ends(collapsed).sort(), [['_root_', 'docs'], ['docs', '_root_']]);
});

test('and at file level, where its scope was never empty to begin with', () => {
  const collapsed = collapseGraph(REPO, 'file');
  assert.deepEqual(ends(collapsed).sort(), [['README_md', 'docs_design_md'], ['docs_design_md', 'README_md']]);
});

test('a fileless ghost is no scope at all — no circle, no edges', () => {
  // It used to land on `''` and draw as a node named "(root)": at file level
  // that is a rollup of every external reference in the graph, sitting on the
  // canvas under the name of a directory it has nothing to do with.
  const withGhost: GraphData = {
    nodes: [...REPO.nodes, node('Vec', '', ['ghost'])],
    links: [...REPO.links, link('readme', 'Vec')],
  };
  const collapsed = collapseGraph(withGhost, 'file');
  assert.deepEqual(collapsed.nodes.map((n) => n.original_id).sort(), ['README.md', 'docs/design.md']);
  assert.equal(ends(collapsed).length, 2);
});
