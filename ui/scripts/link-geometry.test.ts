/**
 * Unit tests for edge geometry — arrow-head placement and link thickness.
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

import { ARROW_GAP, ARROW_LEN, arrowHeadPoint, linkStrokeWidth } from '../src/viewmodels/linkGeometry.ts';

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
