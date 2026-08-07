/**
 * Unit tests for the wayback — back, forward, and what counts as a frame
 * (UI-092).
 *
 * The failure modes worth guarding are all shaped the same way: a stack that
 * looks right after one step and is wrong after three. So most of these
 * assert on a *sequence* rather than a single call — drill, drill, back,
 * navigate — which is where a forward stack that should have been dropped,
 * or a frame that should have been coalesced, actually shows up.
 *
 *   npm run test:history
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptyState, type ViewState } from '../src/viewmodels/savedViews.ts';
import {
  HISTORY_LIMIT,
  NO_HISTORY,
  canGoBack,
  canGoForward,
  peekBack,
  peekForward,
  record,
  stepBack,
  stepForward,
  stepTitle,
  worthRecording,
  type ViewHistory,
} from '../src/viewmodels/viewHistory.ts';

/** A picture of `path`, with whatever else the case needs. */
function view(path: string, over: Partial<ViewState> = {}): ViewState {
  return { ...emptyState(), scope: [{ pattern: path, negate: false }], ...over };
}

/** The scope patterns of a stack, for readable assertions. */
function paths(frames: ViewState[]): string[] {
  return frames.map((f) => f.scope.filter((r) => !r.negate).map((r) => r.pattern).join('+'));
}

// --- what counts as a frame ---

test('a picture with a scope is worth returning to', () => {
  assert.equal(worthRecording(view('ui/src')), true);
});

test('the empty scope is not — it draws nothing', () => {
  assert.equal(worthRecording(emptyState()), false);
});

test('a scope of nothing but exclusions still draws nothing', () => {
  const only = { ...emptyState(), scope: [{ pattern: 'ui/src/tests', negate: true }] };
  assert.equal(worthRecording(only), false);
});

test('the first-run auto-scope does not put a blank canvas at the bottom of the stack', () => {
  // App opens with no scope and immediately picks one. That first navigation
  // records the empty state, and back would land on a blank canvas.
  const h = record(NO_HISTORY, emptyState());
  assert.equal(canGoBack(h), false);
});

// --- record ---

test('recording pushes the picture being left, most recent last', () => {
  let h = record(NO_HISTORY, view('ui'));
  h = record(h, view('ui/src'));
  assert.deepEqual(paths(h.back), ['ui', 'ui/src']);
});

test('re-applying the picture already on screen costs no back press', () => {
  let h = record(NO_HISTORY, view('ui/src'));
  h = record(h, view('ui/src'));
  assert.deepEqual(paths(h.back), ['ui/src']);
});

test('two states differing only in the order of a captured set are one picture', () => {
  let h = record(NO_HISTORY, view('ui', { hiddenLanguages: ['Rust', 'Go'] }));
  h = record(h, view('ui', { hiddenLanguages: ['Go', 'Rust'] }));
  assert.deepEqual(paths(h.back), ['ui']);
});

test('a picture that differs in a filter is a different picture', () => {
  let h = record(NO_HISTORY, view('ui'));
  h = record(h, view('ui', { hiddenLanguages: ['Rust'] }));
  assert.equal(h.back.length, 2);
});

test('the stack is capped, oldest first out', () => {
  let h: ViewHistory = NO_HISTORY;
  for (let i = 0; i < HISTORY_LIMIT + 10; i++) h = record(h, view(`p${i}`));
  assert.equal(h.back.length, HISTORY_LIMIT);
  assert.equal(paths(h.back)[0], `p${10}`);
});

// --- back and forward ---

test('back returns the previous picture and banks the current one', () => {
  const h = record(NO_HISTORY, view('ui'));
  const step = stepBack(h, view('ui/src/stores'));
  assert.ok(step);
  assert.deepEqual(paths([step.restore]), ['ui']);
  assert.deepEqual(paths(step.history.forward), ['ui/src/stores']);
  assert.equal(canGoBack(step.history), false);
});

test('back at the bottom of the stack is not a step', () => {
  assert.equal(stepBack(NO_HISTORY, view('ui')), null);
  assert.equal(canGoBack(NO_HISTORY), false);
});

test('forward returns what back was pressed from', () => {
  const h = record(NO_HISTORY, view('ui'));
  const back = stepBack(h, view('ui/src'));
  assert.ok(back);
  const fwd = stepForward(back.history, back.restore);
  assert.ok(fwd);
  assert.deepEqual(paths([fwd.restore]), ['ui/src']);
  assert.deepEqual(paths(fwd.history.back), ['ui']);
  assert.equal(canGoForward(fwd.history), false);
});

test('forward returns the reader to what they were looking at, not to the frame that got them there', () => {
  // Drill to ui/src, hide a language there, then press back. Forward has to
  // restore the picture with the language hidden — that is what was on screen.
  const h = record(NO_HISTORY, view('ui'));
  const drifted = view('ui/src', { hiddenLanguages: ['Rust'] });
  const back = stepBack(h, drifted);
  assert.ok(back);
  const fwd = stepForward(back.history, back.restore);
  assert.ok(fwd);
  assert.deepEqual(fwd.restore.hiddenLanguages, ['Rust']);
});

test('forward at the top of the stack is not a step', () => {
  assert.equal(stepForward(NO_HISTORY, view('ui')), null);
});

test('a new navigation makes the forward stack unreachable', () => {
  const h = record(NO_HISTORY, view('ui'));
  const back = stepBack(h, view('ui/src'));
  assert.ok(back);
  assert.equal(canGoForward(back.history), true);
  const after = record(back.history, view('ui'));
  assert.equal(canGoForward(after), false);
});

test('a navigation that coalesces still drops the forward stack', () => {
  // Pressing back then re-drilling to the same place is not a "no move" — the
  // reader chose a direction, and a forward button pointing the other way now
  // lands somewhere they did not come from.
  const h = record(NO_HISTORY, view('ui'));
  const back = stepBack(h, view('ui/src'));
  assert.ok(back);
  const after = record(back.history, view('ui'));
  assert.deepEqual(paths(after.back), ['ui']);
  assert.equal(canGoForward(after), false);
});

test('drill, drill, back, back walks all the way out', () => {
  let h = record(NO_HISTORY, view('ui'));
  h = record(h, view('ui/src'));
  const first = stepBack(h, view('ui/src/stores'));
  assert.ok(first);
  assert.deepEqual(paths([first.restore]), ['ui/src']);
  const second = stepBack(first.history, first.restore);
  assert.ok(second);
  assert.deepEqual(paths([second.restore]), ['ui']);
  assert.equal(canGoBack(second.history), false);
  // Deepest first: forward is a stack too, so the next press goes back in
  // one step to `ui/src` rather than jumping to where the walk started.
  assert.deepEqual(paths(second.history.forward), ['ui/src/stores', 'ui/src']);
  assert.deepEqual(paths([peekForward(second.history)!]), ['ui/src']);
});

// --- what the controls say ---

test('peek names where each button leads', () => {
  const h = record(NO_HISTORY, view('ui'));
  assert.deepEqual(paths([peekBack(h)!]), ['ui']);
  assert.equal(peekForward(h), null);
});

test('a step title names the destination rather than the direction alone', () => {
  assert.match(stepTitle('Back', view('ui/src')), /ui\/src/);
});

test('an empty direction explains itself instead of naming a picture', () => {
  assert.match(stepTitle('Back', null), /first picture/);
  assert.match(stepTitle('Forward', null), /press Back/);
});
