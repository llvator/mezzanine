/**
 * Unit tests for the source-diff view model (UI-085).
 *
 * The details pane's job under a loaded diff is to answer "what changed in
 * this code", and the three functions here are the whole of that answer:
 * how much moved, which lines sit opposite which in a side-by-side reading,
 * and what can be folded away without hiding a change.
 *
 * The properties that matter are the ones a reader would notice being wrong:
 *
 *   1. a replaced line lands on ONE split row, not two screens apart —
 *      otherwise the split view is worse than the unified one it replaced
 *   2. folding never eats a changed line, or its immediate context
 *   3. a diff with nothing in it is not reported as "N lines hidden"
 *
 *   npm run test:linediff
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  computeLineDiff,
  changeCounts,
  chunkDiff,
  toSplitRows,
  exceedsLcsBudget,
  LCS_BUDGET,
  type DiffLine,
} from '../src/utils/lineDiff.ts';

const lines = (n: number, prefix = 'line') =>
  Array.from({ length: n }, (_, i) => `${prefix} ${i + 1}`).join('\n');

test('a one-line edit counts as one added and one removed', () => {
  const d = computeLineDiff('a\nb\nc', 'a\nB\nc');
  assert.deepEqual(changeCounts(d), { added: 1, removed: 1 });
});

test('an unchanged body reports no change at all', () => {
  const d = computeLineDiff('a\nb', 'a\nb');
  assert.deepEqual(changeCounts(d), { added: 0, removed: 0 });
  assert.ok(d.every((l) => l.kind === 'equal'));
});

test('a replaced line puts before and after on the same split row', () => {
  // The reason split view exists. Unzipped, `b` and `B` are two rows and the
  // reader has to hold one in their head while they find the other.
  const rows = toSplitRows(computeLineDiff('a\nb\nc', 'a\nB\nc'));
  assert.equal(rows.length, 3);
  assert.equal(rows[1].base?.text, 'b');
  assert.equal(rows[1].head?.text, 'B');
});

test('an insertion pads the side it did not happen on', () => {
  const rows = toSplitRows(computeLineDiff('a\nc', 'a\nb\nc'));
  const inserted = rows.find((r) => r.head?.text === 'b');
  assert.ok(inserted, 'the inserted line has a row');
  assert.equal(inserted!.base, null, 'nothing opposite it on the before side');
  // Padding is a cell, not a dropped row: the sides stay aligned below it.
  assert.equal(rows.length, 3);
});

test('a longer replacement block keeps its extra lines, unpaired', () => {
  const rows = toSplitRows(computeLineDiff('a\nx\nz', 'a\n1\n2\n3\nz'));
  const paired = rows.filter((r) => r.base && r.head);
  const headOnly = rows.filter((r) => !r.base && r.head);
  assert.equal(paired.length, 3, 'a/z equal rows plus the x↔1 pairing');
  assert.equal(headOnly.length, 2, 'lines 2 and 3 have nothing to sit beside');
});

test('folding hides the far-away middle and keeps three lines of context', () => {
  const base = lines(40);
  const head = base.replace('line 20', 'line 20 CHANGED');
  const chunks = chunkDiff(computeLineDiff(base, head));

  const gaps = chunks.filter((c) => c.kind === 'gap');
  assert.equal(gaps.length, 2, 'one gap before the change, one after');

  const shown = chunks.flatMap((c) => (c.kind === 'lines' ? c.lines : []));
  assert.ok(shown.some((l) => l.text === 'line 20 CHANGED'), 'the change survives folding');
  assert.ok(shown.some((l) => l.text === 'line 17'), 'three lines of context above');
  assert.ok(shown.some((l) => l.text === 'line 23'), 'three lines of context below');
  assert.ok(!shown.some((l) => l.text === 'line 5'), 'and the rest is gone');
});

test('folding accounts for every line it hides', () => {
  const base = lines(40);
  const head = base.replace('line 20', 'line 20 CHANGED');
  const all = computeLineDiff(base, head);
  const chunks = chunkDiff(all);
  const kept = chunks.reduce((n, c) => n + (c.kind === 'lines' ? c.lines.length : 0), 0);
  const hidden = chunks.reduce((n, c) => n + (c.kind === 'gap' ? c.hidden : 0), 0);
  assert.equal(kept + hidden, all.length);
});

test('a gap of three lines or fewer is shown rather than announced', () => {
  // "⋯ 2 unchanged lines" is taller than the two lines it replaces.
  const base = 'a\nb\nc\nd\ne\nf\ng';
  const head = 'A\nb\nc\nd\ne\nf\nG';
  const chunks = chunkDiff(computeLineDiff(base, head), 1);
  assert.ok(!chunks.some((c) => c.kind === 'gap'), 'no gap worth taking');
});

test('nothing changed means nothing is folded away', () => {
  // A lone "42 lines hidden" note would be the view claiming to have folded
  // a change out of sight when there is no change.
  const d = computeLineDiff(lines(42), lines(42));
  const chunks = chunkDiff(d);
  assert.equal(chunks.length, 1);
  assert.equal(chunks[0].kind, 'lines');
  assert.equal((chunks[0] as { lines: DiffLine[] }).lines.length, 42);
});

test('two separate edits keep their own context blocks', () => {
  const base = lines(60);
  const head = base.replace('line 10', 'line 10 X').replace('line 50', 'line 50 Y');
  const chunks = chunkDiff(computeLineDiff(base, head));
  const gaps = chunks.filter((c) => c.kind === 'gap');
  assert.equal(gaps.length, 3, 'before, between, after');
});

// ── Whole-file diffs (UI-097) ────────────────────────────────────────────
//
// Clicking a file node diffs the file, not a 40-line function, so the cost of
// the alignment stopped being theoretical. These pin the two things that
// makes safe: the numbering still refers to the real file, and a pathological
// pair degrades instead of hanging the pane.

test('line numbers survive the trimmed prefix', () => {
  // The trim is an optimisation; if it leaked into the numbering, every line
  // number in a whole-file diff would be wrong by the length of the prologue.
  const base = `${lines(500)}\nchanged\n${lines(500, 'tail')}`;
  const head = `${lines(500)}\nCHANGED\n${lines(500, 'tail')}`;
  const d = computeLineDiff(base, head);
  const removed = d.find((l) => l.text === 'changed');
  const added = d.find((l) => l.text === 'CHANGED');
  assert.equal(removed?.baseLine, 501);
  assert.equal(added?.headLine, 501);
  assert.equal(d.length, 1002, 'every line of both sides is accounted for');
});

test('the shared tail keeps its own numbering', () => {
  const base = `a\nb\n${lines(5, 'tail')}`;
  const head = `a\nb\nEXTRA\n${lines(5, 'tail')}`;
  const d = computeLineDiff(base, head);
  const last = d[d.length - 1];
  assert.equal(last.text, 'tail 5');
  assert.equal(last.baseLine, 7, 'seven lines on the before side');
  assert.equal(last.headLine, 8, 'eight on the after side');
});

test('a big file with a small edit stays cheap', () => {
  // 6,000 lines a side is 36M cells untrimmed — over the budget and slow
  // enough to freeze the pane. Trimmed, the middle is one line.
  const base = `${lines(3000)}\nx\n${lines(3000, 'tail')}`;
  const head = `${lines(3000)}\nX\n${lines(3000, 'tail')}`;
  assert.equal(exceedsLcsBudget(base, head), false, 'the trim brings it well under');
  const started = process.hrtime.bigint();
  const d = computeLineDiff(base, head);
  const ms = Number(process.hrtime.bigint() - started) / 1e6;
  assert.deepEqual(changeCounts(d), { added: 1, removed: 1 });
  assert.ok(ms < 500, `took ${ms.toFixed(0)}ms — the trim is not doing its job`);
});

test('two wholly different large files degrade instead of hanging', () => {
  const base = lines(3000, 'alpha');
  const head = lines(3000, 'beta');
  assert.equal(exceedsLcsBudget(base, head), true, '9M cells is over the budget');
  const started = process.hrtime.bigint();
  const d = computeLineDiff(base, head);
  const ms = Number(process.hrtime.bigint() - started) / 1e6;
  // Everything removed, everything added: the honest degradation, and what
  // the caller warns about rather than passing off as an alignment.
  assert.deepEqual(changeCounts(d), { added: 3000, removed: 3000 });
  assert.ok(ms < 500, `took ${ms.toFixed(0)}ms — the budget did not hold`);
  assert.ok(LCS_BUDGET > 0);
});
