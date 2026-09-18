/**
 * Unit tests for the blank-canvas explanation (UI-064).
 *
 * The state under test is the one that used to be silent: nodes were built,
 * the DOM held them, `Shown: 0` was in the stats bar, and the canvas was
 * white with no card saying why. What makes it worth its own module is the
 * gap between *built* and *seen* — a dimmed node is built, and is only seen
 * when the dim leaves something on screen. `diffDimOpacity` defaults to 0,
 * where dimmed means `display: none`, so the two counts differ in exactly
 * the case worth reporting.
 *
 *   npm run test:empty
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { blankCanvasReason, seenNodeCount } from '../src/viewmodels/emptyCanvas.ts';
import type { DisplayPlan } from '../src/viewmodels/displayPlan.ts';

function plan(over: Partial<DisplayPlan> = {}): DisplayPlan {
  return {
    mode: 'force',
    visibleNodeIds: new Set(),
    visibleLinkKeys: new Set(),
    dimmedLinkKeys: new Set(),
    selectedId: null,
    treePositions: new Map(),
    nodeDistances: null,
    searchMatched: new Set(),
    searchNeighbors: new Set(),
    demotedHubIds: new Set(),
    dimmedNodeIds: new Set(),
    dimOpacity: 0,
    overflow: null,
    ...over,
  } as DisplayPlan;
}

const ids = (n: number) => new Set(Array.from({ length: n }, (_, i) => `n${i}`));

// ── seen vs built ─────────────────────────────────────────────────────────

test('a dimmed node at zero opacity is not seen', () => {
  // This is `display: none` in the View, and it is the diff filters' default.
  assert.equal(seenNodeCount(plan({ dimmedNodeIds: ids(40), dimOpacity: 0 })), 0);
});

test('a dimmed node at a visible opacity is seen', () => {
  assert.equal(seenNodeCount(plan({ dimmedNodeIds: ids(40), dimOpacity: 0.15 })), 40);
});

test('visible and dimmed both count when the dim shows', () => {
  const p = plan({ visibleNodeIds: ids(3), dimmedNodeIds: new Set(['x', 'y']), dimOpacity: 0.2 });
  assert.equal(seenNodeCount(p), 5);
});

// ── when to explain ───────────────────────────────────────────────────────

test('the state that used to be silent now reports itself', () => {
  const p = plan({ dimmedNodeIds: ids(68), dimOpacity: 0 });
  assert.deepEqual(blankCanvasReason(p, 68), { built: 68 });
});

test('a canvas with something on it says nothing', () => {
  assert.equal(blankCanvasReason(plan({ visibleNodeIds: ids(15) }), 68), null);
});

test('one visible node is enough to stay quiet', () => {
  assert.equal(blankCanvasReason(plan({ visibleNodeIds: ids(1) }), 68), null);
});

test('nudging the dim off zero is enough to stay quiet', () => {
  const p = plan({ dimmedNodeIds: ids(68), dimOpacity: 0.15 });
  assert.equal(blankCanvasReason(p, 68), null);
});

// ── whose card is it ──────────────────────────────────────────────────────

test('no scope selected belongs to the empty-state card', () => {
  // Nothing built means nothing was asked for. Explaining a filter here
  // would be answering a question the user has not reached yet.
  assert.equal(blankCanvasReason(plan(), 0), null);
});

test('an overflowing plan belongs to the overflow card', () => {
  // UI-062's card already names both numbers and offers remedies. Stacking
  // a second explanation on the first is its own kind of unhelpful.
  const p = plan({ overflow: { drawn: 5000, ceiling: 400 } });
  assert.equal(blankCanvasReason(p, 5000), null);
});

test('a built count of one still explains itself', () => {
  // The card renders "1 node" rather than "1 nodes", so the boundary is
  // worth pinning even though the number is small.
  assert.deepEqual(blankCanvasReason(plan({ dimmedNodeIds: ids(1) }), 1), { built: 1 });
});
