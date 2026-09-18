/**
 * UI-134 — git's list of changed files, joined against the graph's reading of
 * the same change.
 *
 * The join is the claim this pane makes, so it is what is under test: which
 * rows the canvas can draw, which it is silent about by construction, and
 * which the two sides disagree over. Every case here is one a reader would
 * otherwise have to check by hand against a live engine.
 *
 * Run: node --experimental-strip-types --test scripts/changed-files.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  readChangedFiles, headRefFor, statusChange, statusLabel, splitPath,
  agreementLabel, isSourceEdit, changedFilesPayload, statusLetter, statusPhrase,
  type ChangedFile,
} from '../src/viewmodels/changedFiles.ts';

const file = (path: string, over: Partial<ChangedFile> = {}): ChangedFile => ({
  status: 'M',
  path,
  additions: 3,
  deletions: 1,
  binary: false,
  untracked: false,
  ...over,
});

/** An analysis that loaded the two rust files and nothing else — the shape of
 *  a repo whose settings name `["rust"]`, which is most of them. */
const ANALYSED = new Set(['src/diff.rs', 'src/quiet.rs', 'src/gone.rs']);

test('a file with changed entities is drawn, and says how many', () => {
  const reading = readChangedFiles([file('src/diff.rs')], {
    changedEntities: new Map([['src/diff.rs', 4]]),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'drawn', entities: 4 });
  assert.equal(agreementLabel(reading.rows[0].agreement), '4 entities');
});

test('a file outside the analysis is named as such rather than left silent', () => {
  // The case the whole pane exists for: the canvas cannot draw README.md and
  // is not wrong to leave it out, but nothing on screen said so.
  const reading = readChangedFiles([file('README.md')], {
    changedEntities: new Map([['src/diff.rs', 4]]),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'unanalysed' });
});

test('a file the analysis loaded and the diff found nothing in is a third answer', () => {
  // Ordinary for a comment or whitespace edit — and also what a parser that
  // stopped seeing a construct looks like, which is why it is not merged with
  // "not analysed".
  const reading = readChangedFiles([file('src/quiet.rs')], {
    changedEntities: new Map(),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'silent' });
});

test('a deleted file is not accused of being unanalysed', () => {
  // It is absent from the *head* sidecar because it is gone. The base side
  // still holds it, and the entity rows for its removals are the proof.
  const reading = readChangedFiles([file('src/gone.rs', { status: 'D' })], {
    changedEntities: new Map([['src/gone.rs', 2]]),
    analysed: new Set(['src/diff.rs']),
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'drawn', entities: 2 });
});

test('nothing is accused while the analysed set is still in flight', () => {
  const reading = readChangedFiles([file('README.md')], {
    changedEntities: new Map(),
    analysed: null,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'unknown' });
  assert.equal(agreementLabel(reading.rows[0].agreement), '');
});

test('a rename is joined under either of its two names', () => {
  const renamed = file('src/new.rs', { status: 'R', old_path: 'src/old.rs' });
  const reading = readChangedFiles([renamed], {
    // The diff keyed its rows under the name the base side used.
    changedEntities: new Map([['src/old.rs', 3]]),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'drawn', entities: 3 });
  assert.deepEqual(reading.onlyInGraph, [], 'the old name is accounted for');
});

test('a file the graph claims and git does not list is surfaced, not swallowed', () => {
  // Empty on a healthy comparison. Non-empty means the two disagree about
  // what changed — a diff left over from a previous comparison, or a path
  // spelling that stopped matching.
  const reading = readChangedFiles([file('src/diff.rs')], {
    changedEntities: new Map([['src/diff.rs', 1], ['src/stale.rs', 6]]),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.onlyInGraph, ['src/stale.rs']);
});

test('the totals are git`s counts, added and removed kept apart', () => {
  const reading = readChangedFiles(
    [
      file('src/diff.rs', { additions: 10, deletions: 2 }),
      file('README.md', { additions: 1, deletions: 0 }),
      // A binary file counts as neither: git spells both columns `-`.
      file('assets/logo.png', { status: 'A', additions: 0, deletions: 0, binary: true }),
    ],
    { changedEntities: new Map(), analysed: ANALYSED },
  );
  assert.deepEqual(reading.totals, { files: 3, additions: 11, deletions: 2 });
});

test('the head literal in diff.json becomes the sentinel the endpoints take', () => {
  // The failure this prevents is silent: `working` reaches git as a ref that
  // does not resolve, and an unresolvable ref reads as "no files changed".
  assert.equal(headRefFor('working'), 'WORKING');
  assert.equal(headRefFor('staged'), 'STAGED');
  assert.equal(headRefFor('c0ff6d2'), 'c0ff6d2');
  assert.equal(headRefFor(undefined), null);
});

test('a rename keeps the modified hue, and an unknown letter keeps itself', () => {
  // Giving a `git mv` the colour of new code would make a restructure read as
  // a rewrite — the same reading `diff::match_moved` exists to avoid.
  assert.equal(statusChange('R'), 'modified');
  assert.equal(statusChange('A'), 'added');
  assert.equal(statusChange('D'), 'removed');
  assert.equal(statusLabel('D'), 'deleted');
  assert.equal(statusLabel('X'), 'X', 'a letter this code has not met arrives intact');
});

test('a path splits into the name that reads and the folder that trails it', () => {
  assert.deepEqual(splitPath('ui/src/stores/diff.ts'), {
    name: 'diff.ts',
    dir: 'ui/src/stores',
  });
  assert.deepEqual(splitPath('README.md'), { name: 'README.md', dir: '' });
});

test('a row is an edit only when the source moved, not when the ripple did', () => {
  // Caught against a live engine, not by any fixture: on the change this pane
  // was built under, `diff.json` held 474 impact-only rows against 60 real
  // ones. Counting them put eighteen files git was right to leave out into
  // `onlyInGraph`, which is the one list whose whole value is being empty.
  assert.equal(isSourceEdit({ file_path: 'a.rs', status: 'modified', source_changed: true }), true);
  assert.equal(isSourceEdit({ file_path: 'a.rs', status: 'modified', source_changed: false }), false);
  assert.equal(isSourceEdit({ file_path: 'a.rs', status: 'added', source_changed: true }), true);
  assert.equal(isSourceEdit({ file_path: 'a.rs', status: 'removed', source_changed: true }), true);
  assert.equal(isSourceEdit({ file_path: 'a.rs', status: 'unchanged', source_changed: false }), false);
});

test('a file that only felt a change elsewhere is not accused of hiding one', () => {
  // The caller counts through `isSourceEdit`, so a file whose rows are all
  // ripple arrives here with no count at all — and is `silent`, not `drawn`,
  // and not in the disagreement list either.
  const rows = [{ file_path: 'src/rippled.rs', status: 'modified' as const, source_changed: false }];
  const changedEntities = new Map(
    rows.filter(isSourceEdit).map((r) => [r.file_path, 1] as const),
  );
  const reading = readChangedFiles([file('src/quiet.rs')], {
    changedEntities,
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.rows[0].agreement, { kind: 'silent' });
  assert.deepEqual(reading.onlyInGraph, []);
});

// ─── UI-137: the payload the extension's native Changes tree renders ──────
//
// The tree cannot import the join (the extension's tsconfig sets `rootDir:
// src`), so the finished reading crosses the bridge instead. These cover the
// three ways that hand-off can lie about a comparison it is describing.

test('the bridge payload carries the same rows, worded once', () => {
  const reading = readChangedFiles(
    [file('src/diff.rs'), file('README.md', { status: 'A', untracked: true })],
    { changedEntities: new Map([['src/diff.rs', 4]]), analysed: ANALYSED },
  );
  const payload = changedFilesPayload(reading, { from_ref: 'HEAD', to_ref: 'working' });

  assert.equal(payload.active, true);
  assert.deepEqual(payload.rows.map((r) => r.path), ['src/diff.rs', 'README.md']);
  // The sentence is rendered on this side, by the same function the Svelte
  // pane calls, so the tree cannot word a row differently from the tab.
  assert.equal(payload.rows[0].agreement.label, agreementLabel(reading.rows[0].agreement));
  assert.equal(payload.rows[0].agreement.label, '4 entities');
  assert.equal(payload.rows[1].agreement.label, 'not analysed');
  assert.equal(payload.rows[1].untracked, true);
  assert.deepEqual(payload.totals, reading.totals);
  assert.deepEqual(payload.onlyInGraph, reading.onlyInGraph);
});

test('the payload sends both spellings of the head, mapped once', () => {
  // `toRef` is what the pane displays; `headRef` is what `/api/file-diff`
  // takes. Deriving the second on the extension side would be a second place
  // for a lowercase `working` to reach git as a ref that does not resolve —
  // a failure that presents as an empty file rather than as an error.
  const empty = readChangedFiles([], { changedEntities: new Map(), analysed: ANALYSED });
  const working = changedFilesPayload(empty, { from_ref: 'HEAD', to_ref: 'working' });
  assert.equal(working.toRef, 'working');
  assert.equal(working.headRef, 'WORKING');

  const staged = changedFilesPayload(empty, { from_ref: 'HEAD', to_ref: 'staged' });
  assert.equal(staged.headRef, 'STAGED');

  const sha = changedFilesPayload(empty, { from_ref: 'HEAD~2', to_ref: 'ba8c43b' });
  assert.equal(sha.headRef, 'ba8c43b');
});

test('no diff, or no reading, empties the tree rather than stranding rows', () => {
  // Both directions, because they fail differently. A reading with no refs
  // would label rows with nothing to fetch them against; refs with no reading
  // would leave the *previous* comparison's rows on screen, which under a
  // working head means describing the previous save.
  const reading = readChangedFiles([file('src/diff.rs')], {
    changedEntities: new Map([['src/diff.rs', 4]]),
    analysed: ANALYSED,
  });
  assert.equal(changedFilesPayload(reading, null).active, false);
  assert.equal(changedFilesPayload(null, { from_ref: 'HEAD', to_ref: 'working' }).active, false);

  const none = changedFilesPayload(null, null);
  assert.equal(none.active, false);
  assert.deepEqual(none.rows, []);
  assert.deepEqual(none.totals, { files: 0, additions: 0, deletions: 0 });
});

test('the disagreement list survives the crossing', () => {
  // `onlyInGraph` is the one number whose whole value is being zero, and it is
  // the reason the join is not duplicated on the far side of the bridge: two
  // copies could report two different residues for the same comparison.
  const reading = readChangedFiles([file('src/diff.rs')], {
    changedEntities: new Map([['src/diff.rs', 4], ['src/vanished.rs', 2]]),
    analysed: ANALYSED,
  });
  assert.deepEqual(reading.onlyInGraph, ['src/vanished.rs']);
  assert.deepEqual(
    changedFilesPayload(reading, { from_ref: 'HEAD', to_ref: 'working' }).onlyInGraph,
    ['src/vanished.rs'],
  );
});

test('a file git has never followed reads U, not A', () => {
  // `git diff --name-status` has no code for untracked, so the endpoint tags
  // such a file `A` and sets the flag — faithful to git, and the right thing
  // to store. Showing that `A` is the bug: it makes a file git has never heard
  // of look identical to one staged for the next commit, and only one of the
  // two survives a checkout.
  assert.equal(statusLetter({ status: 'A', untracked: true }), 'U');
  assert.equal(statusPhrase({ status: 'A', untracked: true }), 'untracked');

  // A tracked addition keeps git's own letter and word.
  assert.equal(statusLetter({ status: 'A', untracked: false }), 'A');
  assert.equal(statusPhrase({ status: 'A', untracked: false }), 'added');

  // Everything else is git's letter unchanged.
  assert.equal(statusLetter({ status: 'M', untracked: false }), 'M');
  assert.equal(statusLetter({ status: 'D', untracked: false }), 'D');
  assert.equal(statusPhrase({ status: 'R', untracked: false }), 'renamed');
});

test('the payload sends the reader-facing letter beside git\'s own', () => {
  // Both, so nothing downstream has to know which it is holding — and so the
  // native tree cannot re-derive the rule and drift from the browser tab.
  const reading = readChangedFiles(
    [file('notes.txt', { status: 'A', untracked: true }), file('src/diff.rs')],
    { changedEntities: new Map([['src/diff.rs', 4]]), analysed: ANALYSED },
  );
  const payload = changedFilesPayload(reading, { from_ref: 'HEAD', to_ref: 'working' });

  assert.equal(payload.rows[0].status, 'A');
  assert.equal(payload.rows[0].letter, 'U');
  assert.equal(payload.rows[0].phrase, 'untracked');
  assert.equal(payload.rows[1].letter, 'M');
  assert.equal(payload.rows[1].phrase, 'modified');
});

test('an untracked file still draws in the added colour', () => {
  // The letter changed; the hue did not. `U` is a file that is not there yet
  // in either tree, which is what green means on this canvas.
  assert.equal(statusChange('A'), 'added');
});

test('the payload names both sides, resolved against the picker\'s own list', () => {
  // UI-139. The labels are rendered here because this is the only side that
  // holds the commit list; the extension gets finished strings.
  const empty = readChangedFiles([], { changedEntities: new Map(), analysed: ANALYSED });
  const commits = [{
    hash: 'ba8c43b1f2e3d4c5b6a798877665544332211000',
    short_hash: 'ba8c43b',
    message: 'Let a comparison scope itself to what it compared',
    author: 'Netajam',
    date: '2026-08-30',
  }];

  const payload = changedFilesPayload(
    empty,
    { from_ref: 'ba8c43b1f2e3d4c5b6a798877665544332211000', to_ref: 'working' },
    { commits },
  );
  assert.equal(payload.fromLabel.text, 'ba8c43b Let a comparison scope itself to what it compared');
  assert.equal(payload.fromLabel.live, false);
  assert.equal(payload.toLabel.text, 'Working tree');
  assert.equal(payload.toLabel.live, true);
});

test('with no commit list loaded the labels fall back to the refs', () => {
  // Not an error state: the picker may not have fetched yet. The header then
  // says exactly what it said before this existed, rather than nothing.
  const empty = readChangedFiles([], { changedEntities: new Map(), analysed: ANALYSED });
  const payload = changedFilesPayload(empty, { from_ref: 'HEAD~3', to_ref: 'working' });
  assert.equal(payload.fromLabel.text, 'HEAD~3');
  assert.equal(payload.toLabel.text, 'Working tree');
});
