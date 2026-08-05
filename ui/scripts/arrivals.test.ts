/**
 * Unit tests for the arrival window (UI-066).
 *
 * The clock is injected, so the 30-second window is tested in microseconds.
 * That is the whole reason this logic is not inline in GraphView: the
 * interesting behaviour is *when a mark lapses*, and the only honest way to
 * check that against a real clock is to wait.
 *
 *   npm run test:arrivals
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  newcomers, worthMarking, noteArrivals, pruneArrivals, msUntilNextExpiry,
  ARRIVAL_HIGHLIGHT_MS, ARRIVAL_BURST_LIMIT, type ArrivalLog,
} from '../src/utils/arrivals.ts';

const log = (): ArrivalLog => new Map();

// ── which nodes are new ───────────────────────────────────────────────────

test('newcomers are the ids that were not there before', () => {
  assert.deepEqual(newcomers(new Set(['a', 'b']), ['a', 'b', 'c']), ['c']);
});

test('a node that left is not a newcomer', () => {
  assert.deepEqual(newcomers(new Set(['a', 'b']), ['a']), []);
});

test('the first graph is not an arrival', () => {
  // Every id is new on the first render; marking them all would ring the
  // entire canvas. The View gates on a reload, and the burst limit is the
  // backstop.
  const first = newcomers(new Set(), Array.from({ length: 200 }, (_, i) => `n${i}`));
  assert.equal(first.length, 200);
  assert.equal(worthMarking(first.length), false);
});

// ── what is worth marking ─────────────────────────────────────────────────

test('nothing new marks nothing', () => {
  assert.equal(worthMarking(0), false);
});

test('a handful of arrivals is worth marking', () => {
  assert.equal(worthMarking(1), true);
  assert.equal(worthMarking(ARRIVAL_BURST_LIMIT), true);
});

test('one past the burst limit marks nothing', () => {
  assert.equal(worthMarking(ARRIVAL_BURST_LIMIT + 1), false);
});

// ── the window ────────────────────────────────────────────────────────────

test('a mark holds for the full window', () => {
  const l = noteArrivals(log(), ['a'], 1_000);
  pruneArrivals(l, 1_000 + ARRIVAL_HIGHLIGHT_MS - 1);
  assert.ok(l.has('a'), 'still marked one millisecond short of the deadline');
});

test('a mark lapses at its deadline', () => {
  const l = noteArrivals(log(), ['a'], 1_000);
  pruneArrivals(l, 1_000 + ARRIVAL_HIGHLIGHT_MS);
  assert.equal(l.has('a'), false);
});

test('pruning leaves the marks that have not lapsed', () => {
  const l = log();
  noteArrivals(l, ['early'], 0, 100);
  noteArrivals(l, ['late'], 0, 500);
  pruneArrivals(l, 200);
  assert.deepEqual([...l.keys()], ['late']);
});

test('arriving again restarts the window', () => {
  // A node that leaves and comes back has just arrived, whatever the log
  // thought a moment ago.
  const l = noteArrivals(log(), ['a'], 0, 100);
  noteArrivals(l, ['a'], 90, 100);
  pruneArrivals(l, 150);
  assert.ok(l.has('a'), 're-arrival should have pushed the deadline out');
  assert.equal(l.get('a'), 190);
});

// ── the sweep timer ───────────────────────────────────────────────────────

test('an empty log schedules no sweep', () => {
  assert.equal(msUntilNextExpiry(log(), 0), null);
});

test('the sweep is aimed at the earliest deadline, not the latest', () => {
  const l = log();
  noteArrivals(l, ['late'], 0, 900);
  noteArrivals(l, ['early'], 0, 100);
  assert.equal(msUntilNextExpiry(l, 0), 100);
});

test('an already-lapsed mark asks for an immediate sweep', () => {
  // Rather than a negative delay, which `setTimeout` would fire on anyway
  // but which reads as a bug at the call site.
  const l = noteArrivals(log(), ['a'], 0, 100);
  assert.equal(msUntilNextExpiry(l, 500), 0);
});
