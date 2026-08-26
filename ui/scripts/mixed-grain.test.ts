/**
 * Rings: one picture drawn at more than one grain (UI-104).
 *
 * The level used to be a property of the picture, so reading eight entities
 * meant drawing four hundred. A ring plan spends the render budget where the
 * reader is looking — the focus and its neighbours as entities, the rest
 * folded into the file or the directory that holds it.
 *
 * The rule most of this file is about is that **a scope is atomic**. Rings are
 * drawn over a graph and a picture is drawn over a tree, and the two
 * disagree: a file can hold one entity the focus calls and another nothing
 * reaches. Drawing the first as a circle and folding the second into a
 * directory would put an entity on screen and a Folder node containing it —
 * the same code twice, once inside the other.
 *
 *   npm run test:rings
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { collapseGraph, grainFromLevel } from '../src/viewmodels/collapseGraph.ts';
import {
  DEFAULT_RINGS,
  grainFromPlan,
  hopDistances,
  planRingGrain,
  ringsFor,
  seedFromPath,
} from '../src/viewmodels/mixedGrain.ts';
import type { RingGrains } from '../src/viewmodels/mixedGrain.ts';
import type { D3Link, D3Node, GraphData, GraphLevel } from '../src/types/graph.ts';

function node(id: string, file_path: string): D3Node {
  return {
    id, original_id: id, name: id, qualified_name: id,
    kind: 'function', kind_raw: 'Function', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'TypeScript',
  } as unknown as D3Node;
}

const link = (source: string, target: string, kind_raw = 'Calls'): D3Link =>
  ({ source, target, kind: kind_raw.toLowerCase(), kind_raw, order: null } as unknown as D3Link);

/**
 * a1 is the focus.
 *
 *   ui/src/a.ts   a1 ─→ a2          (same file)
 *                 a1 ─→ ghost
 *   ui/src/b.ts   a1 ─→ b1,  b2 unreachable
 *   lib/c.ts      b1 ─→ c1
 *   lib/e.ts      e1 unreachable
 *   far/d.ts      c1 ─→ d1
 */
const NODES: D3Node[] = [
  node('a1', 'ui/src/a.ts'),
  node('a2', 'ui/src/a.ts'),
  node('b1', 'ui/src/b.ts'),
  node('b2', 'ui/src/b.ts'),
  node('c1', 'lib/c.ts'),
  node('e1', 'lib/e.ts'),
  node('d1', 'far/d.ts'),
  node('ghost', ''),
];

const LINKS: D3Link[] = [
  link('a1', 'a2'),
  link('a1', 'b1'),
  link('a1', 'ghost'),
  link('b1', 'c1'),
  link('c1', 'd1'),
];

const GRAPH: GraphData = { nodes: NODES, links: LINKS } as GraphData;
const seed = new Set(['a1']);

const plan = () => planRingGrain(NODES, LINKS, seed, DEFAULT_RINGS);
const grains = () => Object.fromEntries(plan().grainById);

// ---------------------------------------------------------------- the walk

test('hops are counted from the seed, undirected', () => {
  const d = hopDistances(LINKS, seed, 3);
  assert.equal(d.get('a1'), 0);
  assert.equal(d.get('b1'), 1);
  assert.equal(d.get('c1'), 2);
  assert.equal(d.get('d1'), 3);
});

test('a caller is exactly as near as a callee', () => {
  // c1 reaches b1 against the arrow and d1 along it. A ring is "how near is
  // this to what I am reading"; direction is the direction filters' question.
  const d = hopDistances(LINKS, new Set(['c1']), 2);
  assert.equal(d.get('b1'), 1);
  assert.equal(d.get('d1'), 1);
  assert.equal(d.get('a1'), 2);
});

test('the walk stops at the cap rather than crossing the component', () => {
  const d = hopDistances(LINKS, seed, 1);
  assert.deepEqual([...d.keys()].sort(), ['a1', 'a2', 'b1', 'ghost']);
});

test('no seed is no walk', () => {
  assert.equal(hopDistances(LINKS, new Set(), 3).size, 0);
});

// --------------------------------------------------------------- the grain

test('the focus and one hop out stay entities, the rest coarsens', () => {
  const g = grains();
  assert.equal(g.a1, 'entity');
  assert.equal(g.b1, 'entity');
  assert.equal(g.c1, 'file');
  assert.equal(g.d1, 'folder');
});

test('a file with one entity in reach opens whole', () => {
  // b2 is unreachable and would ask for the outermost ring on its own. Its
  // file is open, so it draws as an entity beside b1 — anything else puts b2
  // inside a Folder rollup that also contains the b1 already on screen.
  assert.equal(hopDistances(LINKS, seed, 3).has('b2'), false);
  assert.equal(grains().b2, 'entity');
});

test('a file out of reach inside an opened folder is a File, never a Folder', () => {
  // lib is opened because c1 is in reach at file grain. e1 is in no ring at
  // all, but folding it to `lib` would draw a Folder node overlapping the
  // lib/c.ts File node beside it.
  assert.equal(grains().e1, 'file');
  assert.equal(grains().c1, 'file');
});

test('a module nothing reaches stays one circle', () => {
  assert.equal(grains().d1, 'folder');
});

test('a ghost takes its ring and belongs to no scope', () => {
  // One hop from the focus, so it is drawn; below Entity `collapseGraph`
  // drops it, which is the exemption `f.grouping` holds at every grain.
  assert.equal(grains().ghost, 'entity');
  const far = planRingGrain(NODES, LINKS, new Set(['d1']), DEFAULT_RINGS);
  assert.equal(far.grainById.get('ghost'), 'folder');
});

test('the cost of a ring plan is known before it is drawn', () => {
  // a1 a2 b1 b2 ghost as entities, lib/c.ts and lib/e.ts as files, far as one
  // module — eight nodes become eight circles here only by coincidence of the
  // fixture; what matters is that it is counted, not drawn, to get it.
  const p = plan();
  assert.equal(p.drawnCount, 8);
  const budgeted = planRingGrain(NODES, LINKS, seed, ['entity', 'folder']);
  assert.ok(budgeted.drawnCount < p.drawnCount);
});

// ------------------------------------------------- the uniform case, intact

test('grainFromLevel reproduces a uniform level exactly', () => {
  const none: ReadonlySet<string> = new Set();
  const at = (level: GraphLevel, exp: ReadonlySet<string> = none) =>
    Object.fromEntries(NODES.map((n) => [n.id, grainFromLevel(level, exp)(n)]));

  assert.equal(at('entity').a1, 'entity');
  assert.equal(at('file').a1, 'file');
  assert.equal(at('folder').a1, 'folder');
  // Expansion opens exactly one level, and never two.
  assert.equal(at('file', new Set(['ui/src/a.ts'])).a1, 'entity');
  assert.equal(at('folder', new Set(['ui/src'])).a1, 'file');
});

test('an expanded root never promotes ghosts onto the canvas', () => {
  // The root module's path is `''` and a ghost's file_path is `''`. Testing
  // membership before the file_path guard would open every external symbol in
  // the repo the moment the reader expanded the root.
  const rootExpanded = new Set(['']);
  assert.equal(grainFromLevel('folder', rootExpanded)(node('ghost', '')), 'folder');
  assert.equal(grainFromLevel('file', rootExpanded)(node('ghost', '')), 'file');
  const collapsed = collapseGraph(GRAPH, 'file', rootExpanded);
  assert.equal(collapsed.nodes.some((n) => n.original_id === ''), false);
});

// --------------------------------------------------- the plan, on the canvas

test('a ring plan draws one picture at three grains', () => {
  const p = plan();
  const mixed = collapseGraph(GRAPH, 'entity', new Set(), grainFromPlan(p, DEFAULT_RINGS));
  const kinds = Object.fromEntries(mixed.nodes.map((n) => [n.original_id ?? n.id, n.kind_raw]));

  assert.equal(kinds['a1'], 'Function');
  assert.equal(kinds['lib/c.ts'], 'File');
  assert.equal(kinds['far'], 'Folder');
});

test('an edge between two drawn entities keeps its own kind', () => {
  // UI-058's rule, which is what makes a mixed canvas readable rather than a
  // wall of `DependsOn`.
  const p = plan();
  const mixed = collapseGraph(GRAPH, 'entity', new Set(), grainFromPlan(p, DEFAULT_RINGS));
  const a1b1 = mixed.links.find((l) => l.source === 'a1' && l.target === 'b1');
  assert.equal(a1b1?.kind_raw, 'Calls');
});

test('an edge into a rollup merges, and carries what it merged', () => {
  const p = plan();
  const mixed = collapseGraph(GRAPH, 'entity', new Set(), grainFromPlan(p, DEFAULT_RINGS));
  const b1c = mixed.links.find((l) => l.source === 'b1' && l.target === 'lib_c_ts');
  assert.equal(b1c?.kind_raw, 'DependsOn');
  assert.deepEqual(b1c?.breakdown, { Calls: 1 });
});

test('the drawn count is what the canvas actually gets', () => {
  const p = plan();
  const mixed = collapseGraph(GRAPH, 'entity', new Set(), grainFromPlan(p, DEFAULT_RINGS));
  assert.equal(mixed.nodes.length, p.drawnCount);
});

test('a node the plan never saw falls outward, not to Entity', () => {
  const p = plan();
  const stranger = node('late', 'brand/new.ts');
  assert.equal(grainFromPlan(p, DEFAULT_RINGS)(stranger), 'folder');
});

// ------------------------------------------------ the focus, and the controls

test('a focused file seeds every entity in it', () => {
  assert.deepEqual([...seedFromPath(NODES, 'ui/src/a.ts')].sort(), ['a1', 'a2']);
});

test('a focused folder seeds everything beneath it', () => {
  assert.deepEqual([...seedFromPath(NODES, 'ui/src')].sort(), ['a1', 'a2', 'b1', 'b2']);
});

test('a focus matches on a segment boundary, never a prefix', () => {
  // `ui/src` must not take `ui/srcgen` with it — the two are unrelated
  // directories that happen to share five characters.
  const withSibling = [...NODES, node('gen1', 'ui/srcgen/g.ts')];
  assert.equal(seedFromPath(withSibling, 'ui/src').has('gen1'), false);
});

test('a ghost is never a seed', () => {
  // Its file_path is `''`, which as a prefix matches every path there is.
  assert.equal(seedFromPath(NODES, '').has('ghost'), false);
});

test('reach says how far entities extend, the level says what the rest is', () => {
  assert.deepEqual(ringsFor(0, 'folder'), ['entity', 'folder']);
  assert.deepEqual(ringsFor(2, 'file'), ['entity', 'entity', 'entity', 'file']);
});

test('the two controls compose: same focus, wider reach, more detail', () => {
  const seedA = seedFromPath(NODES, 'ui/src/a.ts');
  const tight = planRingGrain(NODES, LINKS, seedA, ringsFor(0, 'folder'));
  const wide = planRingGrain(NODES, LINKS, seedA, ringsFor(2, 'folder'));
  assert.equal(tight.grainById.get('c1'), 'folder');
  assert.equal(wide.grainById.get('c1'), 'entity');
});

test('the focus itself is always drawn at the finest grain', () => {
  // The invariant the whole control rests on: `ringsFor` puts Entity in ring
  // 0, so focusing a scope can never fold away the thing being focused. A
  // reader whose focus vanished from the canvas would have no way back to it.
  for (const outer of ['entity', 'file', 'folder'] as GraphLevel[]) {
    const p = planRingGrain(NODES, LINKS, seedFromPath(NODES, 'ui/src/a.ts'), ringsFor(0, outer));
    assert.equal(p.grainById.get('a1'), 'entity', `outer=${outer}`);
    assert.equal(p.grainById.get('a2'), 'entity', `outer=${outer}`);
  }
});

test('one ring is a uniform picture again', () => {
  const rings: RingGrains = ['file'];
  const p = planRingGrain(NODES, LINKS, seed, rings);
  assert.deepEqual(
    [...new Set(NODES.filter((n) => n.file_path).map((n) => p.grainById.get(n.id)))],
    ['file'],
  );
});
