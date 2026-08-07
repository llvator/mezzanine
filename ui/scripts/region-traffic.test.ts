/**
 * Unit tests for what a region says about its own relationships (UI-071).
 *
 * The counting rules are the whole feature, and two of them are arguable
 * enough to be worth pinning down here rather than in a component:
 *
 *   - a link is crossing *for one region and inside for its parent*, which is
 *     what makes the nested trail worth reading;
 *   - a link into a ghost is neither, because "crossing" is meant to say this
 *     goes elsewhere in the repo, and an import of the standard library says
 *     nothing about whether a folder is a subsystem.
 *
 *   npm run test:traffic
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  membershipText,
  membershipTitle,
  regionTraffic,
  trafficSentence,
  type TrafficLink,
} from '../src/viewmodels/regionTraffic.ts';

/** `ui/src/stores/a.ts` → its ancestor chain, innermost first. A ghost — an
 *  id with no slash — belongs to no region, as `folderKeyOf` decides. */
function chainOf(id: string): string[] {
  const slash = id.lastIndexOf('/');
  if (slash < 0) return [];
  const dir = id.slice(0, slash);
  if (dir === '') return [];
  const out: string[] = [];
  const parts = dir.split('/');
  for (let i = parts.length; i > 0; i--) out.push(parts.slice(0, i).join('/'));
  return out;
}

function run(ids: string[], links: TrafficLink[], hidden: string[] = []) {
  const hide = new Set(hidden);
  return regionTraffic({
    candidates: ids.map((id) => ({ id })),
    isDrawn: (id) => !hide.has(id),
    links,
    keysOf: chainOf,
  });
}

const A = 'ui/src/stores/a.ts';
const B = 'ui/src/stores/b.ts';
const C = 'ui/src/viewmodels/c.ts';
const GHOST = 'println';

// --- membership ---

test('a region counts every node beneath it, at every level of the chain', () => {
  const t = run([A, B, C], []);
  assert.equal(t.get('ui/src/stores')?.total, 2);
  assert.equal(t.get('ui/src/viewmodels')?.total, 1);
  assert.equal(t.get('ui/src')?.total, 3);
  assert.equal(t.get('ui')?.total, 3);
});

test('drawn and total differ exactly by what a filter hid', () => {
  const t = run([A, B, C], [], [B]);
  assert.deepEqual(
    [t.get('ui/src/stores')?.drawn, t.get('ui/src/stores')?.total],
    [1, 2],
  );
});

test('a node in no region contributes to none', () => {
  const t = run([A, GHOST], []);
  assert.equal(t.get('ui/src/stores')?.total, 1);
  assert.equal(t.size, 3); // stores, src, ui — and nothing for the ghost
});

// --- inside versus crossing ---

test('both ends in the region is inside', () => {
  const t = run([A, B], [{ source: A, target: B }]);
  assert.deepEqual(
    [t.get('ui/src/stores')?.inside, t.get('ui/src/stores')?.crossing],
    [1, 0],
  );
});

test('one link is crossing for the folder and inside for its parent', () => {
  // The claim the nested trail exists to make: coupling contained one level
  // up is not the same as coupling contained here.
  const t = run([A, C], [{ source: A, target: C }]);
  assert.deepEqual(
    [t.get('ui/src/stores')?.inside, t.get('ui/src/stores')?.crossing],
    [0, 1],
  );
  assert.deepEqual(
    [t.get('ui/src/viewmodels')?.inside, t.get('ui/src/viewmodels')?.crossing],
    [0, 1],
  );
  assert.deepEqual([t.get('ui/src')?.inside, t.get('ui/src')?.crossing], [1, 0]);
});

test('a region counts links pointing at it, not only links leaving it', () => {
  const t = run([A, C], [{ source: C, target: A }]);
  assert.equal(t.get('ui/src/stores')?.crossing, 1);
});

test('a link to a ghost is neither inside nor crossing', () => {
  // Otherwise every folder that imports anything looks leaky, and the number
  // stops separating the case it exists to separate.
  const t = run([A, GHOST], [{ source: A, target: GHOST }]);
  assert.deepEqual(
    [t.get('ui/src/stores')?.inside, t.get('ui/src/stores')?.crossing],
    [0, 0],
  );
});

test('a self-link is inside', () => {
  const t = run([A], [{ source: A, target: A }]);
  assert.equal(t.get('ui/src/stores')?.inside, 1);
});

test('two links between the same folders each count', () => {
  const t = run([A, B, C], [
    { source: A, target: C },
    { source: B, target: C },
  ]);
  assert.equal(t.get('ui/src/stores')?.crossing, 2);
  assert.equal(t.get('ui/src')?.inside, 2);
});

// --- what the card says ---

test('the sentence names the region and never states a ratio', () => {
  const s = trafficSentence('stores', { drawn: 2, total: 2, inside: 21, crossing: 13 });
  assert.equal(s, '21 of 34 relationships stay inside stores');
  assert.doesNotMatch(s, /%/);
  assert.doesNotMatch(s, /cohesion/i);
});

test('a region with nothing crossing says so outright', () => {
  assert.match(trafficSentence('stores', { drawn: 2, total: 2, inside: 4, crossing: 0 }),
    /^All 4 relationships here stay inside stores$/);
});

test('a region with nothing inside says that outright too', () => {
  assert.match(trafficSentence('types', { drawn: 9, total: 9, inside: 0, crossing: 17 }),
    /^All 17 relationships here cross out of types$/);
});

test('a region with no drawn relationships says nothing was drawn', () => {
  assert.match(trafficSentence('docs', { drawn: 3, total: 3, inside: 0, crossing: 0 }),
    /No relationships drawn/);
});

test('membership is silent when nothing is hidden', () => {
  assert.equal(membershipText({ drawn: 19, total: 19, inside: 0, crossing: 0 }, 0), '19');
});

test('membership speaks up when a filter is hiding members', () => {
  assert.equal(membershipText({ drawn: 12, total: 19, inside: 0, crossing: 0 }, 0), '12 of 19');
  assert.match(membershipTitle({ drawn: 12, total: 19, inside: 0, crossing: 0 }),
    /filtered, not sparse/);
});

test('a region the traffic pass never saw falls back to the hull s own count', () => {
  assert.equal(membershipText(undefined, 7), '7');
});
