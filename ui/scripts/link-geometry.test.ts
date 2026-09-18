/**
 * Unit tests for edge geometry — arrow-head placement, link thickness, and
 * where a mark that rides on an edge is allowed to sit.
 *
 * What makes this worth a suite rather than a look at the canvas: the defect
 * it fixes was invisible at file level and obvious at module level, because
 * both inputs (merged edge weight, node radius) only reach their extremes
 * once collapsing merges hundreds of relationships into one link. A canvas
 * check at the wrong aggregation level says everything is fine.
 *
 * Same zero-dependency setup as the sibling suites, and the same
 * extensionless-import trap: `linkGeometry` is its own module so this file
 * can reach it without pulling d3 or a store in behind it.
 *
 *   npm run test:linkgeom
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  ARROW_GAP, ARROW_LEN, EDGE_MARK_GAP, ORDER_BADGE_R,
  arrowHeadPoint, edgeAnchorPoint, linkStrokeWidth,
} from '../src/viewmodels/linkGeometry.ts';

/** Distance from a tail at the origin to an anchor on its line. */
const along = (p: { x: number; y: number }) => Math.hypot(p.x, p.y);

test('the head stops one gap short of the rim, not at the centre', () => {
  // Tail at the origin, head node of radius 20 centred 100 px to the right.
  const p = arrowHeadPoint(0, 0, 100, 0, 20);
  assert.equal(p.x, 100 - 20 - ARROW_GAP);
  assert.equal(p.y, 0);
});

test('the setback follows the node radius, which the size channel moves', () => {
  // The old code used a constant refX for every link. Across the encoding's
  // 7-34 px radius range that is wrong at both ends by the same 27 px the
  // channel spans, so the two extremes must not agree here.
  const small = arrowHeadPoint(0, 0, 200, 0, 7);
  const large = arrowHeadPoint(0, 0, 200, 0, 34);
  assert.equal(small.x - large.x, 34 - 7);
});

test('the setback is measured along the line, not along an axis', () => {
  // 3-4-5 triangle: head 100 px away on the diagonal, radius+gap = 50.
  const p = arrowHeadPoint(0, 0, 60, 80, 50 - ARROW_GAP);
  assert.equal(Math.round(Math.hypot(p.x, p.y)), 50);
});

test('overlapping nodes never push the head behind the tail', () => {
  // Radius far exceeds the distance — trimming naively would put the head at
  // x = -170 and render the arrow pointing backwards down the line.
  const p = arrowHeadPoint(0, 0, 30, 0, 200);
  assert.ok(p.x > 0, `expected the head to stay ahead of the tail, got ${p.x}`);
  assert.ok(p.x <= 30);
});

test('a degenerate line is left alone', () => {
  const p = arrowHeadPoint(50, 50, 50, 50, 20);
  assert.deepEqual(p, { x: 50, y: 50 });
});

// ── Edge-anchored marks (order badges, kind labels) ────────────────────
//
// The defect these cover: the badge was placed at a flat 25 % of the
// centre-to-centre line. Node radius is a user-chosen channel, so turning the
// size scale up in the legend grows the tail circle past that mark and the
// number disappears under it — no movement on screen to explain where it
// went. Every case below is one the fraction alone gets wrong.

const CLEAR = ORDER_BADGE_R + EDGE_MARK_GAP;

test('a roomy edge keeps the badge at the fraction it asked for', () => {
  // 300 px apart, small circles: 25 % is nowhere near either rim, so the
  // clamp must not move it and the picture must not change.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 10 }, { x: 300, y: 0, radius: 10 }, 0.25, CLEAR);
  assert.equal(p.x, 75);
  assert.equal(p.y, 0);
});

test('a tail circle grown past the fraction pushes the badge off its rim', () => {
  // 120 px apart puts the flat 25 % mark at x = 30. A 34 px tail radius —
  // the top of the encoding's range — covers it whole.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 34 }, { x: 120, y: 0, radius: 10 }, 0.25, CLEAR);
  assert.equal(p.x, 34 + CLEAR);
  assert.ok(p.x > 34, 'the badge must clear the rim it was buried under');
});

test('the head circle cannot swallow a mark aimed at the middle', () => {
  // The kind label sits at 50 %; a hub as the head node reaches past it.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 8 }, { x: 100, y: 0, radius: 60 }, 0.5, CLEAR);
  assert.equal(p.x, 100 - 60 - CLEAR);
});

test('the clamp follows the rim as the size scale moves', () => {
  // Same edge, two points on the size channel: the anchor has to travel with
  // the radius, which is the whole reason a fraction could not do the job.
  const small = edgeAnchorPoint({ x: 0, y: 0, radius: 20 }, { x: 120, y: 0, radius: 10 }, 0.25, CLEAR);
  const large = edgeAnchorPoint({ x: 0, y: 0, radius: 34 }, { x: 120, y: 0, radius: 10 }, 0.25, CLEAR);
  assert.equal(large.x - small.x, 34 - 20);
});

test('the clamp is measured along the line, not along an axis', () => {
  // 3-4-5 again: a 50 px tail radius on a 100 px diagonal edge.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 50 - EDGE_MARK_GAP - ORDER_BADGE_R }, { x: 60, y: 80, radius: 5 }, 0.25, CLEAR);
  assert.equal(Math.round(along(p)), 50);
});

test('two circles that leave no free line still get a placed mark', () => {
  // Overlapping discs: there is no uncovered spot, so the anchor lands
  // between them rather than shooting off the end of the line.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 50 }, { x: 60, y: 0, radius: 50 }, 0.25, CLEAR);
  assert.ok(p.x >= 0 && p.x <= 60, `expected a point on the line, got ${p.x}`);
});

test('a mark never lands past either end of its own edge', () => {
  // A tail hub far larger than the edge is long: clamping to its rim alone
  // would put the badge beyond the head node entirely.
  for (const [tailR, headR] of [[200, 0], [0, 200], [200, 200]]) {
    const p = edgeAnchorPoint({ x: 0, y: 0, radius: tailR }, { x: 30, y: 0, radius: headR }, 0.25, CLEAR);
    assert.ok(p.x >= 0 && p.x <= 30, `radii ${tailR}/${headR} put the mark at ${p.x}`);
  }
});

test('a degenerate edge falls back to the plain fraction', () => {
  const p = edgeAnchorPoint({ x: 50, y: 50, radius: 20 }, { x: 50, y: 50, radius: 20 }, 0.25, CLEAR);
  assert.deepEqual(p, { x: 50, y: 50 });
});

test('the badge clearance is read off the disc actually drawn', () => {
  // The clearance arithmetic is only right while it uses the badge's own
  // radius; a constant here would drift the first time the disc is resized.
  const p = edgeAnchorPoint({ x: 0, y: 0, radius: 40 }, { x: 100, y: 0, radius: 5 }, 0.25, CLEAR);
  assert.ok(p.x - 40 >= ORDER_BADGE_R, 'the whole disc must clear the rim');
});

test('stroke width still ranks weights, within a range an arrow can sit on', () => {
  assert.equal(linkStrokeWidth(1), linkStrokeWidth(null));
  assert.ok(linkStrokeWidth(8) > linkStrokeWidth(2));
  assert.ok(linkStrokeWidth(2) > linkStrokeWidth(1));
});

test('the heaviest module edges stay thinner than the arrow they carry', () => {
  // 377 is the top merged weight in this repo at Folder level; the old ramp
  // put it at 6 px, and with `markerUnits="strokeWidth"` that alone made the
  // head 36 px — larger than the biggest module circle's radius.
  assert.ok(
    linkStrokeWidth(377) < ARROW_LEN / 2,
    `a link should not out-weigh its own arrow head, got ${linkStrokeWidth(377)}`,
  );
});
