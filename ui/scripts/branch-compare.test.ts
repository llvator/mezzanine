/**
 * UI-143 — comparing two branches, and picking the base that answers the
 * question the reviewer asked.
 *
 * The rule under test throughout: the base for reviewing a branch is where it
 * left the other one, not where the other one has got to since. Every other
 * assertion here is about saying which of the two is on screen — a tip-to-tip
 * comparison is not wrong, it answers a different question, and the failure
 * this module exists to prevent is the two being indistinguishable.
 *
 * Run: node --experimental-strip-types --test scripts/branch-compare.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { orderBranches, defaultPair, branchPlan } from '../src/viewmodels/branchCompare.ts';
import type { BranchRef, Commit } from '../src/stores/diff.ts';

function branch(name: string, over: Partial<BranchRef> = {}): BranchRef {
  return {
    name,
    remote: false,
    is_head: false,
    tip: `${name}-tip-hash`,
    tip_short: `${name.slice(0, 3)}1234`,
    subject: `latest on ${name}`,
    author: 'Ada',
    date: '2026-08-31',
    ...over,
  };
}

const FORK: Commit = {
  hash: 'fork-hash',
  short_hash: 'f0r4b45',
  message: 'the commit they both came from\n\nand a body nobody labels with',
  author: 'Ada',
  date: '2026-08-20',
};

const MAIN = branch('main');
const FEATURE = branch('feat/chip', { is_head: true });

// ---------------------------------------------------------------------------
//  Which branches are offered, and which two are offered first
// ---------------------------------------------------------------------------

test('local branches come before remote-tracking ones', () => {
  const ordered = orderBranches([
    branch('origin/main', { remote: true }),
    MAIN,
    branch('origin/feat/chip', { remote: true }),
    FEATURE,
  ]);
  assert.deepEqual(ordered.map((b) => b.name), ['main', 'feat/chip', 'origin/main', 'origin/feat/chip']);
});

test('the branch under review defaults to the one checked out', () => {
  const { base, compare } = defaultPair([MAIN, FEATURE]);
  assert.equal(compare?.name, 'feat/chip');
  assert.equal(base?.name, 'main');
});

test('sitting on main makes it the base, not the branch under review', () => {
  // Where a reviewer pulling someone else's work sits. The checked-out branch
  // points both ways depending on which branch it is, and reading it as "the
  // thing under review" here pairs main against a feature branch backwards.
  const onMain = branch('main', { is_head: true });
  const { base, compare } = defaultPair([onMain, branch('feat/chip')]);
  assert.equal(base?.name, 'main');
  assert.equal(compare?.name, 'feat/chip');
});

test('on main with several branches, the most recently worked on is offered', () => {
  // The server sends most-recently-committed first.
  const onMain = branch('main', { is_head: true });
  const { compare } = defaultPair([onMain, branch('feat/today'), branch('feat/last-year')]);
  assert.equal(compare?.name, 'feat/today');
});

test('the base defaults to what the repository integrates into, not to the newest branch', () => {
  // Server order is most-recently-committed first, so a busy side branch sits
  // above `master` — and would win a "first other branch" rule.
  const { base } = defaultPair([FEATURE, branch('spike/perf'), branch('master')]);
  assert.equal(base?.name, 'master');
});

test('the two sides never open equal', () => {
  // One branch, and it is the one checked out: there is no base to offer, and
  // offering the same branch twice would open on the one refusal.
  const { base, compare } = defaultPair([FEATURE]);
  assert.equal(compare?.name, 'feat/chip');
  assert.equal(base, undefined);
});

test('a repository with no branches offers no pair rather than a broken one', () => {
  assert.deepEqual(defaultPair([]), {});
});

// ---------------------------------------------------------------------------
//  What the comparison actually compares
// ---------------------------------------------------------------------------

test('reviewing a branch measures from where it diverged', () => {
  const plan = branchPlan(MAIN, FEATURE, FORK, true);
  assert.equal(plan.refusal, null);
  assert.equal(plan.from, 'fork-hash', 'the divergence point, not main’s tip');
  assert.equal(plan.to, 'feat/chip', 'a name, because it means wherever the branch is');
  assert.match(plan.note, /diverged/);
  assert.match(plan.note, /f0r4b45/, 'the base is named, not assumed');
});

test('the sentence carries the subject’s first line only', () => {
  assert.match(branchPlan(MAIN, FEATURE, FORK, true).note, /the commit they both came from\./);
  assert.doesNotMatch(branchPlan(MAIN, FEATURE, FORK, true).note, /a body nobody labels with/);
});

test('a tip-to-tip comparison says what it will report as removed', () => {
  const plan = branchPlan(MAIN, FEATURE, FORK, false);
  assert.equal(plan.from, 'main', 'the name, so the log line reads as the branch');
  assert.equal(plan.to, 'feat/chip');
  assert.equal(plan.refusal, null);
  // The whole reason the divergence reading exists. A reader who deliberately
  // picks tip-to-tip must be told what they picked.
  assert.match(plan.note, /removed/);
});

test('branches with no common commit fall back to their tips and say so', () => {
  const plan = branchPlan(MAIN, branch('orphan'), null, true);
  assert.equal(plan.from, 'main');
  assert.equal(plan.refusal, null, 'unrelated histories are comparable, just not from a fork');
  assert.match(plan.note, /share no history/);
});

// ---------------------------------------------------------------------------
//  The pairings that are refused before a worktree is checked out
// ---------------------------------------------------------------------------

test('a branch compared against itself is refused', () => {
  const plan = branchPlan(MAIN, branch('main'), FORK, true);
  assert.ok(plan.refusal, 'refused');
  assert.match(plan.refusal!, /itself/);
});

test('a branch already merged into the base is refused by name', () => {
  // Its divergence point *is* its tip. The engine would answer `+0 −0 ~0`
  // after two checkouts and two full analyses; this answers before either.
  const merged = branch('feat/landed');
  const at_tip: Commit = { ...FORK, hash: merged.tip };
  const plan = branchPlan(MAIN, merged, at_tip, true);
  assert.ok(plan.refusal, 'refused');
  assert.match(plan.refusal!, /already have|contained/);
  assert.match(plan.note, /already contained in main/);
});

test('a half-chosen pair is refused rather than sent', () => {
  assert.ok(branchPlan(undefined, FEATURE, null, true).refusal);
  assert.ok(branchPlan(MAIN, undefined, null, true).refusal);
});
