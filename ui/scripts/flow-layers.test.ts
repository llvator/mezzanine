/**
 * Unit tests for the flux reading (UI-146).
 *
 * The rules worth pinning here are the ones a reader would argue with, not
 * the traversal:
 *
 *   - which end is upstream. An edge points at a dependency, so layer 0 is
 *     the thing everything else stands on, and getting this backwards would
 *     invert every panel in the app while still producing a plausible picture;
 *   - what a cycle's layer is. The members share one, and they count in each
 *     other's upstream AND downstream, because both are true of them;
 *   - that an edge with an end outside the population is ignored, so a folder's
 *     internal hierarchy cannot be rearranged by a call into the stdlib.
 *
 *   npm run test:flow
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { flowLayers, type FlowEdge } from '../src/viewmodels/flowLayers.ts';

/** `'a->b'` reads closer to the picture than a pile of object literals. */
function edges(...spec: string[]): FlowEdge[] {
  return spec.map((s) => {
    const [source, target] = s.split('->');
    return { source, target };
  });
}

test('layer 0 is what everything else stands on', () => {
  // a depends on b depends on c. c is the foundation.
  const r = flowLayers(['a', 'b', 'c'], edges('a->b', 'b->c'));
  assert.equal(r.standing.get('c')!.layer, 0);
  assert.equal(r.standing.get('b')!.layer, 1);
  assert.equal(r.standing.get('a')!.layer, 2);
  assert.deepEqual(r.layers, [['c'], ['b'], ['a']]);
});

test('upstream is what it depends on, downstream what depends on it', () => {
  const r = flowLayers(['a', 'b', 'c'], edges('a->b', 'b->c'));
  assert.deepEqual(
    { up: r.standing.get('a')!.upstream, down: r.standing.get('a')!.downstream },
    { up: 2, down: 0 },
  );
  assert.deepEqual(
    { up: r.standing.get('c')!.upstream, down: r.standing.get('c')!.downstream },
    { up: 0, down: 2 },
  );
});

test('roles name the two ends and the middle', () => {
  const r = flowLayers(['a', 'b', 'c', 'lonely'], edges('a->b', 'b->c'));
  assert.equal(r.standing.get('a')!.role, 'entry');
  assert.equal(r.standing.get('b')!.role, 'relay');
  assert.equal(r.standing.get('c')!.role, 'foundation');
  assert.equal(r.standing.get('lonely')!.role, 'isolated');
});

test('a diamond puts both middles on the same layer', () => {
  // top depends on left and right; both depend on base.
  const r = flowLayers(
    ['top', 'left', 'right', 'base'],
    edges('top->left', 'top->right', 'left->base', 'right->base'),
  );
  assert.equal(r.standing.get('base')!.layer, 0);
  assert.equal(r.standing.get('left')!.layer, 1);
  assert.equal(r.standing.get('right')!.layer, 1);
  assert.equal(r.standing.get('top')!.layer, 2);
  // Counted once each, not once per path through the diamond.
  assert.equal(r.standing.get('top')!.upstream, 3);
  assert.equal(r.standing.get('base')!.downstream, 3);
});

test('the layer is the LONGEST chain below, not the shortest', () => {
  // top depends on base directly AND through mid. The short path must not
  // pull it down beside mid — it stands on a two-deep chain.
  const r = flowLayers(['top', 'mid', 'base'], edges('top->base', 'top->mid', 'mid->base'));
  assert.equal(r.standing.get('top')!.layer, 2);
});

test('a cycle shares one layer and counts both ways', () => {
  // a and b need each other; both stand on base; top stands on a.
  const r = flowLayers(
    ['top', 'a', 'b', 'base'],
    edges('top->a', 'a->b', 'b->a', 'a->base', 'b->base'),
  );
  const a = r.standing.get('a')!;
  const b = r.standing.get('b')!;
  assert.equal(a.layer, b.layer);
  assert.equal(a.layer, 1);
  assert.equal(r.standing.get('top')!.layer, 2);
  // Each other's mate, counted on both sides: a reaches b and b reaches a.
  assert.deepEqual(a.cycle, ['b']);
  assert.deepEqual(b.cycle, ['a']);
  assert.equal(a.upstream, 2);   // b and base
  assert.equal(a.downstream, 2); // b and top
  assert.deepEqual(r.cycles, [['a', 'b']]);
});

test('a three-member cycle is one group, not three pairs', () => {
  const r = flowLayers(['x', 'y', 'z'], edges('x->y', 'y->z', 'z->x'));
  assert.deepEqual(r.cycles, [['x', 'y', 'z']]);
  assert.equal(r.layers.length, 1);
  assert.equal(r.standing.get('x')!.upstream, 2);
  assert.equal(r.standing.get('x')!.downstream, 2);
});

test('an edge with an end outside the population is ignored', () => {
  // `stdlib` is not in the node set — a call into it must not make `a` a
  // relay, nor add a layer.
  const r = flowLayers(['a', 'b'], edges('a->b', 'a->stdlib', 'stdlib->b'));
  assert.equal(r.layers.length, 2);
  assert.equal(r.standing.get('a')!.role, 'entry');
  assert.equal(r.standing.get('a')!.upstream, 1);
});

test('a self-loop is not a hierarchy of one', () => {
  const r = flowLayers(['rec'], edges('rec->rec'));
  assert.equal(r.standing.get('rec')!.layer, 0);
  assert.equal(r.standing.get('rec')!.role, 'isolated');
  assert.equal(r.standing.get('rec')!.cycle, null);
  assert.deepEqual(r.cycles, []);
});

test('a repeated pair is one relationship, not many', () => {
  const r = flowLayers(['a', 'b'], edges('a->b', 'a->b', 'a->b'));
  assert.equal(r.standing.get('a')!.upstream, 1);
  assert.equal(r.standing.get('b')!.downstream, 1);
});

test('every input id gets a standing, edges or not', () => {
  const r = flowLayers(['solo'], []);
  assert.equal(r.standing.size, 1);
  assert.deepEqual(r.layers, [['solo']]);
  assert.equal(r.standing.get('solo')!.role, 'isolated');
});

test('an empty population reads as empty, not as one layer of nothing', () => {
  const r = flowLayers([], edges('a->b'));
  assert.deepEqual(r.layers, []);
  assert.equal(r.standing.size, 0);
});

test('the reading is the same whatever order the nodes arrive in', () => {
  const spec = edges('a->b', 'b->c', 'd->b');
  const one = flowLayers(['a', 'b', 'c', 'd'], spec);
  const two = flowLayers(['d', 'c', 'b', 'a'], spec);
  assert.deepEqual(one.layers, two.layers);
  for (const id of ['a', 'b', 'c', 'd']) {
    assert.deepEqual(one.standing.get(id), two.standing.get(id));
  }
});

test('a long chain does not overflow the stack', () => {
  // Iterative Tarjan is the reason this is a test and not a crash: the
  // recursion depth would be the chain length.
  const n = 20000;
  const ids = Array.from({ length: n }, (_, i) => `n${i}`);
  const chain: FlowEdge[] = [];
  for (let i = 0; i + 1 < n; i++) chain.push({ source: `n${i}`, target: `n${i + 1}` });
  const r = flowLayers(ids, chain);
  assert.equal(r.layers.length, n);
  assert.equal(r.standing.get('n0')!.upstream, n - 1);
  assert.equal(r.standing.get(`n${n - 1}`)!.downstream, n - 1);
});

test('reachability is exact past the 32-component word boundary', () => {
  // One chain of 40 nodes: every component sits in a different bitset word
  // from most of the others, so an off-by-one in the word loop shows up as a
  // count that is right for the first 32 and wrong after.
  const n = 40;
  const ids = Array.from({ length: n }, (_, i) => `c${i}`);
  const chain: FlowEdge[] = [];
  for (let i = 0; i + 1 < n; i++) chain.push({ source: `c${i}`, target: `c${i + 1}` });
  const r = flowLayers(ids, chain);
  for (let i = 0; i < n; i++) {
    assert.equal(r.standing.get(`c${i}`)!.upstream, n - 1 - i, `upstream of c${i}`);
    assert.equal(r.standing.get(`c${i}`)!.downstream, i, `downstream of c${i}`);
  }
});
