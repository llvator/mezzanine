/**
 * UI-152 — pointing at a changed file without re-rooting the canvas on it.
 *
 * Two rules carry the channel and each is a decision a reader would otherwise
 * meet as a surprise: which sources light it (the open row *and* the hovered
 * one, not the latest of the two), and which circles a file path lands on at
 * each grain the canvas can be drawn at.
 *
 * Run: node --experimental-strip-types --test scripts/change-highlight.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { holdersOf, litPaths, type Holder } from '../src/viewmodels/changeHighlight.ts';

const node = (id: string, file_path: string): Holder => ({ id, file_path });

/** One canvas at three grains at once — a mixed-grain picture, which is what
 *  `collapseGraph` actually produces once a folder is expanded. */
const NODES = [
  node('folder-src-stores', 'src/stores'),
  node('file-diff', 'src/stores/diff.ts'),
  node('entity-loadDiff', 'src/stores/diff.ts'),
  node('file-graph', 'src/stores/graph.ts'),
  node('folder-src-views', 'src/views'),
  node('ghost-console', ''),
  node('root', ''),
];

test('the open row and the hovered row are both lit', () => {
  // The comparison "does this file live near the one I opened" is made of
  // exactly this union. Letting the hover replace the open row would lose the
  // answer the reader asked for first.
  assert.deepEqual(litPaths('src/stores/diff.ts', 'src/stores/graph.ts').sort(), [
    'src/stores/diff.ts',
    'src/stores/graph.ts',
  ]);
});

test('hovering the row that is already open lights one thing, not two', () => {
  assert.deepEqual(litPaths('src/stores/diff.ts', 'src/stores/diff.ts'), ['src/stores/diff.ts']);
});

test('nothing open and nothing hovered lights nothing', () => {
  assert.deepEqual(litPaths(null, null), []);
  assert.equal(holdersOf([], NODES).size, 0);
});

test('a file lights its own circle, the entities in it, and the folder above', () => {
  // The three grains the canvas can be drawn at. A reader who narrows to
  // folders and points at a row must still be shown where it went, or the
  // channel goes silent at exactly the aggregation that needs it most.
  const lit = holdersOf(['src/stores/diff.ts'], NODES);
  assert.deepEqual([...lit].sort(), ['entity-loadDiff', 'file-diff', 'folder-src-stores']);
});

test('a sibling folder is not lit', () => {
  // `startsWith` on its own would be wrong the other way round too; the
  // separator is part of the test, in `pathClaims`.
  assert.equal(holdersOf(['src/stores/diff.ts'], NODES).has('folder-src-views'), false);
  assert.equal(
    holdersOf(['src/stores_old/diff.ts'], [node('f', 'src/stores')]).size,
    0,
  );
});

test('a ghost and the root rollup are never lit', () => {
  // Both spell their path `''`, which as a scope is the whole repository — a
  // circle that lights for every file is evidence about none of them.
  const lit = holdersOf(['src/stores/diff.ts'], NODES);
  assert.equal(lit.has('ghost-console'), false);
  assert.equal(lit.has('root'), false);
});

test('a temp worktree prefix on either side still meets the graph', () => {
  // The same normalization every diff lookup runs: a path that came back from
  // a base worktree carries the prefix, the graph's paths are repo-relative,
  // and a miss here is indistinguishable from a file the canvas does not draw.
  const prefixed = '/var/folders/x/mezz-diff-base-abc123/src/stores/diff.ts';
  assert.deepEqual([...holdersOf([prefixed], [node('file-diff', 'src/stores/diff.ts')])], ['file-diff']);
  assert.deepEqual([...holdersOf(['src/stores/diff.ts'], [node('file-diff', prefixed)])], ['file-diff']);
});

test('a file the canvas does not draw lights nothing rather than everything', () => {
  // The row's own agreement column says *why* — not analysed, not drawn — and
  // a channel that hides nothing has no other way to say it.
  assert.equal(holdersOf(['docs/agents/mcp-server.md'], NODES).size, 0);
});

test('two lit files are a union, not the last one to arrive', () => {
  const lit = holdersOf(['src/stores/diff.ts', 'src/views/App.svelte'], NODES);
  assert.equal(lit.has('file-diff'), true);
  assert.equal(lit.has('folder-src-views'), true);
});
