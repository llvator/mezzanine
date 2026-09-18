/**
 * Unit tests for the spec pane's highlight channel — the leg that lights code
 * up without hiding anything.
 *
 * What is worth asserting is the arithmetic the filter and the highlight
 * disagree about. They share `claimedPathsForAll` and `pathsClaim`, both
 * already pinned elsewhere; what is new here is that two sources combine
 * rather than replace, and that "claims nothing" is a *different* answer for a
 * highlight than it is for a filter — a distinction the filter's `null` vs
 * `[]` exists to preserve and this one deliberately drops.
 *
 *   npm run test:spechighlight
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  claimedNodeIds, highlightSources, togglePin, type Claimable,
} from '../src/viewmodels/specHighlight.ts';

function node(id: string, file_path: string): Claimable {
  return { id, file_path };
}

// ── what is lighting the canvas ───────────────────────────────────────────

test('nothing pinned and nothing hovered lights nothing', () => {
  assert.deepEqual(highlightSources(new Set(), null), []);
});

test('the hover is a source on its own', () => {
  assert.deepEqual(highlightSources(new Set(), 'f.parse'), ['f.parse']);
});

test('a pin and a hover light together rather than replacing each other', () => {
  // The gesture the pin exists for: hold one entity lit, then sweep the pane
  // to see which of the others touch the same code.
  const sources = highlightSources(new Set(['f.parse']), 'f.render');
  assert.deepEqual(sources.sort(), ['f.parse', 'f.render']);
});

test('hovering something already pinned does not double it', () => {
  assert.deepEqual(highlightSources(new Set(['f.parse']), 'f.parse'), ['f.parse']);
});

test('pins survive the pointer leaving', () => {
  assert.deepEqual(highlightSources(new Set(['f.parse']), null), ['f.parse']);
});

// ── which code that lights ────────────────────────────────────────────────

const NODES: Claimable[] = [
  node('a', 'src/parser/mod.rs'),
  node('b', 'src/parser/rust/inference.rs'),
  node('c', 'src/analyzer/mod.rs'),
  node('d', 'src/parser_old/legacy.rs'),
];

test('a declared folder lights every file under it', () => {
  const lit = claimedNodeIds(['src/parser'], NODES);
  assert.deepEqual([...lit].sort(), ['a', 'b']);
});

test('a sibling folder sharing a prefix is not lit', () => {
  // `src/parser` must not claim `src/parser_old/` — the separator is part of
  // the test, and this is the case that made it so (see `pathClaims`).
  const lit = claimedNodeIds(['src/parser'], NODES);
  assert.equal(lit.has('d'), false);
});

test('two sources light the union of what they claim', () => {
  const lit = claimedNodeIds(['src/parser', 'src/analyzer'], NODES);
  assert.deepEqual([...lit].sort(), ['a', 'b', 'c']);
});

test('an exact file path lights that file alone', () => {
  const lit = claimedNodeIds(['src/parser/rust/inference.rs'], NODES);
  assert.deepEqual([...lit], ['b']);
});

test('a ref written with ./ and a trailing slash still lights', () => {
  // Authors write both forms; normalization runs on both sides or the whole
  // spec reports as drift while looking healthy.
  const lit = claimedNodeIds(['./src/parser/'], NODES);
  assert.deepEqual([...lit].sort(), ['a', 'b']);
});

test('no highlight and an empty claim are the same nothing', () => {
  // The one place this differs from the filter on purpose. There, `null` is
  // "no filter" and `[]` is "declares no code, so empty the canvas" — a
  // distinction that only means something to a channel that can hide things.
  assert.equal(claimedNodeIds(null, NODES).size, 0);
  assert.equal(claimedNodeIds([], NODES).size, 0);
});

test('a ghost node with no file path is never lit', () => {
  const ghosts = [node('std', '')];
  assert.equal(claimedNodeIds(['src/parser'], ghosts).size, 0);
});

test('a rollup is lit by the folder it stands for', () => {
  // `collapseGraph` mints a fresh id for a File or Module circle, so matching
  // on ids would light nothing at any level above Entity. `file_path` is what
  // survives the collapse, which is why the answer is computed from it.
  const rollup = [node('collapsed::src/parser', 'src/parser/mod.rs')];
  assert.deepEqual([...claimedNodeIds(['src/parser'], rollup)], ['collapsed::src/parser']);
});

// ── pinning ───────────────────────────────────────────────────────────────

test('a pin toggles on and back off', () => {
  const once = togglePin(new Set(), 'f.parse');
  assert.deepEqual([...once], ['f.parse']);
  assert.deepEqual([...togglePin(once, 'f.parse')], []);
});

test('pinning a second entity keeps the first', () => {
  const both = togglePin(new Set(['f.parse']), 'f.render');
  assert.deepEqual([...both].sort(), ['f.parse', 'f.render']);
});

test('toggling never mutates the set it was given', () => {
  // The store holds this set and Svelte compares by identity; mutating in
  // place is the failure where the canvas stops repainting on the second pin.
  const before = new Set(['f.parse']);
  togglePin(before, 'f.render');
  assert.deepEqual([...before], ['f.parse']);
});
