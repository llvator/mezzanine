/**
 * UI-138 — folding what the engine says into what the page shows.
 *
 * The fold has one hard requirement: it is fed by two channels that overlap.
 * A reconnecting page has the `activity` stream coming back *and* a catch-up
 * fetch in flight, so the same notice can arrive twice, out of order, and in
 * either direction. Every test below is some form of "and it still ends up
 * saying the truth about whether the engine is busy".
 *
 * Run: node --experimental-strip-types --test scripts/activity-feed.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  applySnapshot,
  applyUpdate,
  clean,
  elapsed,
  EMPTY_ACTIVITY,
  FEED_LIMIT,
  newestFirst,
  warningCount,
  type ActivityState,
  type Notice,
  type Running,
  type Snapshot,
  type Update,
} from '../src/viewmodels/activityFeed.ts';

const RUNNING: Running = {
  phase: 'diff',
  message: '   Computing structural diff...',
  since_ms: 1_000,
};

function notice(seq: number, over: Partial<Notice> = {}): Notice {
  return {
    seq,
    at_ms: 1_000 + seq,
    kind: 'step',
    phase: 'analysis',
    message: `step ${seq}`,
    ...over,
  };
}

function frame(seq: number, running: Running | null, over: Partial<Notice> = {}): Update {
  return { notice: notice(seq, over), running };
}

/** Feed a state through a list of frames. */
function fold(frames: Update[], from: ActivityState = EMPTY_ACTIVITY): ActivityState {
  return frames.reduce(applyUpdate, from);
}

test('a frame carries both the message and whether anything is still running', () => {
  const state = applyUpdate(EMPTY_ACTIVITY, frame(1, RUNNING));
  assert.equal(state.running?.message, '   Computing structural diff...');
  assert.equal(state.notices.length, 1);
  assert.equal(state.seq, 1);
});

test('an end frame with no running run leaves the page idle', () => {
  const state = fold([frame(1, RUNNING), frame(2, null, { kind: 'end' })]);
  assert.equal(state.running, null, 'the engine said it was done');
  assert.equal(state.notices.length, 2, 'and the end is still in the feed');
});

/**
 * The failure this whole feature exists to prevent, in its nastiest form: a
 * replayed `start` after the run it opened has already been closed. Without
 * the `seq` guard the chip would sit there claiming a finished diff is still
 * going, which is worse than the blank page it replaced — it is wrong rather
 * than merely uninformative.
 */
test('a replayed frame cannot reopen a run that already finished', () => {
  const opened = frame(1, RUNNING, { kind: 'start' });
  const closed = frame(2, null, { kind: 'end' });
  const state = fold([opened, closed, opened]);
  assert.equal(state.running, null, 'the replay was dropped');
  assert.equal(state.seq, 2);
  assert.equal(state.notices.length, 2, 'and did not duplicate the notice');
});

test('an out-of-order frame is dropped rather than applied backwards', () => {
  const state = fold([frame(5, null), frame(3, RUNNING)]);
  assert.equal(state.running, null);
  assert.equal(state.seq, 5);
});

test('a frame with no notice is survived', () => {
  const state = applyUpdate(EMPTY_ACTIVITY, null);
  assert.equal(state, EMPTY_ACTIVITY, 'the same state, untouched');
  assert.equal(applyUpdate(EMPTY_ACTIVITY, {} as Update), EMPTY_ACTIVITY);
});

test('the feed is bounded, oldest dropped first', () => {
  const frames = Array.from({ length: FEED_LIMIT + 40 }, (_, i) => frame(i + 1, RUNNING));
  const state = fold(frames);
  assert.equal(state.notices.length, FEED_LIMIT);
  assert.equal(state.notices[0].seq, 41, 'the oldest 40 went');
  assert.equal(state.seq, FEED_LIMIT + 40, 'and seq still counts them all');
});

// --- catch-up ------------------------------------------------------------

test('a snapshot fills in what the stream never delivered', () => {
  const snap: Snapshot = {
    seq: 3,
    running: RUNNING,
    notices: [notice(1), notice(2), notice(3)],
  };
  const state = applySnapshot(EMPTY_ACTIVITY, snap);
  assert.equal(state.notices.length, 3);
  assert.equal(state.running?.phase, 'diff', 'and says the engine is busy');
  assert.equal(state.seq, 3);
});

test('a snapshot does not re-deliver notices the stream already applied', () => {
  const live = fold([frame(1, RUNNING), frame(2, RUNNING)]);
  const state = applySnapshot(live, {
    seq: 3,
    running: RUNNING,
    notices: [notice(1), notice(2), notice(3)],
  });
  assert.deepEqual(
    state.notices.map((n) => n.seq),
    [1, 2, 3],
    'only the one it had not seen was added',
  );
});

/**
 * The engine's ring is bounded, so a page that was away long enough has
 * missed notices nobody can still produce. `seq` has to jump the gap: asking
 * again from where the page left off would get the same nothing every time,
 * forever.
 */
test('seq follows the engine past notices the ring has dropped', () => {
  const state = applySnapshot(EMPTY_ACTIVITY, {
    seq: 900,
    running: null,
    notices: [notice(898), notice(899), notice(900)],
  });
  assert.equal(state.seq, 900);
});

test('a snapshot that lost the race does not walk seq backwards', () => {
  const live = fold([frame(10, RUNNING)]);
  const state = applySnapshot(live, { seq: 4, running: RUNNING, notices: [notice(2)] });
  assert.equal(state.seq, 10, 'the live frame was newer');
  assert.equal(state.notices.length, 1, 'and the stale notice was not added');
});

test('a snapshot that cannot be read leaves the page as it was', () => {
  const live = fold([frame(1, RUNNING)]);
  assert.equal(applySnapshot(live, null), live);
});

/**
 * `running` comes from the engine on every frame and every snapshot, and is
 * never reconstructed here. This is the test that says so: a snapshot
 * reporting idle wins over a live frame that said otherwise, because it is
 * the newer answer to the same question.
 */
test('a snapshot is authoritative about whether the engine is busy', () => {
  const live = fold([frame(1, RUNNING)]);
  const state = applySnapshot(live, { seq: 2, running: null, notices: [notice(2, { kind: 'end' })] });
  assert.equal(state.running, null);
});

// --- what the panel reads ------------------------------------------------

test('warnings are counted for the badge', () => {
  const state = fold([
    frame(1, RUNNING),
    frame(2, RUNNING, { kind: 'warn' }),
    frame(3, RUNNING, { kind: 'warn' }),
  ]);
  assert.equal(warningCount(state), 2);
});

test('the panel reads newest first', () => {
  const state = fold([frame(1, RUNNING), frame(2, RUNNING), frame(3, RUNNING)]);
  assert.deepEqual(
    newestFirst(state).map((n) => n.seq),
    [3, 2, 1],
  );
  assert.deepEqual(
    state.notices.map((n) => n.seq),
    [1, 2, 3],
    'without disturbing the stored order',
  );
});

test('terminal indentation is dropped, since the chip has no nesting to carry', () => {
  assert.equal(clean('   Computing structural diff...'), 'Computing structural diff...');
  assert.equal(clean('🔄 Starting diff: a → b'), '🔄 Starting diff: a → b');
  assert.equal(clean(undefined as unknown as string), '');
});

test('elapsed reads as a duration at both scales', () => {
  assert.equal(elapsed(0, 0), '0s');
  assert.equal(elapsed(0, 42_000), '42s');
  assert.equal(elapsed(0, 71_900), '1m 12s');
  assert.equal(elapsed(0, 65_000), '1m 05s', 'two digits, so the chip does not jump width');
  assert.equal(elapsed(5_000, 0), '0s', 'a clock skew is not a negative duration');
});
