/**
 * UI-114 — the chip that says which branch the canvas is drawing.
 *
 * Four server answers, and the rule under test is that none of them is an
 * error: a detached HEAD, a branch with no commits and a directory that is not
 * a checkout at all are ordinary states of a thing `mezz watch` is pointed at.
 * The chip either names the branch or says nothing.
 *
 * Run: node --experimental-strip-types --test scripts/branch-label.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { branchLabel } from '../src/viewmodels/branchLabel.ts';
import type { BranchInfo } from '../src/stores/branch.ts';

const ON_MAIN: BranchInfo = {
  branch: 'main',
  detached: false,
  head_short: '885b981',
  git: true,
};

test('a branch puts its own name on the chip', () => {
  const label = branchLabel(ON_MAIN);
  assert.equal(label?.text, 'main');
  assert.equal(label?.detached, false);
  assert.match(label!.title, /885b981/, 'the commit is in the tooltip');
});

test('the tooltip says the graph is this checkout, not the comparison', () => {
  // The one thing a reader cannot see for themselves. With a diff loaded the
  // canvas is covered in refs, and the circles belong to none of them — the
  // server keeps them on the working tree (SRV-019).
  assert.match(branchLabel(ON_MAIN)!.title, /working tree/);
});

test('a detached HEAD names the commit, because that is the only name it has', () => {
  const label = branchLabel({
    branch: null,
    detached: true,
    head_short: 'c0ff6d2',
    git: true,
  });
  assert.equal(label?.detached, true);
  assert.match(label!.text, /c0ff6d2/, '"detached" alone does not say where');
});

test('an unborn branch is named, and said to have nothing on it', () => {
  const label = branchLabel({
    branch: 'main',
    detached: false,
    head_short: null,
    git: true,
  });
  assert.equal(label?.text, 'main');
  assert.match(label!.title, /no commits/);
});

test('a root that is not a checkout renders nothing at all', () => {
  // `mezz watch` runs against any directory. A chip reading "no branch" there
  // would invent a problem out of an ordinary state.
  const label = branchLabel({
    branch: null,
    detached: false,
    head_short: null,
    git: false,
  });
  assert.equal(label, null);
});

test('an engine too old to answer renders nothing rather than guessing', () => {
  assert.equal(branchLabel(null), null);
});
