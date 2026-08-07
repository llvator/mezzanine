/**
 * UI-088 — the diff detail ladder.
 *
 * Run: node --experimental-strip-types --test scripts/diff-levels.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { planDiffLevel, isDiffLevel, DIFF_LEVELS, type LevelEdge } from '../src/viewmodels/diffLevels.ts';

const ids = (s: ReadonlySet<string>) => [...s].sort();

/**
 * The shape the ladder exists for.
 *
 *   edited ──changed──▶ helper        helper was never edited
 *   edited ──old─────▶ neighbour      untouched wiring
 *   other  ──old─────▶ neighbour      nothing to do with the change
 */
const LINKS: LevelEdge[] = [
  { src: 'edited', tgt: 'helper', changed: true },
  { src: 'edited', tgt: 'neighbour', changed: false },
  { src: 'other', tgt: 'neighbour', changed: false },
];
const ALL = new Set(['edited', 'helper', 'neighbour', 'other']);
const EDITS = new Set(['edited']);

test('edits draws only what was edited', () => {
  const plan = planDiffLevel('edits', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.visible), ['edited']);
  assert.deepEqual(ids(plan.dimmed), ['helper', 'neighbour', 'other']);
  assert.equal(plan.changedEdgesOnly, true);
});

test('rewiring adds the far end of a changed edge, even unedited', () => {
  const plan = planDiffLevel('rewiring', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.visible), ['edited', 'helper']);
  assert.equal(
    plan.visible.has('neighbour'), false,
    'an untouched edge earns its far end nothing',
  );
  assert.equal(plan.changedEdgesOnly, true, 'still only changed edges get a line');
});

test('neighbourhood adds every direct neighbour and lets untouched wiring draw', () => {
  const plan = planDiffLevel('neighbourhood', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.visible), ['edited', 'helper', 'neighbour']);
  assert.equal(plan.visible.has('other'), false, 'two hops away is not one hop away');
  assert.equal(plan.changedEdgesOnly, false);
});

test('the ladder only ever widens', () => {
  const seen = DIFF_LEVELS.map((l) => planDiffLevel(l, ALL, EDITS, LINKS).visible);
  for (let i = 1; i < seen.length; i++) {
    for (const id of seen[i - 1]) {
      assert.ok(seen[i].has(id), `${id} vanished moving to ${DIFF_LEVELS[i]}`);
    }
  }
});

test('a node another filter hid cannot be recruited back', () => {
  // `helper` failed a kind or language filter, so it is not a candidate. A
  // changed edge pointing at it must not override that — the diff narrows the
  // view, it does not reopen it.
  const candidates = new Set(['edited', 'neighbour', 'other']);
  for (const level of DIFF_LEVELS) {
    const plan = planDiffLevel(level, candidates, EDITS, LINKS);
    assert.equal(plan.visible.has('helper'), false, `${level} recruited a filtered node`);
  }
});

test('an edit that is not a candidate is not drawn', () => {
  const plan = planDiffLevel('edits', new Set(['other']), EDITS, LINKS);
  assert.deepEqual(ids(plan.visible), []);
});

test('neighbourhood measures hops from the edits, not from what it just added', () => {
  // edited → helper → far. `far` is two hops out and must stay off, or the
  // rung walks the whole connected component instead of one ring.
  const links: LevelEdge[] = [
    { src: 'edited', tgt: 'helper', changed: false },
    { src: 'helper', tgt: 'far', changed: false },
  ];
  const plan = planDiffLevel('neighbourhood', new Set(['edited', 'helper', 'far']), EDITS, links);
  assert.deepEqual(ids(plan.visible), ['edited', 'helper']);
});

test('incoming edges count as neighbours too', () => {
  const links: LevelEdge[] = [{ src: 'caller', tgt: 'edited', changed: false }];
  const plan = planDiffLevel('neighbourhood', new Set(['caller', 'edited']), EDITS, links);
  assert.deepEqual(ids(plan.visible), ['caller', 'edited']);
});

test('rewiring reaches a changed edge that points INTO an edit', () => {
  const links: LevelEdge[] = [{ src: 'newCaller', tgt: 'edited', changed: true }];
  const plan = planDiffLevel('rewiring', new Set(['newCaller', 'edited']), EDITS, links);
  assert.deepEqual(ids(plan.visible), ['edited', 'newCaller']);
});

test('every candidate is either visible or dimmed, never lost', () => {
  for (const level of DIFF_LEVELS) {
    const plan = planDiffLevel(level, ALL, EDITS, LINKS);
    assert.equal(
      plan.visible.size + plan.dimmed.size, ALL.size,
      `${level} dropped a candidate on the floor`,
    );
    for (const id of plan.visible) {
      assert.equal(plan.dimmed.has(id), false, `${id} is both visible and dimmed`);
    }
  }
});

test('rewiring shows a changed edge whose ends were never edited', () => {
  // The swap case: an entity drops one call and adds another. Its source is
  // byte-for-byte identical and every metric holds, so the diff calls it an
  // *impact* — which means it is not in `edits` and no node filter can ever
  // reach it. The edge is the only witness, and this rung is where it shows.
  const plan = planDiffLevel('rewiring', ALL, new Set(), LINKS);
  assert.deepEqual(ids(plan.visible), ['edited', 'helper']);
});

test('nothing changed at all means nothing drawn below neighbourhood', () => {
  const untouched = LINKS.map((l) => ({ ...l, changed: false }));
  const plan = planDiffLevel('rewiring', ALL, new Set(), untouched);
  assert.deepEqual(ids(plan.visible), [], 'no edits, no changed edges, no canvas');
});

test('isDiffLevel guards the message contract', () => {
  for (const level of DIFF_LEVELS) assert.ok(isDiffLevel(level));
  for (const bad of ['Edits', 'core', '', null, undefined, 0, {}]) {
    assert.equal(isDiffLevel(bad), false, `accepted ${JSON.stringify(bad)}`);
  }
});
