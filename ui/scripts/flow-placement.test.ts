/**
 * Unit tests for the Flow view's layout (UI-146).
 *
 * The layering is `flowLayers`' business and tested there. What is pinned here
 * is what the *canvas* needs to be true of the coordinates:
 *
 *   - upstream is on the LEFT, so the picture and the axis caption agree;
 *   - every drawn node gets a position, or the View pins it at the origin
 *     under whatever is already there;
 *   - the barycentre pass puts a dependant near its dependencies, and breaks
 *     ties the same way twice.
 *
 *   npm run test:flowplace
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { flowFields, flowPlacement } from '../src/viewmodels/flowPlacement.ts';
import type { FlowEdge } from '../src/viewmodels/flowLayers.ts';

function edges(...spec: string[]): FlowEdge[] {
  return spec.map((s) => {
    const [source, target] = s.split('->');
    return { source, target };
  });
}

test('upstream is on the left', () => {
  // a depends on b depends on c; c is the foundation and belongs at x-min.
  const p = flowPlacement(['a', 'b', 'c'], edges('a->b', 'b->c'));
  const x = (id: string) => p.positions.get(id)!.x;
  assert.ok(x('c') < x('b'), 'foundation left of the middle');
  assert.ok(x('b') < x('a'), 'middle left of the entry point');
});

test('the columns are centred on the origin', () => {
  const p = flowPlacement(['a', 'b', 'c'], edges('a->b', 'b->c'));
  assert.equal(p.positions.get('b')!.x, 0);
  assert.equal(p.positions.get('a')!.x, -p.positions.get('c')!.x);
});

test('the axis names one rung per layer, in reading order', () => {
  const p = flowPlacement(['a', 'b', 'c', 'd'], edges('a->b', 'b->c', 'd->c'));
  assert.deepEqual(p.axis.map((r) => r.layer), [0, 1, 2]);
  assert.deepEqual(p.axis.map((r) => r.count), [1, 2, 1]);
  // The caption sits over the column it names.
  for (const rung of p.axis) {
    const members = p.reading.layers[rung.layer];
    for (const id of members) assert.equal(p.positions.get(id)!.x, rung.x);
  }
});

test('every node gets a position', () => {
  const ids = ['a', 'b', 'c', 'lonely'];
  const p = flowPlacement(ids, edges('a->b', 'b->c'));
  for (const id of ids) assert.ok(p.positions.has(id), `${id} placed`);
});

test('a dependant sits at the height of what it depends on', () => {
  // Two independent chains. `hiA` and `hiB` must not cross over each other on
  // the way to their own foundations.
  const p = flowPlacement(
    ['hiA', 'hiB', 'loA', 'loB'],
    edges('hiA->loA', 'hiB->loB'),
  );
  const y = (id: string) => p.positions.get(id)!.y;
  // Whichever way the foundations sorted, each dependant follows its own.
  const aIsAbove = y('loA') < y('loB');
  assert.equal(y('hiA') < y('hiB'), aIsAbove);
});

test('the same graph lays out the same way twice', () => {
  const spec = edges('a->c', 'b->c', 'c->d', 'e->d');
  const one = flowPlacement(['a', 'b', 'c', 'd', 'e'], spec);
  const two = flowPlacement(['e', 'd', 'c', 'b', 'a'], spec);
  for (const id of ['a', 'b', 'c', 'd', 'e']) {
    assert.deepEqual(one.positions.get(id), two.positions.get(id), id);
  }
});

test('density changes the spacing and nothing else', () => {
  const spec = edges('a->b');
  const compact = flowPlacement(['a', 'b'], spec, 'compact');
  const spacious = flowPlacement(['a', 'b'], spec, 'spacious');
  assert.deepEqual(
    compact.reading.layers,
    spacious.reading.layers,
  );
  assert.ok(
    Math.abs(spacious.positions.get('a')!.x) > Math.abs(compact.positions.get('a')!.x),
  );
});

test('an empty graph places nothing and captions nothing', () => {
  const p = flowPlacement([], []);
  assert.equal(p.positions.size, 0);
  assert.deepEqual(p.axis, []);
});

test('off, the plan fields are exactly force mode as it was', () => {
  // The seam `displayPlan.compute` spreads. Its whole promise is that turning
  // Flow off costs the force path nothing — same mode, no positions, no axis,
  // no marks — so that a fourth mode cannot move the default picture.
  const f = flowFields(false, new Set(['a', 'b']), () => edges('a->b'), 'normal');
  assert.equal(f.mode, 'force');
  assert.equal(f.treePositions.size, 0);
  assert.deepEqual(f.flowAxis, []);
  assert.equal(f.flowCycleIds.size, 0);
});

test('off, it never asks for the edges', () => {
  // The reason the thunk exists: `compute` calls this on every plan it builds,
  // and gathering the flux edges is a pass over the whole link list. Force mode
  // must not pay for it.
  let asked = 0;
  flowFields(false, new Set(['a']), () => { asked++; return []; }, 'normal');
  assert.equal(asked, 0);
});

test('on, it names the cycle members and captions every column', () => {
  const f = flowFields(
    true,
    new Set(['a', 'b', 'base']),
    () => edges('a->b', 'b->a', 'a->base'),
    'normal',
  );
  assert.equal(f.mode, 'flow');
  assert.equal(f.treePositions.size, 3);
  assert.deepEqual(f.flowAxis.map((r) => r.layer), [0, 1]);
  assert.deepEqual([...f.flowCycleIds].sort(), ['a', 'b']);
});
