/**
 * Unit tests for the scatter's arithmetic and its drawing ceiling (UI-093).
 *
 * Same zero-dependency setup as the sibling suites — Node's built-in runner
 * plus type stripping. `scatterPoints.ts` imports nothing at all, which is
 * what makes it importable here.
 *
 * The suite that matters is the first one: a population of 200,000 points is
 * exactly what took the Quality tab down, and `Math.max(1, ...points)` throws
 * on it. A test at 1,000 points would have passed against the broken code, so
 * the size is the assertion.
 *
 *   npm run test:scatter
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  axisMax,
  sampleScatter,
  SCATTER_CEILING,
} from '../src/viewmodels/scatterPoints.ts';

/** `n` points on a diagonal, so index, x and y are all distinguishable. */
function points(n: number): { x: number; y: number }[] {
  return Array.from({ length: n }, (_, i) => ({ x: i, y: n - i }));
}

// --- axisMax: the fold that replaced the spread ---------------------------

test('the axis maximum survives a population that overflows a spread', () => {
  const big = points(200_000);
  assert.throws(() => Math.max(1, ...big.map((p) => p.x)), RangeError,
    'precondition: this many arguments must still blow the stack');
  assert.equal(axisMax(big, (p) => p.x), 199_999);
  assert.equal(axisMax(big, (p) => p.y), 200_000);
});

test('an empty population scales to the floor rather than -Infinity', () => {
  assert.equal(axisMax([], (p: { x: number }) => p.x), 1);
});

test('an all-zero population scales to the floor, so no point divides by zero', () => {
  const flat = [{ x: 0, y: 0 }, { x: 0, y: 0 }];
  assert.equal(axisMax(flat, (p) => p.x), 1);
});

test('one NaN metric does not become the maximum', () => {
  const withHole = [{ x: 3 }, { x: NaN }, { x: 7 }];
  assert.equal(axisMax(withHole, (p) => p.x), 7);
});

test('the floor is a floor, not a clamp', () => {
  assert.equal(axisMax([{ x: 40 }], (p) => p.x), 40);
});

// --- sampleScatter: the drawing ceiling -----------------------------------

test('a population under the ceiling is handed back untouched', () => {
  const small = points(50);
  const sample = sampleScatter(small, 100);
  assert.equal(sample.shown, small, 'expected the identical array, not a copy');
  assert.equal(sample.sampled, false);
  assert.equal(sample.total, 50);
});

test('the ceiling is inclusive — exactly at the limit still draws every point', () => {
  const sample = sampleScatter(points(100), 100);
  assert.equal(sample.sampled, false);
  assert.equal(sample.shown.length, 100);
});

test('over the ceiling, the sample stays within it', () => {
  const sample = sampleScatter(points(213_060), SCATTER_CEILING);
  assert.equal(sample.sampled, true);
  assert.equal(sample.total, 213_060);
  assert.ok(sample.shown.length <= SCATTER_CEILING + 2,
    `drew ${sample.shown.length}, ceiling ${SCATTER_CEILING} (+2 extremes)`);
  assert.ok(sample.shown.length > SCATTER_CEILING / 2,
    'a ceiling that draws far fewer than it allows is wasting the plot');
});

test('both axis extremes survive the sample, so the axes are not empty at the top', () => {
  const many = points(213_060);
  const sample = sampleScatter(many, SCATTER_CEILING);
  const maxX = axisMax(many, (p) => p.x);
  const maxY = axisMax(many, (p) => p.y);
  assert.ok(sample.shown.some((p) => p.x === maxX), 'no point at the right edge');
  assert.ok(sample.shown.some((p) => p.y === maxY), 'no point at the top edge');
});

test('the sample is ordered and free of duplicates', () => {
  const sample = sampleScatter(points(50_000), 1_000);
  const xs = sample.shown.map((p) => p.x);
  assert.deepEqual(xs, [...xs].sort((a, b) => a - b), 'points came back out of order');
  assert.equal(new Set(xs).size, xs.length, 'the extremes were added twice');
});

test('the sample is deterministic — the same population draws the same picture', () => {
  const population = points(20_000);
  assert.deepEqual(
    sampleScatter(population, 500).shown,
    sampleScatter(population, 500).shown,
  );
});

test('the sample spans the population rather than its first slice', () => {
  const sample = sampleScatter(points(100_000), 1_000);
  const last = sample.shown[sample.shown.length - 1];
  assert.ok(last.x > 99_000,
    `the sample stops at x=${last.x} — a head slice, not a stride`);
});
