/**
 * UI-149 — filtering the Changes tab by kind of change.
 *
 * Three rules carry the feature, and each one is a decision a reader would
 * otherwise discover by being surprised: an empty selection shows everything,
 * a selected kind the comparison no longer contains keeps its chip, and the
 * counts describe the whole change rather than the filtered list.
 *
 * Run: node --experimental-strip-types --test scripts/change-facets.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { changeFacets, filterRows, toggleFacet } from '../src/viewmodels/changeFacets.ts';
import type { ChangedFile, ChangedFileRow } from '../src/viewmodels/changedFiles.ts';

const row = (path: string, over: Partial<ChangedFile> = {}): ChangedFileRow => ({
  file: {
    status: 'M',
    path,
    additions: 3,
    deletions: 1,
    binary: false,
    untracked: false,
    ...over,
  },
  agreement: { kind: 'silent' },
});

/** An ordinary working tree: two edits, one new file staged, one never added,
 *  one deletion. The shape the chips were drawn for. */
const ROWS = [
  row('src/diff.rs'),
  row('src/quiet.rs'),
  row('src/new.rs', { status: 'A' }),
  row('notes.md', { status: 'A', untracked: true }),
  row('src/gone.rs', { status: 'D' }),
];

test('a facet per kind, counted over every row', () => {
  const facets = changeFacets(ROWS);
  assert.deepEqual(
    facets.map((f) => [f.letter, f.count, f.phrase]),
    [
      ['M', 2, 'modified'],
      ['A', 1, 'added'],
      ['U', 1, 'untracked'],
      ['D', 1, 'deleted'],
    ],
  );
});

test('untracked is its own facet, not folded into added', () => {
  // The distinction git does not make and a reviewer does: `A` is in the
  // change, `U` is not in the repository at all. Folding them would put the
  // one question the list is opened to answer behind the same chip as its
  // opposite.
  const facets = changeFacets(ROWS);
  const letters = facets.map((f) => f.letter);
  assert.ok(letters.includes('U') && letters.includes('A'));
  assert.equal(facets.find((f) => f.letter === 'U')?.count, 1);
  assert.equal(facets.find((f) => f.letter === 'A')?.count, 1);
});

test('an untracked chip carries the added colour, and a rename the modified one', () => {
  // A rename is not a rewrite: giving `git mv` the green of new code would
  // make a restructure read as one. The same rule `statusChange` states.
  const facets = changeFacets([...ROWS, row('src/moved.rs', { status: 'R', old_path: 'src/was.rs' })]);
  assert.equal(facets.find((f) => f.letter === 'U')?.change, 'added');
  assert.equal(facets.find((f) => f.letter === 'R')?.change, 'modified');
});

test('nothing selected shows everything', () => {
  // The tab opens in this state, and a tab that opened empty would look like a
  // comparison that found no files.
  assert.equal(filterRows(ROWS, new Set()).length, ROWS.length);
});

test('a selection keeps only the rows of those kinds', () => {
  const shown = filterRows(ROWS, new Set(['M']));
  assert.deepEqual(shown.map((r) => r.file.path), ['src/diff.rs', 'src/quiet.rs']);
});

test('two kinds selected is a union, not an intersection', () => {
  const shown = filterRows(ROWS, new Set(['A', 'U']));
  assert.deepEqual(shown.map((r) => r.file.path), ['src/new.rs', 'notes.md']);
});

test('a selected kind survives a save that removes its last file', () => {
  // Under `→ working` the list is refetched on every save. Dropping the chip
  // when its count hits zero would take away the only control that turns the
  // filter off at exactly the moment the filter empties the list.
  const facets = changeFacets(
    ROWS.filter((r) => r.file.status !== 'D'),
    new Set(['D']),
  );
  const deleted = facets.find((f) => f.letter === 'D');
  assert.deepEqual(deleted && [deleted.count, deleted.selected], [0, true]);
});

test('an unknown letter reaches the reader as itself, after the known ones', () => {
  // The rule `statusLabel` follows: a letter this pane has no name for is
  // passed through rather than swallowed.
  const facets = changeFacets([...ROWS, row('link', { status: 'X' })]);
  assert.equal(facets[facets.length - 1].letter, 'X');
  assert.equal(facets[facets.length - 1].phrase, 'X');
});

test('the chip order does not follow the counts', () => {
  // Under a working-tree head the counts move on every save. Chips that
  // reshuffled with them would cost a reader more than the filter saves, since
  // a click is aimed at a position.
  const many = [...ROWS, row('a.rs', { status: 'D' }), row('b.rs', { status: 'D' }), row('c.rs', { status: 'D' })];
  assert.deepEqual(changeFacets(many).map((f) => f.letter), ['M', 'A', 'U', 'D']);
});

test('toggling returns a new set, and toggling twice is the identity', () => {
  // A set mutated in place is a store that does not notify.
  const first = toggleFacet(new Set<string>(), 'M');
  assert.deepEqual([...first], ['M']);
  const second = toggleFacet(first, 'M');
  assert.notEqual(second, first);
  assert.equal(second.size, 0);
});
