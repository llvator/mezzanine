/**
 * UI-104 — what a diff's *silence* about a file means.
 *
 * The rule under test is one branch: a file the diff never mentioned is
 * interesting when the diff was looking at this tree, and out of scope when it
 * was not. Getting it wrong in the second case is what made the whole ladder
 * look broken — every node younger than the compared head commit landed in the
 * `edits` seed, and no rung can remove what is already in the seed.
 *
 * Run: node --experimental-strip-types --test scripts/diff-verdict.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  changeOf, editKind, isEdit, unknownVerdict, headIsWorkingTree,
  type DiffFacts, type VerdictNode,
} from '../src/viewmodels/diffVerdict.ts';
import type { ChangeStatus } from '../src/stores/diff.ts';
import type { ScopeChange } from '../src/viewmodels/diffRollup.ts';

/**
 * A diff that looked at two files and reported one edited function in one of
 * them, one function it added and one it deleted. `quiet.ts` is in the tree and
 * unchanged; `young.ts` is not in the tree at all — the file the two heads
 * disagree about.
 */
function facts(headIsWorking: boolean): DiffFacts {
  const statuses = new Map<string, ChangeStatus>([
    ['src/edited.ts:10:touched', 'modified'],
    ['src/edited.ts:40:rippled', 'modified'],
    ['src/edited.ts:70:arrived', 'added'],
    ['src/edited.ts:90:departed', 'removed'],
  ]);
  const sourceChanged = new Map<string, boolean>([
    ['src/edited.ts:10:touched', true],
    // Only its fan-in moved because something nearby changed.
    ['src/edited.ts:40:rippled', false],
    ['src/edited.ts:70:arrived', true],
    ['src/edited.ts:90:departed', true],
  ]);
  const scopes = new Map<string, ScopeChange>([
    ['src/edited.ts', { status: 'modified', sourceChanged: true }],
    ['src/quiet.ts', { status: 'unchanged', sourceChanged: false }],
    ['src/fresh.ts', { status: 'added', sourceChanged: true }],
    ['src', { status: 'modified', sourceChanged: true }],
  ]);
  return { statuses, sourceChanged, scopes, headIsWorking };
}

const node = (original_id: string, file_path: string): VerdictNode =>
  ({ original_id, file_path });

const TOUCHED = node('src/edited.ts:10:touched', 'src/edited.ts');
const RIPPLED = node('src/edited.ts:40:rippled', 'src/edited.ts');
const ARRIVED = node('src/edited.ts:70:arrived', 'src/edited.ts');
const DEPARTED = node('src/edited.ts:90:departed', 'src/edited.ts');
/** A collapsed File node for a file that is new in its entirety. */
const FRESH_FILE = node('src/fresh.ts', 'src/fresh.ts');
/** In a file the diff looked at, but not itself reported — a Parameter, say. */
const SKIPPED = node('src/quiet.ts:3:name::param::x', 'src/quiet.ts');
/** In a file the diff never saw. */
const YOUNG = node('src/young.ts:1:brandNew', 'src/young.ts');
/** Ghosts carry no file at all. */
const GHOST = node('println', '');

test('to_ref says which tree the head was', () => {
  assert.equal(headIsWorkingTree('working'), true);
  assert.equal(headIsWorkingTree('b7d8219'), false);
  assert.equal(headIsWorkingTree(undefined), false, 'no diff loaded is not the working tree');
  assert.equal(headIsWorkingTree(null), false);
});

test('an entity id beats the scope rollup it sits in', () => {
  // Both would answer; the entity is the more specific claim.
  assert.deepEqual(changeOf(RIPPLED, facts(true)), { status: 'modified', sourceChanged: false });
});

test('a collapsed scope node falls through to the rollup', () => {
  assert.deepEqual(
    changeOf(node('src/edited.ts', 'src/edited.ts'), facts(true)),
    { status: 'modified', sourceChanged: true },
  );
});

test('impact-only movement is not an edit', () => {
  assert.equal(isEdit(TOUCHED, facts(true)), true);
  assert.equal(
    isEdit(RIPPLED, facts(true)), false,
    'only its fan-in moved — on most diffs the ripple outnumbers the edits',
  );
});

test('a working-tree diff shows what it never saw', () => {
  // The file was created since the diff ran. It is exactly what the reader
  // opened the diff to see, and dimming it to 0 would hide it.
  assert.equal(unknownVerdict(YOUNG, facts(true)), 'visible');
  assert.equal(isEdit(YOUNG, facts(true)), true);
});

test('a commit-to-commit diff dims what it never saw (UI-104)', () => {
  // The canvas is drawing the working tree while the diff describes two older
  // ones, so "missing" means younger than the head commit — which says nothing
  // about the comparison and grows with how far back it reaches.
  assert.equal(unknownVerdict(YOUNG, facts(false)), 'dimmed');
  assert.equal(
    isEdit(YOUNG, facts(false)), false,
    'it must not seed the edits rung, or no rung below neighbourhood can filter',
  );
});

test('a file the diff did look at dims either way', () => {
  for (const headIsWorking of [true, false]) {
    assert.equal(
      unknownVerdict(SKIPPED, facts(headIsWorking)), 'dimmed',
      `the diff considered src/quiet.ts and said nothing about this node (headIsWorking=${headIsWorking})`,
    );
  }
});

test('a ghost dims either way, leaving showGhosts in charge', () => {
  for (const headIsWorking of [true, false]) {
    assert.equal(unknownVerdict(GHOST, facts(headIsWorking)), 'dimmed');
  }
});

// --- UI-109: which half of the change a node is on ---

test('editKind splits the seed the ladder starts from', () => {
  const f = facts(true);
  assert.equal(editKind(ARRIVED, f), 'new', 'it did not exist on the base side');
  assert.equal(editKind(TOUCHED, f), 'existing', 'it existed and changed in place');
  assert.equal(
    editKind(DEPARTED, f), 'existing',
    'a deletion is existing code — it was there to be deleted, and it is not new',
  );
  assert.equal(editKind(RIPPLED, f), null, 'impact-only ripple is not an edit at all');
});

test('the split holds on a collapsed scope node, not only on entities', () => {
  // The canvas draws File and Module nodes above entity level, and their
  // status comes from the rollup. If the facet meant something different up
  // there, `New` would empty the picture the moment a reader zoomed out.
  assert.equal(editKind(FRESH_FILE, facts(true)), 'new');
  assert.equal(editKind(node('src/edited.ts', 'src/edited.ts'), facts(true)), 'existing');
  assert.equal(editKind(node('src/quiet.ts', 'src/quiet.ts'), facts(true)), null);
});

test('a file the diff never saw is new, and only under a working head', () => {
  // It has no ChangeStatus to read — the maps are silent about it, which is
  // the whole point of `unknownVerdict`. Under a working head that silence
  // means "created since the diff ran", so `New` is where the reader will
  // look for it. Under a commit head it is not in the seed at all (UI-105),
  // and it must not sneak into a facet either.
  assert.equal(editKind(YOUNG, facts(true)), 'new');
  assert.equal(editKind(YOUNG, facts(false)), null);
});

test('isEdit is exactly "has a kind"', () => {
  // One rule, so the seed and the facet that splits it cannot disagree about
  // what an edit is.
  for (const headIsWorking of [true, false]) {
    const f = facts(headIsWorking);
    for (const n of [TOUCHED, RIPPLED, ARRIVED, DEPARTED, YOUNG, SKIPPED, GHOST, FRESH_FILE]) {
      assert.equal(
        isEdit(n, f), editKind(n, f) !== null,
        `${n.original_id} (headIsWorking=${headIsWorking})`,
      );
    }
  }
});

test('what the diff did report is unaffected by which head it was', () => {
  // The regression guard on the fix: only the silent case may move. A node the
  // diff has an opinion about must read the same in both modes.
  for (const n of [TOUCHED, RIPPLED, ARRIVED, DEPARTED, FRESH_FILE, node('src/edited.ts', 'src/edited.ts')]) {
    assert.deepEqual(
      changeOf(n, facts(true)), changeOf(n, facts(false)),
      `${n.original_id} changed verdict with the head, and only silence should`,
    );
    assert.equal(isEdit(n, facts(true)), isEdit(n, facts(false)));
    assert.equal(
      editKind(n, facts(true)), editKind(n, facts(false)),
      `${n.original_id} changed sides with the head, and only silence should`,
    );
  }
});
