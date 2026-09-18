/**
 * UI-139 — how a comparison names the two things it is comparing.
 *
 * The header said `ba8c43b → working`, which is honest and is most of a
 * question. What is under test is the part that can lie: a sentinel dressed as
 * a commit, a ref resolved to a subject that is not its own, and `HEAD`
 * silently replaced by the hash it happened to point at.
 *
 * Run: node --experimental-strip-types --test scripts/ref-label.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { refLabel, subjectOf, truncateSubject } from '../src/viewmodels/refLabel.ts';
import type { Commit, Stash } from '../src/stores/diff.ts';

const commit = (over: Partial<Commit> = {}): Commit => ({
  hash: 'ba8c43b1f2e3d4c5b6a798877665544332211000',
  short_hash: 'ba8c43b',
  message: 'Let a comparison scope itself to what it compared',
  author: 'Netajam',
  date: '2026-08-30',
  ...over,
});

const COMMITS = [commit(), commit({
  hash: 'fd67139aabbccddeeff00112233445566778899a',
  short_hash: 'fd67139',
  message: 'File SRV-021: a diff run from a subdirectory\n\nbody text, not a subject',
})];

const STASHES: Stash[] = [{
  hash: '99aabbccddeeff00112233445566778899aabbcc',
  short_hash: '99aabbc',
  selector: 'stash@{0}',
  base_hash: 'ba8c43b1f2e3d4c5b6a798877665544332211000',
  base_short: 'ba8c43b',
  message: 'WIP on main: the half-finished parser',
  author: 'Netajam',
  date: '2026-08-29',
}];

test('a commit is named by its hash and its subject', () => {
  const label = refLabel('ba8c43b1f2e3d4c5b6a798877665544332211000', COMMITS);
  assert.equal(label.text, 'ba8c43b Let a comparison scope itself to what it compared');
  assert.equal(label.live, false);
  assert.match(label.title, /Netajam/);
});

test('only the first line is the subject', () => {
  // A commit body is not a label. Rendering it would put a paragraph in a
  // header sized for one line.
  assert.equal(subjectOf('subject\n\nbody\nmore body'), 'subject');
  const label = refLabel('fd67139', COMMITS);
  assert.equal(label.text, 'fd67139 File SRV-021: a diff run from a subdirectory');
  assert.ok(!label.text.includes('body text'));
});

test('the two sentinels are states, not commits', () => {
  // `working` and `staged` name no object, have no subject, and must not be
  // looked up — asking the commit list about them returns nothing, which
  // would render as a ref that failed to resolve.
  const working = refLabel('working', COMMITS);
  assert.equal(working.text, 'Working tree');
  assert.equal(working.live, true);

  const staged = refLabel('staged', COMMITS);
  assert.equal(staged.text, 'Staged');
  assert.equal(staged.live, true);
});

test('HEAD keeps its own name even when it resolves', () => {
  // HEAD means "wherever I am", which a hash stops meaning the moment
  // anything lands. The subject still comes along.
  const label = refLabel('HEAD', [commit({ hash: 'HEAD' })]);
  assert.ok(label.text.startsWith('HEAD '));
  assert.ok(!label.text.startsWith('ba8c43b'));
});

test('a ref the list does not hold is named as itself', () => {
  // `HEAD~12`, a branch, a commit older than the loaded window. An empty
  // subject here would read as a commit with no message rather than as one
  // nobody looked up.
  assert.equal(refLabel('HEAD~12', COMMITS).text, 'HEAD~12');
  assert.equal(refLabel('deadbeefdeadbeef', COMMITS).text, 'deadbeefdeadbeef');
  assert.equal(refLabel('main', []).text, 'main');
});

test('a hash matches on either spelling and in either direction', () => {
  // The ref arrives in whatever form produced it: diff.json echoes a full
  // hash, a hand-typed From may be seven characters, git abbreviates to
  // whatever is unambiguous.
  assert.ok(refLabel('ba8c43b', COMMITS).text.startsWith('ba8c43b '));
  assert.ok(refLabel('ba8c43b1f2e3', COMMITS).text.startsWith('ba8c43b '));
  assert.ok(refLabel('ba8c43b1f2e3d4c5b6a798877665544332211000', COMMITS).text.startsWith('ba8c43b '));
});

test('a stash is named by its selector, not by its hash', () => {
  const label = refLabel('99aabbccddeeff00112233445566778899aabbcc', COMMITS, STASHES);
  assert.ok(label.text.startsWith('stash@{0} '));
  assert.match(label.text, /half-finished parser/);
});

test('a long subject is cut, and the hash still identifies it', () => {
  const long = 'x'.repeat(120);
  assert.ok(truncateSubject(long).length <= 52);
  assert.ok(truncateSubject(long).endsWith('…'));
  // Short ones are untouched — no ellipsis on something that fit.
  assert.equal(truncateSubject('short'), 'short');
});

test('no ref at all says so rather than rendering empty', () => {
  const label = refLabel(undefined);
  assert.equal(label.text, '?');
  assert.equal(label.live, false);
});
