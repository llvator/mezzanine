/**
 * What the commit picker's `From` means, and where the boundary is drawn
 * (UI-151).
 *
 *   npm run test:commitrange
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  baseRefFor, railRows, includedCount, rangeWarning, isRoot, findCommit,
} from '../src/viewmodels/commitRange.ts';
import type { Commit } from '../src/stores/diff';

/** Newest first, as `git log` and the picker both list them. */
function history(n: number): Commit[] {
  return Array.from({ length: n }, (_, i) => ({
    hash: `c${i}`.padEnd(40, '0'),
    short_hash: `c${i}`,
    parent_hash: i + 1 < n ? `c${i + 1}`.padEnd(40, '0') : undefined,
    message: `commit ${i}`,
    author: 'a',
    date: '2026-09-01',
  }));
}

const H = (i: number) => `c${i}`.padEnd(40, '0');

test('the picked commit is inside the comparison, so the base is its parent', () => {
  const commits = history(5);
  assert.equal(baseRefFor(H(2), commits).ref, H(3));
});

test('a merge is entered by its first parent', () => {
  const commits = history(3);
  commits[0].parent_hash = `${H(1)} ${H(2)}`.split(' ')[0];
  assert.equal(baseRefFor(H(0), commits).ref, H(1));
});

test('the root commit cannot be the oldest one included', () => {
  const commits = history(3);
  const base = baseRefFor(H(2), commits);
  assert.equal(base.ref, null);
  assert.equal(base.problem, 'root');
  assert.equal(isRoot(commits[2], commits), true);
  assert.equal(isRoot(commits[0], commits), false);
});

test('a listing with no parents at all is an old server, not a repo of roots', () => {
  // Every `From` would otherwise be blocked against a server built before the
  // listing carried `%P`.
  const commits = history(3).map((c) => ({ ...c, parent_hash: undefined }));
  assert.equal(isRoot(commits[0], commits), false);
  assert.equal(baseRefFor(H(0), commits).ref, `${H(0)}~1`);
});

test('a ref the list does not hold is left to git', () => {
  assert.equal(baseRefFor('feature-x', history(3)).ref, 'feature-x~1');
});

test('a hand-typed short hash resolves to the same parent as a click', () => {
  const commits = history(5);
  assert.equal(baseRefFor('c2', commits).ref, H(3));
  assert.equal(findCommit('c2', commits)?.hash, H(2));
});

test('the rail marks the span, its two ends, and the base below it', () => {
  const rows = railRows(history(6).map((c) => c.hash), H(3), H(1));

  assert.deepEqual(rows.map((r) => r.included), [false, true, true, true, false, false]);
  assert.equal(rows[1].isTo, true);
  assert.equal(rows[3].isFrom, true);
  assert.equal(includedCount(rows), 3);

  // The one commit outside the range, named — and the divider sits above it,
  // which is directly below `From`.
  assert.equal(rows[4].isBase, true);
  assert.equal(rows[4].boundaryAbove, true);
  assert.equal(rows.filter((r) => r.boundaryAbove).length, 1);
});

test('the lit segments stop at the two end nodes', () => {
  const rows = railRows(history(6).map((c) => c.hash), H(3), H(1));
  assert.equal(rows[1].litAbove, false, 'nothing runs up out of To');
  assert.equal(rows[1].litBelow, true);
  assert.equal(rows[2].litAbove, true);
  assert.equal(rows[2].litBelow, true);
  assert.equal(rows[3].litAbove, true);
  assert.equal(rows[3].litBelow, false, 'nothing runs down out of From');
});

test('a single commit is a range of one, with no lit segment', () => {
  const rows = railRows(history(4).map((c) => c.hash), H(1), H(1));
  assert.equal(includedCount(rows), 1);
  assert.equal(rows[1].isFrom && rows[1].isTo, true);
  assert.equal(rows[1].litAbove || rows[1].litBelow, false);
  assert.equal(rows[2].boundaryAbove, true);
});

test('HEAD is the top of the list it is being read against', () => {
  const hashes = history(4).map((c) => c.hash);
  const rows = railRows(hashes, H(2), 'HEAD', H(0));
  assert.equal(includedCount(rows), 3);
  assert.equal(rows[0].isTo, true);
});

test("HEAD against another branch's listing places nothing", () => {
  const rows = railRows(history(4).map((c) => c.hash), H(2), 'HEAD', null);
  assert.equal(includedCount(rows), null);
  assert.match(rangeWarning(history(4).map((c) => c.hash), H(2), 'HEAD', null)!, /not in the list/);
});

test('a range that runs backwards is warned about, not drawn', () => {
  const hashes = history(5).map((c) => c.hash);
  const rows = railRows(hashes, H(1), H(3));
  assert.equal(includedCount(rows), null);
  assert.equal(rows.some((r) => r.boundaryAbove), false);
  assert.match(rangeWarning(hashes, H(1), H(3))!, /newer than/);
});

test('the base has nowhere to be drawn when From is the oldest row loaded', () => {
  const hashes = history(3).map((c) => c.hash);
  const rows = railRows(hashes, H(2), H(0));
  assert.equal(includedCount(rows), 3);
  assert.equal(rows.some((r) => r.boundaryAbove), false);
});

test('an unfinished pick is not a mistake', () => {
  const hashes = history(3).map((c) => c.hash);
  assert.equal(rangeWarning(hashes, '', 'HEAD', H(0)), null);
  assert.equal(rangeWarning(hashes, H(1), ''), null);
});

test('a complete, drawable range says nothing', () => {
  const hashes = history(5).map((c) => c.hash);
  assert.equal(rangeWarning(hashes, H(3), H(1)), null);
});
