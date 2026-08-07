/**
 * UI-090 — when Entity level would redraw the File-level picture.
 *
 * The predicate is deliberately about the *graph*, not about a language, so
 * every test below states its case in nodes and file paths. If a test here
 * ever needs the word "Markdown" to make sense, the implementation has
 * started hardcoding what it was written to derive.
 *
 *   npm run test:levelshape
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { everyFileIsOneEntity } from '../src/viewmodels/collapseGraph.ts';
import type { D3Node } from '../src/types/graph.ts';

/** Enough of a node to answer the predicate; the rest is inert here. */
function node(file_path: string, name = 'n'): D3Node {
  return {
    id: `${file_path}:${name}`, original_id: name, name, qualified_name: name,
    kind: 'note', kind_raw: 'Note', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'Markdown',
  } as D3Node;
}

test('one node per file is the redundant shape', () => {
  const nodes = [node('docs/a.md', 'A'), node('docs/b.md', 'B'), node('guide/c.md', 'C')];
  assert.equal(everyFileIsOneEntity(nodes), true);
});

test('a single file with two entities is not', () => {
  // One counter-example anywhere in the graph is enough: Entity level then
  // shows something File level does not.
  const nodes = [node('src/a.rs', 'one'), node('src/a.rs', 'two'), node('src/b.rs', 'three')];
  assert.equal(everyFileIsOneEntity(nodes), false);
});

test('an empty graph is not the redundant shape', () => {
  // Nothing is drawn at either level, so there is no redundancy to report —
  // and an empty canvas has its own message. Saying "same as File" over a
  // blank canvas would be noise on top of nothing.
  assert.equal(everyFileIsOneEntity([]), false);
});

test('a one-node graph is the redundant shape', () => {
  assert.equal(everyFileIsOneEntity([node('README.md', 'Readme')]), true);
});

test('a code graph that happens to be one-per-file counts too', () => {
  // The predicate is about the graph on screen, not about the language: a
  // filtered Rust view with one entity left per file gets the same answer,
  // and that answer is correct — Entity really would redraw File.
  const nodes = [node('src/a.rs', 'A'), node('src/b.rs', 'B')];
  assert.equal(everyFileIsOneEntity(nodes), true);
});
