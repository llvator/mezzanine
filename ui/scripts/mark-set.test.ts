/**
 * Unit tests for the marked set — the multi-node pick that turns a file view
 * into an entity view.
 *
 * What is actually worth asserting here is the *level-independence*: a mark is
 * made on one aggregation level and spent on another, so every claim below is
 * some version of "the same file, drawn three different ways, is one mark".
 *
 *   npm run test:marks
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { livePaths, markPathOf, prunedMarks, toggleMarked } from '../src/viewmodels/markSet.ts';
import type { D3Node } from '../src/types/graph.ts';

/** Enough of a node to answer `markPathOf`. The rest of `D3Node` is inert
 *  here, which is the point of the function taking one field. */
function node(partial: Partial<D3Node>): D3Node {
  return {
    id: 'n', original_id: 'n', name: 'n', qualified_name: 'n',
    kind: 'function', kind_raw: 'Function', file_path: '', line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'Rust',
    ...partial,
  } as D3Node;
}

// ── what a click stands for ───────────────────────────────────────────────

test('an entity marks the file it was parsed from', () => {
  const fn = node({ kind_raw: 'Function', original_id: 'ui/src/a.ts::run', file_path: 'ui/src/a.ts' });
  assert.equal(markPathOf(fn), 'ui/src/a.ts');
});

test('a file rollup and an entity inside it are the same mark', () => {
  // The whole feature rests on this: the mark is made at File level and has to
  // still name something after the drill re-draws the canvas at Entity level.
  const rollup = node({ kind_raw: 'File', original_id: 'ui/src/a.ts', file_path: 'ui/src/a.ts' });
  const inside = node({ kind_raw: 'Class', original_id: 'ui/src/a.ts::A', file_path: 'ui/src/a.ts' });
  assert.equal(markPathOf(rollup), markPathOf(inside));
});

test('a module rollup marks its directory', () => {
  const mod = node({ kind_raw: 'Module', original_id: 'ui/src', file_path: 'ui/src' });
  assert.equal(markPathOf(mod), 'ui/src');
});

test('a ghost cannot be marked', () => {
  // External and stdlib references are not code in this repo, so there is no
  // scope to narrow to — and their `file_path` is the empty string, which as a
  // scope would mean the whole repository.
  const ghost = node({ kind_raw: 'Function', name: 'print', file_path: '', tags: ['ghost'] });
  assert.equal(markPathOf(ghost), null);
});

test('the root module cannot be marked', () => {
  // Its path is '', which as a scope is the entire repo: drilling into it is a
  // no-op dressed as a narrowing.
  const root = node({ kind_raw: 'Module', original_id: '', file_path: '' });
  assert.equal(markPathOf(root), null);
});

// ── the set ───────────────────────────────────────────────────────────────

test('marking twice unmarks', () => {
  const once = toggleMarked(new Set<string>(), 'a.ts');
  assert.deepEqual([...once], ['a.ts']);
  assert.deepEqual([...toggleMarked(once, 'a.ts')], []);
});

test('marking leaves the other marks alone', () => {
  const two = toggleMarked(toggleMarked(new Set<string>(), 'a.ts'), 'b.ts');
  assert.deepEqual([...two].sort(), ['a.ts', 'b.ts']);
});

test('toggling does not mutate the set it was given', () => {
  // The store hands its own set in; mutating it would move the state without
  // waking a single subscriber.
  const before = new Set(['a.ts']);
  toggleMarked(before, 'b.ts');
  assert.deepEqual([...before], ['a.ts']);
});

// ── surviving a scope change ──────────────────────────────────────────────

test('live paths carry every file and the directory holding it', () => {
  const live = livePaths([
    node({ file_path: 'ui/src/a.ts' }),
    node({ file_path: 'ui/src/b.ts' }),
    node({ file_path: 'top.ts' }),
  ]);
  assert.deepEqual([...live].sort(), ['', 'top.ts', 'ui/src', 'ui/src/a.ts', 'ui/src/b.ts']);
});

test('a ghost contributes no live path', () => {
  assert.deepEqual([...livePaths([node({ file_path: '', tags: ['ghost'] })])], []);
});

test('a mark on a directory survives a drop to entity level', () => {
  // Made at Module level, pruned against an entity graph where no node's
  // file_path is ever a directory. Without the parent-directory rule this is
  // exactly the mark that would vanish on the way to being spent.
  const marks = new Set(['ui/src']);
  const live = livePaths([node({ file_path: 'ui/src/a.ts' })]);
  assert.deepEqual([...prunedMarks(marks, live)], ['ui/src']);
});

test('a mark on a path the scope no longer holds is dropped', () => {
  const live = livePaths([node({ file_path: 'ui/src/a.ts' })]);
  assert.deepEqual([...prunedMarks(new Set(['gone/x.ts']), live)], []);
});

test('pruning returns the same set when nothing died', () => {
  // Identity, not equality: the store hands the result straight back, and a
  // fresh set on every scope change would repaint the canvas for no change.
  const marks = new Set(['ui/src/a.ts']);
  const live = livePaths([node({ file_path: 'ui/src/a.ts' })]);
  assert.equal(prunedMarks(marks, live), marks);
});
