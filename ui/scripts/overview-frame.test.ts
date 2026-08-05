/**
 * Unit tests for the overview panel's geometry.
 *
 * What makes this worth a suite: the panel's whole job is to be *correct
 * about where you are*, and a wrong box is not a wrong-looking box — it is a
 * confident answer to the one question the panel exists to answer. The
 * failure modes are all off-by-an-inversion (mapping world→screen where
 * screen→world was meant) and all of them look plausible on screen: the box
 * still moves when you pan, just to the wrong place.
 *
 * The round-trip tests are the ones that matter. `overviewToWorld` is the
 * inverse of the projection the dots are drawn with, and a click has to land
 * where the picture says it will.
 *
 * Same zero-dependency setup as the sibling suites: `overviewFrame` imports
 * nothing, so this file can reach it under `node --test` without pulling d3
 * or a Svelte store in behind it.
 *
 *   npm run test:overview
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  viewportWorldRect, dotsExtent, unionRect, overviewWorld, fitWorld,
  projectRect, overviewToWorld, centreTransform, dotRadius, MIN_DOT_R,
  type OverviewDot,
} from '../src/viewmodels/overviewFrame.ts';

const dot = (x: number, y: number, r = 10): OverviewDot =>
  ({ x, y, r, fill: '#fff', opacity: 1 });

// ── The viewport rectangle ───────────────────────────────────────────────

test('the identity transform shows exactly the viewport, at the origin', () => {
  const r = viewportWorldRect({ k: 1, x: 0, y: 0 }, 800, 600);
  assert.deepEqual(r, { minX: 0, minY: 0, maxX: 800, maxY: 600 });
});

test('zooming in halves what is on screen, in world units', () => {
  const r = viewportWorldRect({ k: 2, x: 0, y: 0 }, 800, 600);
  assert.equal(r.maxX - r.minX, 400);
  assert.equal(r.maxY - r.minY, 300);
});

test('the translation is inverted, not applied', () => {
  // d3 puts world (0,0) at screen (100, 50). So the world point at screen
  // (0,0) — the top-left of what you can see — is at world (-100, -50).
  const r = viewportWorldRect({ k: 1, x: 100, y: 50 }, 800, 600);
  assert.deepEqual(r, { minX: -100, minY: -50, maxX: 700, maxY: 550 });
});

test('a zero scale does not divide by zero', () => {
  const r = viewportWorldRect({ k: 0, x: 0, y: 0 }, 800, 600);
  for (const v of Object.values(r)) assert.ok(Number.isFinite(v));
});

// ── Extents ──────────────────────────────────────────────────────────────

test('a dot contributes its circle, not just its centre', () => {
  assert.deepEqual(dotsExtent([dot(0, 0, 10)]), { minX: -10, minY: -10, maxX: 10, maxY: 10 });
});

test('no dots is null rather than an empty rectangle at the origin', () => {
  assert.equal(dotsExtent([]), null);
});

test('union tolerates either side being absent', () => {
  const a = { minX: 0, minY: 0, maxX: 1, maxY: 1 };
  assert.deepEqual(unionRect(a, null), a);
  assert.deepEqual(unionRect(null, a), a);
  assert.equal(unionRect(null, null), null);
});

// ── The panel always holds the box ───────────────────────────────────────

test('panning far off the graph keeps the box inside the panel', () => {
  // One small graph near the origin, and a viewport a long way south-east of
  // it. Fitted to the nodes alone the box would be projected way outside the
  // panel; the union is what stops that.
  const dots = [dot(0, 0), dot(50, 50)];
  const view = { minX: 4000, minY: 3000, maxX: 4800, maxY: 3600 };
  const world = overviewWorld(dots, view);
  assert.ok(world);

  const fit = fitWorld(world, 176, 124, 5);
  const box = projectRect(view, fit);

  assert.ok(box.x >= 0, `box left ${box.x} escaped the panel`);
  assert.ok(box.y >= 0, `box top ${box.y} escaped the panel`);
  assert.ok(box.x + box.width <= 176 + 0.001, 'box right escaped the panel');
  assert.ok(box.y + box.height <= 124 + 0.001, 'box bottom escaped the panel');
});

test('the graph is still in the panel when the viewport has run off', () => {
  const dots = [dot(0, 0), dot(50, 50)];
  const view = { minX: 4000, minY: 3000, maxX: 4800, maxY: 3600 };
  const fit = fitWorld(overviewWorld(dots, view)!, 176, 124, 5);
  const graph = projectRect(dotsExtent(dots)!, fit);
  assert.ok(graph.x >= 0 && graph.x + graph.width <= 176);
  assert.ok(graph.y >= 0 && graph.y + graph.height <= 124);
});

// ── The fit ──────────────────────────────────────────────────────────────

test('one scale on both axes, so the panel is not a stretched graph', () => {
  // A world twice as wide as it is tall, into a panel that is not.
  const fit = fitWorld({ minX: 0, minY: 0, maxX: 200, maxY: 100 }, 176, 124, 5);
  const box = projectRect({ minX: 0, minY: 0, maxX: 200, maxY: 100 }, fit);
  assert.ok(Math.abs(box.width / box.height - 2) < 0.001, 'aspect ratio was not preserved');
});

test('the fitted world is centred in the panel', () => {
  const world = { minX: 0, minY: 0, maxX: 200, maxY: 100 };
  const fit = fitWorld(world, 176, 124, 5);
  const box = projectRect(world, fit);
  // Width is the binding axis here, so the slack is vertical and even.
  assert.ok(Math.abs(box.x - (176 - box.width) / 2) < 0.001);
  assert.ok(Math.abs(box.y - (124 - box.height) / 2) < 0.001);
});

test('padding is honoured on the binding axis', () => {
  const world = { minX: 0, minY: 0, maxX: 200, maxY: 100 };
  const box = projectRect(world, fitWorld(world, 176, 124, 5));
  assert.ok(Math.abs(box.x - 5) < 0.001, `expected the 5px pad, got ${box.x}`);
});

test('a single node does not divide by zero', () => {
  const world = dotsExtent([dot(500, 500, 0)])!;
  const fit = fitWorld(world, 176, 124, 5);
  assert.ok(Number.isFinite(fit.scale) && fit.scale > 0);
  const box = projectRect(world, fit);
  for (const v of Object.values(box)) assert.ok(Number.isFinite(v));
});

// ── The round trip ───────────────────────────────────────────────────────

test('a click where a dot is drawn resolves to that dot', () => {
  const dots = [dot(-300, 120), dot(640, -75), dot(0, 0)];
  const fit = fitWorld(overviewWorld(dots, null)!, 176, 124, 5);
  for (const d of dots) {
    const back = overviewToWorld(d.x * fit.scale + fit.offsetX, d.y * fit.scale + fit.offsetY, fit);
    assert.ok(Math.abs(back.x - d.x) < 1e-6, `x round trip: ${back.x} vs ${d.x}`);
    assert.ok(Math.abs(back.y - d.y) < 1e-6, `y round trip: ${back.y} vs ${d.y}`);
  }
});

test('clicking the centre of the box leaves the viewport where it is', () => {
  const dots = [dot(0, 0), dot(1000, 800)];
  const t = { k: 1.7, x: -220, y: -140 };
  const view = viewportWorldRect(t, 800, 600);
  const fit = fitWorld(overviewWorld(dots, view)!, 176, 124, 5);

  // Press dead centre of the drawn box…
  const box = projectRect(view, fit);
  const w = overviewToWorld(box.x + box.width / 2, box.y + box.height / 2, fit);
  // …and the transform that centres it should be the one already in force.
  const next = centreTransform(w.x, w.y, t.k, 800, 600);
  assert.ok(Math.abs(next.x - t.x) < 1e-6, `x moved: ${next.x} vs ${t.x}`);
  assert.ok(Math.abs(next.y - t.y) < 1e-6, `y moved: ${next.y} vs ${t.y}`);
  assert.equal(next.k, t.k);
});

test('centring puts the asked-for world point in the middle of the screen', () => {
  const next = centreTransform(1234, -56, 2.5, 800, 600);
  const view = viewportWorldRect(next, 800, 600);
  assert.ok(Math.abs((view.minX + view.maxX) / 2 - 1234) < 1e-6);
  assert.ok(Math.abs((view.minY + view.maxY) / 2 - -56) < 1e-6);
});

test('a pan never changes the zoom', () => {
  assert.equal(centreTransform(0, 0, 3.3, 800, 600).k, 3.3);
});

// ── Dots ─────────────────────────────────────────────────────────────────

test('a dot too small to see is drawn at the floor, not sub-pixel', () => {
  // A whole-repo graph shrunk into the panel puts most radii well under 1px.
  assert.equal(dotRadius(7, { scale: 0.02, offsetX: 0, offsetY: 0 }), MIN_DOT_R);
});

test('the floor does not flatten dots that are big enough to differ', () => {
  const fit = { scale: 0.5, offsetX: 0, offsetY: 0 };
  assert.ok(dotRadius(34, fit) > dotRadius(7, fit));
});
