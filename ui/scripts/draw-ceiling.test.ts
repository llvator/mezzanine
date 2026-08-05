/**
 * Unit tests for the render gate (UI-061).
 *
 * Same zero-dependency setup as the sibling suites — Node's built-in runner
 * plus type stripping. `drawCeiling.ts` is importable here precisely because
 * its only import is a type, which stripping erases; that isolation is the
 * reason the decision lives in its own module rather than inside the
 * 1,000-line `displayPlan.ts`.
 *
 * What's worth asserting is the property the old gate got wrong: that the
 * decision is a function of the plan's own drawn count and nothing else, so
 * anything upstream that shrinks that count can lift it. A click-through can
 * show you the overlay disappearing; it cannot show you that the number
 * driving it is the one filters actually move.
 *
 *   npm run test:ceiling
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { gateByDrawCeiling, drawnIdsOf, DRAW_CEILING } from '../src/viewmodels/drawCeiling.ts';
import type { DisplayPlan } from '../src/viewmodels/displayPlan.ts';

/** A plan carrying `n` visible nodes and enough live state that we can tell
 *  pass-through from gating by looking at any of it. */
function planWith(n: number, extra: Partial<DisplayPlan> = {}): DisplayPlan {
  const ids = new Set(Array.from({ length: n }, (_, i) => `n${i}`));
  return {
    mode: 'force',
    visibleNodeIds: ids,
    visibleLinkKeys: new Set(['n0->n1|Calls']),
    selectedId: 'n0',
    treePositions: new Map(),
    nodeDistances: new Map([['n0', 0]]),
    searchMatched: new Set(['n0']),
    searchNeighbors: new Set(['n1']),
    dimmedNodeIds: new Set(['n1']),
    dimOpacity: 0.3,
    overflow: null,
    ...extra,
  };
}

test('a plan under the ceiling passes through untouched', () => {
  const plan = planWith(10);
  const gated = gateByDrawCeiling(plan, 100);
  assert.equal(gated, plan, 'expected the identical object, not a copy');
  assert.equal(gated.overflow, null);
});

test('the ceiling is inclusive — exactly at the limit still draws', () => {
  const gated = gateByDrawCeiling(planWith(100), 100);
  assert.equal(gated.overflow, null);
  assert.equal(gated.visibleNodeIds.size, 100);
});

test('one node over the ceiling gates', () => {
  const gated = gateByDrawCeiling(planWith(101), 100);
  assert.deepEqual(gated.overflow, { drawn: 101, ceiling: 100 });
});

test('a gated plan draws nothing at all', () => {
  const gated = gateByDrawCeiling(planWith(500), 100);
  assert.equal(gated.visibleNodeIds.size, 0);
  assert.equal(gated.visibleLinkKeys.size, 0);
  assert.equal(gated.dimmedNodeIds.size, 0, 'dimmed nodes are still drawn nodes');
  assert.equal(gated.treePositions.size, 0);
  assert.equal(gated.nodeDistances, null);
  assert.equal(gated.selectedId, null);
  assert.equal(gated.dimOpacity, 0);
});

test('gating does not mutate the plan it was handed', () => {
  const plan = planWith(500);
  gateByDrawCeiling(plan, 100);
  assert.equal(plan.visibleNodeIds.size, 500);
  assert.equal(plan.overflow, null);
});

test('overflow reports the count that was refused, not the ceiling', () => {
  // The overlay quotes `drawn` back to the user, so it has to be the real
  // figure — the whole point is that it moves as filters change.
  const gated = gateByDrawCeiling(planWith(4321), 2000);
  assert.equal(gated.overflow?.drawn, 4321);
});

test('tree mode is gated on the same count as force mode', () => {
  // Tree mode pins nodes to computed positions, but it is the same number
  // of DOM elements. Nothing about the layout makes them cheaper.
  const gated = gateByDrawCeiling(planWith(500, { mode: 'tree' }), 100);
  assert.ok(gated.overflow, 'tree-mode plans must be gated too');
  assert.equal(gated.mode, 'tree', 'gating must not change the mode');
});

test('a filter that drops the count below the ceiling lifts the gate', () => {
  // The regression this whole ticket exists for. Upstream, `compute()`
  // applies every filter before we see the plan; here we stand in for that
  // by gating the wide plan and then the narrowed one. The gate must be a
  // pure function of the count it is handed, with no memory of having
  // refused before — otherwise narrowing the view would not recover.
  const wide = gateByDrawCeiling(planWith(5000), 2000);
  assert.ok(wide.overflow, 'the unfiltered plan overflows');

  const narrowed = gateByDrawCeiling(planWith(120), 2000);
  assert.equal(narrowed.overflow, null, 'filtering down must clear the gate');
  assert.equal(narrowed.visibleNodeIds.size, 120);
});

test('an empty plan is not an overflowing one', () => {
  // `compute()` returns an empty plan for an empty graph, and the overlay
  // distinguishes "nothing selected" from "too much selected". A zero count
  // must never produce an overflow marker.
  const gated = gateByDrawCeiling(planWith(0), 2000);
  assert.equal(gated.overflow, null);
});

test('dimmed nodes count as drawn — they are on screen, just faint', () => {
  // The case the first implementation missed, caught by the ui-061 probe on
  // a diff-filtered view: 50 visible, ~10,300 dimmed, and the gate scored it
  // as 50. Dimming is how the diff and search show context; those nodes cost
  // a DOM element and a simulation body exactly like any other.
  const plan = planWith(50, {
    dimmedNodeIds: new Set(Array.from({ length: 5000 }, (_, i) => `d${i}`)),
    dimOpacity: 0.3,
  });
  assert.equal(drawnIdsOf(plan).size, 5050);
  const gated = gateByDrawCeiling(plan, 2000);
  assert.ok(gated.overflow, 'a view drawing 5,050 nodes must be gated');
  assert.equal(gated.overflow?.drawn, 5050);
});

test('a dim opacity of zero means the dimmed set is hidden, not drawn', () => {
  // `applyDisplayPlan` hides rather than fades when the slider is at 0, so
  // those nodes genuinely are not on screen and must not be charged for.
  const plan = planWith(50, {
    dimmedNodeIds: new Set(Array.from({ length: 5000 }, (_, i) => `d${i}`)),
    dimOpacity: 0,
  });
  assert.equal(drawnIdsOf(plan).size, 50);
  assert.equal(gateByDrawCeiling(plan, 2000).overflow, null);
});

test('drawnIdsOf does not double-count a node that is both visible and dimmed', () => {
  const plan = planWith(10, { dimmedNodeIds: new Set(['n0', 'n1']), dimOpacity: 0.5 });
  assert.equal(drawnIdsOf(plan).size, 10);
});

test('the shipped ceiling leaves headroom over the auto-level budget', () => {
  // RENDER_BUDGET (400) is the target `pickLevel` collapses toward, not a
  // limit. A ceiling at or near it would fire on views that legitimately
  // land above the target — a hand-pinned level, ghosts switched on, or the
  // param and class-field nodes injected after collapse.
  assert.ok(DRAW_CEILING >= 400 * 2, `expected headroom over RENDER_BUDGET, got ${DRAW_CEILING}`);
});
