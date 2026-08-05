/**
 * Unit tests for the `Relationships` size channel's domain (`linkDegrees`).
 *
 * What makes this worth a suite rather than a look at the canvas: the count
 * is read from links whose ends change shape underneath it. D3's force
 * simulation rewrites `source`/`target` from id strings to node objects in
 * place on its first tick, so a version that only handled strings would draw
 * a correct legend at mount and every circle at the minimum radius a frame
 * later — a bug that looks like a layout settling, not like a wrong number.
 *
 * Same zero-dependency setup as the sibling suites, and the same
 * extensionless-import trap: `linkDegrees` is its own module precisely so
 * this file can reach it without pulling `stores/quality` in behind it.
 *
 *   npm run test:degree
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { linkDegrees } from '../src/viewmodels/linkDegrees.ts';
import type { D3Link, D3Node } from '../src/types/graph.ts';

/** A link by ids — the shape the graph arrives in, before the simulation. */
function link(source: string | D3Node, target: string | D3Node): D3Link {
  return { source, target, kind: 'calls', kind_raw: 'Calls', incoming_kind: 'called by', order: null };
}

/** Only the field `linkDegrees` reads; the simulation replaces the whole
 *  node object, so the test does too. */
const node = (id: string) => ({ id } as D3Node);

test('a node is counted once per incident link, either direction', () => {
  const deg = linkDegrees([link('a', 'b'), link('c', 'a')]);
  assert.equal(deg.get('a'), 2);
  assert.equal(deg.get('b'), 1);
  assert.equal(deg.get('c'), 1);
});

test('link ends read the same after the simulation swaps them for nodes', () => {
  // The state one tick after mount. Same graph as above, same answer.
  const deg = linkDegrees([link(node('a'), node('b')), link(node('c'), node('a'))]);
  assert.equal(deg.get('a'), 2);
  assert.equal(deg.get('b'), 1);
});

test('a half-swapped link still counts both ends', () => {
  const deg = linkDegrees([link('a', node('b'))]);
  assert.equal(deg.get('a'), 1);
  assert.equal(deg.get('b'), 1);
});

test('a self-link counts once, not twice', () => {
  // Recursion is not a connection to anything else, and the channel is
  // read as "how connected to others".
  assert.equal(linkDegrees([link('a', 'a')]).get('a'), 1);
});

test('parallel links between one pair each count', () => {
  // At entity level a call and a type-use are two relationships; at scope
  // level `collapseGraph` has already merged them into one weighted link.
  const deg = linkDegrees([link('a', 'b'), link('a', 'b')]);
  assert.equal(deg.get('a'), 2);
  assert.equal(deg.get('b'), 2);
});

test('an isolated node is absent, which callers read as zero', () => {
  // Not "no data": zero relationships is a fact about the node, and it
  // should draw at the smallest radius rather than at the no-data one.
  const deg = linkDegrees([link('a', 'b')]);
  assert.equal(deg.get('lonely'), undefined);
  assert.equal(deg.get('lonely') ?? 0, 0);
});

test('an empty graph counts nothing and does not throw', () => {
  assert.equal(linkDegrees([]).size, 0);
});
