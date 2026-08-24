/**
 * Unit tests for picking rows out of a result list.
 *
 * Same zero-dependency setup as the sibling suites — Node's built-in runner
 * plus type stripping. `rowPicks.ts` imports nothing at runtime, which is
 * what makes it testable here; the components that call it reach for stores
 * and cannot be loaded by the runner.
 *
 *   npm run test:picks
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { rowsForClick, pickedHighlight } from '../src/utils/rowPicks.ts';

const ROWS = ['a', 'b', 'c', 'd', 'e'];

// --- rowsForClick: which rows a click applies to ---

test('a plain click applies to the clicked row alone', () => {
  assert.deepEqual(rowsForClick(ROWS, 0, 3, false), ['d']);
});

test('shift extends from the anchor, in either direction', () => {
  assert.deepEqual(rowsForClick(ROWS, 1, 3, true), ['b', 'c', 'd']);
  assert.deepEqual(rowsForClick(ROWS, 3, 1, true), ['b', 'c', 'd']);
});

test('shift on the anchor itself is just that row', () => {
  assert.deepEqual(rowsForClick(ROWS, 2, 2, true), ['c']);
});

test('shift with no anchor yet behaves as a plain click', () => {
  assert.deepEqual(rowsForClick(ROWS, -1, 2, true), ['c']);
});

test('an anchor left over from a longer list does not address this one', () => {
  // The result set shrank under the reader — slicing from 9 would silently
  // take the whole list.
  assert.deepEqual(rowsForClick(ROWS, 9, 1, true), ['b']);
});

test('an out-of-range click applies to nothing', () => {
  assert.deepEqual(rowsForClick(ROWS, 0, 5, false), []);
  assert.deepEqual(rowsForClick(ROWS, 0, -1, true), []);
});

// --- pickedHighlight: what the canvas marks ---

test('no picks means every match is marked', () => {
  const marked = pickedHighlight(['a', 'b', 'c'], new Set());
  assert.deepEqual([...marked].sort(), ['a', 'b', 'c']);
});

test('picks narrow the marking to themselves', () => {
  const marked = pickedHighlight(['a', 'b', 'c'], new Set(['a', 'c']));
  assert.deepEqual([...marked].sort(), ['a', 'c']);
});

test('a pick the view no longer holds is not marked', () => {
  // The canvas moved under the pick — `b` is picked but is not a match any
  // more, so it marks nothing rather than resurrecting.
  const marked = pickedHighlight(['a', 'c'], new Set(['a', 'b']));
  assert.deepEqual([...marked], ['a']);
});

test('picks that all fall out of view mark nothing, rather than everything', () => {
  // The regression this guards: falling back to "all matches" the moment
  // the intersection empties lights up the whole result set exactly when
  // the reader's own choice went out of view.
  const marked = pickedHighlight(['a', 'b'], new Set(['z']));
  assert.equal(marked.size, 0);
});
