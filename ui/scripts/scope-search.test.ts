/**
 * Unit tests for the two pieces that keep the scope search box responsive on
 * a large repo (UI-136): bounded ranking, and the trailing store that moves
 * the search off the keystroke.
 *
 * Same zero-dependency setup as the sibling suites. Both modules under test
 * are leaves — `topMatches` imports nothing, `trailingStore` imports only
 * `svelte/store` — which is what makes them testable here at all; the
 * viewmodel that wires them together reaches for the graph stores and
 * cannot be loaded by this runner.
 *
 *   npm run test:scopesearch
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { writable, get } from 'svelte/store';

import { bestMatches, ranksBefore } from '../src/utils/topMatches.ts';
import type { Ranked } from '../src/utils/topMatches.ts';
import { trailing } from '../src/utils/trailingStore.ts';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** What `bestMatches` is a cheaper route to. Any disagreement between the
 *  two is a bug in the cheap one, so the tests below compare against this
 *  rather than against hand-written expectations. */
function fullSort<T extends Ranked>(matches: T[], k: number): T[] {
  return [...matches]
    .sort((a, b) => b.score - a.score || a.path.localeCompare(b.path))
    .slice(0, k);
}

function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// --- bestMatches -----------------------------------------------------------

test('bestMatches agrees with a full sort, including on ties', () => {
  const rand = mulberry32(0xB0A7);
  for (let iter = 0; iter < 500; iter++) {
    const n = Math.floor(rand() * 60);
    const matches: Ranked[] = Array.from({ length: n }, (_, i) => ({
      // A deliberately small score range: ties are the case where a
      // selection algorithm and a sort most easily disagree, so they have to
      // be common here rather than incidental.
      score: Math.floor(rand() * 5),
      path: `pkg/file${i % 20}.ts`,
    }));
    const k = 1 + Math.floor(rand() * 12);
    assert.deepEqual(bestMatches(matches, k), fullSort(matches, k),
      `iter ${iter}, k=${k}`);
  }
});

test('bestMatches handles the degenerate bounds', () => {
  const one = [{ score: 1, path: 'a' }];
  assert.deepEqual(bestMatches([], 10), []);
  assert.deepEqual(bestMatches(one, 0), []);
  assert.deepEqual(bestMatches(one, 10), one, 'fewer matches than the cap');
});

test('bestMatches keeps the strongest, not the first seen', () => {
  const matches = [
    { score: 1, path: 'weak.ts' },
    { score: 9, path: 'strong.ts' },
    { score: 5, path: 'middling.ts' },
  ];
  assert.deepEqual(bestMatches(matches, 2).map((m) => m.path),
    ['strong.ts', 'middling.ts']);
});

test('ranksBefore breaks score ties by path', () => {
  assert.ok(ranksBefore({ score: 2, path: 'z' }, { score: 1, path: 'a' }));
  assert.ok(ranksBefore({ score: 1, path: 'a' }, { score: 1, path: 'b' }));
  assert.ok(!ranksBefore({ score: 1, path: 'b' }, { score: 1, path: 'a' }));
});

// --- trailing --------------------------------------------------------------

test('trailing starts at its source value rather than undefined', () => {
  const src = writable('seed');
  assert.equal(get(trailing(src, 50)), 'seed');
});

test('trailing publishes only the last of a burst', async () => {
  const src = writable('');
  const out = trailing(src, 30);
  const seen: string[] = [];
  out.subscribe((v) => seen.push(v));

  for (const v of ['g', 'gr', 'gra', 'grap', 'graph']) src.set(v);
  assert.deepEqual(seen, [''], 'nothing published while the burst is in flight');

  await sleep(60);
  assert.deepEqual(seen, ['', 'graph'], 'the intermediate prefixes never land');
});

test('trailing skips the wait when immediateWhen says so', async () => {
  const src = writable('graph');
  const out = trailing(src, 1000, (v) => v === '');
  const seen: string[] = [];
  out.subscribe((v) => seen.push(v));

  src.set('');
  assert.deepEqual(seen, ['graph', ''], 'clearing must not wait out the debounce');
});

test('trailing cancels a pending publish that is undone', async () => {
  const src = writable('a');
  const out = trailing(src, 30);
  const seen: string[] = [];
  out.subscribe((v) => seen.push(v));

  src.set('ab');
  src.set('a'); // back to what is already published
  await sleep(60);
  assert.deepEqual(seen, ['a'], 'no redundant republish of the current value');
});

test('flush publishes the pending value synchronously', () => {
  // The Enter-beats-the-debounce case: `commitQuery` flushes before reading
  // the match set, so a fast typist scopes to what is on screen and not to
  // the prefix that was current a moment earlier.
  const src = writable('gra');
  const out = trailing(src, 1000);
  src.set('graph');
  assert.equal(get(out), 'gra', 'precondition: still trailing');

  out.flush();
  assert.equal(get(out), 'graph');
});

test('flush is a no-op when nothing is pending', async () => {
  const src = writable('graph');
  const out = trailing(src, 30);
  const seen: string[] = [];
  out.subscribe((v) => seen.push(v));

  out.flush();
  out.flush();
  await sleep(60);
  assert.deepEqual(seen, ['graph'], 'flushing an idle store publishes nothing');
});

test('a flushed value is not republished when the timer would have fired', async () => {
  const src = writable('a');
  const out = trailing(src, 30);
  const seen: string[] = [];
  out.subscribe((v) => seen.push(v));

  src.set('ab');
  out.flush();
  await sleep(60);
  assert.deepEqual(seen, ['a', 'ab'], 'the cancelled timer must not fire after a flush');
});
