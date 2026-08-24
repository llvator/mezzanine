/**
 * UI-088 — the diff detail ladder, and UI-109 — the seed it starts from.
 *
 * Run: node --experimental-strip-types --test scripts/diff-levels.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  planDiffLevel, isDiffLevel, DIFF_LEVELS,
  splitEdits, isSeedFacet, SEED_FACETS,
  type LevelEdge, type EditKind, type DiffSeedFacet,
} from '../src/viewmodels/diffLevels.ts';

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

// --- UI-109: the seed split, and how it composes with the rungs ---

/**
 * The same graph, with the change split across both halves.
 *
 *   arrived ──changed──▶ helper      a new function calling untouched code
 *   touched ──old─────▶ neighbour    a function edited in place
 *   swapA   ──changed──▶ swapB       a call swapped between untouched code
 */
const SPLIT_LINKS: LevelEdge[] = [
  { src: 'arrived', tgt: 'helper', changed: true },
  { src: 'touched', tgt: 'neighbour', changed: false },
  { src: 'swapA', tgt: 'swapB', changed: true },
];
const SPLIT_ALL = new Set(['arrived', 'touched', 'helper', 'neighbour', 'swapA', 'swapB']);

/** The edits, already sided by `diffVerdict.editKind`. `swapA` / `swapB` are
 *  in neither half: their source never moved, so they are not edits at all. */
const SIDED = new Map([
  ['arrived', 'new'],
  ['touched', 'existing'],
] as const) as ReadonlyMap<string, EditKind>;

const plan = (facet: DiffSeedFacet, level: (typeof DIFF_LEVELS)[number]) => {
  const { seed, excluded } = splitEdits(facet, SIDED);
  return planDiffLevel(level, SPLIT_ALL, seed, SPLIT_LINKS, excluded);
};

test('all is the whole seed — the picture before the facet existed', () => {
  const { seed, excluded } = splitEdits('all', SIDED);
  assert.deepEqual(ids(seed), ['arrived', 'touched']);
  assert.deepEqual(ids(excluded), [], 'nothing is excluded when nothing was chosen');
  for (const level of DIFF_LEVELS) {
    assert.deepEqual(
      ids(plan('all', level).visible),
      ids(planDiffLevel(level, SPLIT_ALL, new Set(SIDED.keys()), SPLIT_LINKS).visible),
      `${level} moved when the facet was at all, and nothing should`,
    );
  }
});

test('a facet keeps its own half and hands back the other', () => {
  assert.deepEqual(ids(splitEdits('new', SIDED).seed), ['arrived']);
  assert.deepEqual(ids(splitEdits('new', SIDED).excluded), ['touched']);
  assert.deepEqual(ids(splitEdits('existing', SIDED).seed), ['touched']);
  assert.deepEqual(ids(splitEdits('existing', SIDED).excluded), ['arrived']);
});

test('new + neighbourhood draws what the new code plugs into', () => {
  // The composition the facet exists for, and the reason it narrows the SEED
  // rather than the drawn set: the rungs grow from what they are given, so a
  // seed of new entities recruits their context and keeps it.
  const p = plan('new', 'neighbourhood');
  assert.deepEqual(ids(p.visible), ['arrived', 'helper']);
  assert.equal(p.visible.has('touched'), false, 'the existing half stays out');
  assert.equal(p.visible.has('neighbour'), false, 'and so does what it sits next to');
});

test('existing + neighbourhood draws neither the new code nor its neighbours', () => {
  assert.deepEqual(ids(plan('existing', 'neighbourhood').visible), ['neighbour', 'touched']);
});

test('rewiring does not recruit the half the reader excluded (UI-109)', () => {
  // The hole the real-data replay found. `rewiring` scans every changed edge
  // and takes both ends — right for a swap, and fatal here: measured on this
  // repo, `existing` at this rung drew 1,571 of the 1,576 nodes `all` drew,
  // every one of the 688 new entities pulled back in by an edge that is only
  // changed because it touches one.
  const p = plan('existing', 'rewiring');
  assert.equal(p.visible.has('arrived'), false, 'the excluded half came back through an edge');
  assert.equal(p.visible.has('helper'), false, 'and brought its far end with it');
  assert.equal(p.visible.has('touched'), true);
});

test('a swap survives both facets, belonging to neither', () => {
  // Neither end is an edit — identical source, identical metrics — so the edge
  // is the only witness, and this rung is the only place it is ever drawn.
  // Suppressing it under a facet would lose it entirely rather than file it.
  for (const facet of ['all', 'new', 'existing'] as const) {
    const p = plan(facet, 'rewiring');
    assert.equal(p.visible.has('swapA') && p.visible.has('swapB'), true, `lost the swap under ${facet}`);
  }
});

test('a facet with no members empties the canvas rather than falling back', () => {
  // A commit that only adds files has no existing half. Quietly showing the
  // whole change instead would be the lie the ladder was rebuilt to stop:
  // a control that reads as filtering while filtering nothing.
  const onlyNew = new Map([['arrived', 'new']] as const) as ReadonlyMap<string, EditKind>;
  const { seed, excluded } = splitEdits('existing', onlyNew);
  assert.deepEqual(ids(planDiffLevel('edits', SPLIT_ALL, seed, SPLIT_LINKS, excluded).visible), []);
});

test('isSeedFacet guards the same contract isDiffLevel does', () => {
  for (const facet of SEED_FACETS) assert.ok(isSeedFacet(facet));
  for (const bad of ['All', 'edits', 'added', '', null, undefined, 0, {}]) {
    assert.equal(isSeedFacet(bad), false, `accepted ${JSON.stringify(bad)}`);
  }
});

// --- UI-112: which drawn nodes the rung recruited, and which are the edits ---

test('edits recruits nothing, so every drawn node is an edit', () => {
  const plan = planDiffLevel('edits', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.context), []);
});

test('rewiring calls the far end of a changed edge context, not an edit', () => {
  const plan = planDiffLevel('rewiring', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.visible), ['edited', 'helper']);
  assert.deepEqual(ids(plan.context), ['helper'], 'helper was never edited');
});

test('neighbourhood calls every hop it took context', () => {
  const plan = planDiffLevel('neighbourhood', ALL, EDITS, LINKS);
  assert.deepEqual(ids(plan.context), ['helper', 'neighbour']);
});

test('context and the seed partition what is drawn', () => {
  // The invariant the third opacity tier rests on: every drawn node is
  // weighted exactly once, so none can be left at a strength no control set.
  for (const level of DIFF_LEVELS) {
    const p = planDiffLevel(level, ALL, EDITS, LINKS);
    for (const id of p.context) {
      assert.ok(p.visible.has(id), `${level} put ${id} in context without drawing it`);
      assert.equal(EDITS.has(id), false, `${level} called the edit ${id} context`);
    }
    for (const id of p.visible) {
      assert.equal(
        EDITS.has(id) || p.context.has(id), true,
        `${level} drew ${id} as neither an edit nor context`,
      );
    }
  }
});

test('an unedited node the swap case recruits is context', () => {
  // `rewiring` draws both ends of a changed edge even when neither was
  // edited. Neither is the reader's change, and both read as such — this is
  // the one rung where the whole drawn set can be context.
  const p = planDiffLevel('rewiring', ALL, new Set(), LINKS);
  assert.deepEqual(ids(p.context), ['edited', 'helper']);
});

test('the excluded half is context when a rung recruits it back', () => {
  // `existing` + `neighbourhood`: `arrived` is an edit the reader excluded,
  // so it is not part of THIS seed. Drawn as a neighbour of `touched` it
  // would be — but the seed the picture is about is the existing half, and a
  // node reached by a hop is context whatever the diff says about it.
  const links: LevelEdge[] = [{ src: 'touched', tgt: 'arrived', changed: false }];
  const { seed, excluded } = splitEdits('existing', SIDED);
  const p = planDiffLevel('neighbourhood', SPLIT_ALL, seed, links, excluded);
  assert.deepEqual(ids(p.visible), ['arrived', 'touched']);
  assert.deepEqual(ids(p.context), ['arrived']);
});
