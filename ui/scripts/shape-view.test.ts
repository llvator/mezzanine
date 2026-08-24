/**
 * The shape view — one folder's graph, drawn so the violations are visible.
 *
 * The failure worth testing is a drawing that *lies*: an edge marked clean
 * when the engine called it a skip, a breach drawn as an entry, a file left
 * on the canvas that has nothing to do with the folder. All of those look
 * fine on screen — a picture cannot be wrong in a way a reader notices,
 * which is exactly why it has to be asserted here rather than clicked
 * through.
 *
 *   npm run test:shape-view
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEFAULT_SHAPE_LAYOUT,
  childHolding,
  isInside,
  isViolation,
  shapeEdgeVerdicts,
  shapePlacement,
  shapeResolvers,
  verdictFor,
} from '../src/viewmodels/shapeView.ts';
import { collapseGraph } from '../src/viewmodels/collapseGraph.ts';
import type {
  D3Link,
  D3Node,
  FolderPicture,
  GraphData,
  OutsideEdge,
  PictureChild,
  PictureEdge,
} from '../src/types/graph.ts';

function child(path: string, level: number, extra: Partial<PictureChild> = {}): PictureChild {
  return { path, kind: 'file', level, inbound: 0, is_door: false, ...extra };
}

function edge(from: string, to: string, verdict: PictureEdge['verdict']): PictureEdge {
  return { from, to, verdict };
}

function outside(
  outsidePath: string,
  inside: string,
  child_: string,
  verdict: OutsideEdge['verdict'],
): OutsideEdge {
  return { outside: outsidePath, inside, child: child_, verdict };
}

function picture(p: Partial<FolderPicture> = {}): FolderPicture {
  return { folder: 'src/db', children: [], edges: [], outside: [], doors: [], ...p };
}

/** Enough of a node for the resolvers; the rest is inert here. */
function node(id: string, file_path: string): D3Node {
  return {
    id, original_id: id, name: id, qualified_name: id,
    kind: 'function', kind_raw: 'Function', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'Rust',
  } as unknown as D3Node;
}

function link(source: string, target: string): D3Link {
  return { source, target, kind: 'calls', kind_raw: 'Calls', order: null } as D3Link;
}

// --- Which nodes are on screen ---

test('a sibling folder sharing a name prefix is outside', () => {
  // The same trap the Rust side guards: `src/parsed` is not inside
  // `src/parse`, and a bare prefix test would swallow it whole.
  assert.equal(isInside('src/parse', 'src/parsed/b.rs'), false);
  assert.equal(isInside('src/parse', 'src/parse/a.rs'), true);
  assert.equal(isInside('', 'anything.rs'), true, 'the root holds everything');
});

test('a nested file collapses to the child of the folder holding it', () => {
  // Not to its own parent directory — which is what makes `ScopeOf` need to
  // be injectable at all, since `moduleOf` can only ever answer the latter.
  assert.equal(childHolding('src/db', 'src/db/inner/deep/rows.rs'), 'src/db/inner');
  assert.equal(childHolding('src/db', 'src/db/pool.rs'), 'src/db/pool.rs');
  assert.equal(childHolding('src/db', 'src/other/x.rs'), null);
  assert.equal(childHolding('', 'src/db/pool.rs'), 'src');
});

test('the canvas holds the folder, its neighbours, and nothing else', () => {
  const p = picture({
    children: [child('src/db/pool.rs', 0), child('src/db/inner', 1, { kind: 'folder' })],
    outside: [outside('src/api.rs', 'src/db/pool.rs', 'src/db/pool.rs', 'entry')],
  });
  const { grainOf, scopeOf } = shapeResolvers(p);
  const raw: GraphData = {
    nodes: [
      node('pool', 'src/db/pool.rs'),
      node('rows', 'src/db/inner/deep/rows.rs'),
      node('api', 'src/api.rs'),
      node('unrelated', 'src/util/far.rs'),
      node('ghost', ''),
    ],
    links: [link('api', 'pool'), link('pool', 'rows'), link('unrelated', 'api')],
  };
  const drawn = collapseGraph(raw, 'file', new Set(), grainOf, scopeOf);
  const paths = drawn.nodes.map((n) => n.original_id).sort();

  assert.deepEqual(paths, ['src/api.rs', 'src/db/inner', 'src/db/pool.rs']);
  // The nested file became its subfolder's circle, drawn as a Module — so a
  // reader can open it and the rollup carries the subfolder's own metrics.
  const inner = drawn.nodes.find((n) => n.original_id === 'src/db/inner');
  assert.equal(inner?.kind_raw, 'Module');
  // Two hops out is most of the repo, so it is not drawn: `unrelated` only
  // touches the folder through `api`.
  assert.ok(!paths.includes('src/util/far.rs'));
});

test('a ghost is in no folder and never reaches the canvas', () => {
  // A ghost's file_path is '', which as a folder is the repo root — so an
  // unguarded `isInside` would put every external symbol on screen.
  const { scopeOf } = shapeResolvers(picture({ folder: '' }));
  assert.equal(scopeOf(node('ghost', ''), 'file'), null);
});

// --- Where each node sits ---

test('levels become rows, top to bottom', () => {
  const p = picture({
    children: [child('src/db/a.rs', 0), child('src/db/b.rs', 1), child('src/db/c.rs', 1)],
  });
  const { positions, depth } = shapePlacement(p);
  assert.equal(depth, 2);
  assert.equal(positions.get('src/db/a.rs')!.y, 0);
  assert.equal(positions.get('src/db/b.rs')!.y, DEFAULT_SHAPE_LAYOUT.rowGap);
  assert.equal(
    positions.get('src/db/c.rs')!.y,
    positions.get('src/db/b.rs')!.y,
    'one level is one row',
  );
  assert.notEqual(
    positions.get('src/db/b.rs')!.x,
    positions.get('src/db/c.rs')!.x,
    'siblings do not stack on one point',
  );
});

test('a row is centred, so the drawing hangs off one axis', () => {
  const p = picture({
    children: [child('src/db/a.rs', 0), child('src/db/b.rs', 1), child('src/db/c.rs', 1)],
  });
  const { positions } = shapePlacement(p);
  assert.equal(positions.get('src/db/a.rs')!.x, 0, 'a lone node sits on the axis');
  const b = positions.get('src/db/b.rs')!.x;
  const c = positions.get('src/db/c.rs')!.x;
  assert.equal(b + c, 0, 'a pair straddles it');
});

test('who reaches in goes left, what is reached goes right', () => {
  const p = picture({
    children: [child('src/db/pool.rs', 0)],
    outside: [
      outside('src/api.rs', 'src/db/pool.rs', 'src/db/pool.rs', 'entry'),
      outside('src/models.rs', 'src/db/pool.rs', 'src/db/pool.rs', 'exit'),
    ],
  });
  const { positions } = shapePlacement(p);
  assert.ok(positions.get('src/api.rs')!.x < 0, 'incoming on the left');
  assert.ok(positions.get('src/models.rs')!.x > 0, 'outgoing on the right');
});

test('an outsider that both needs the folder and is needed by it goes left', () => {
  // The question this view exists for is who reaches in, so that side wins
  // the tie — and either way it must land on exactly one side, not two.
  const p = picture({
    children: [child('src/db/pool.rs', 0)],
    outside: [
      outside('src/both.rs', 'src/db/pool.rs', 'src/db/pool.rs', 'breach'),
      outside('src/both.rs', 'src/db/pool.rs', 'src/db/pool.rs', 'exit'),
    ],
  });
  const { positions } = shapePlacement(p);
  assert.ok(positions.get('src/both.rs')!.x < 0);
});

test('the same picture always draws the same way', () => {
  // A layout that reshuffles on an unrelated republish is one the reader has
  // to re-read from scratch.
  const p = picture({
    children: [child('src/db/b.rs', 0), child('src/db/a.rs', 0), child('src/db/c.rs', 1)],
    outside: [outside('src/z.rs', 'src/db/a.rs', 'src/db/a.rs', 'entry')],
  });
  const first = shapePlacement(p);
  const shuffled = picture({
    children: [p.children[2], p.children[0], p.children[1]],
    outside: p.outside,
  });
  const second = shapePlacement(shuffled);
  for (const [path, pos] of first.positions) {
    assert.deepEqual(second.positions.get(path), pos, `${path} moved`);
  }
});

// --- How each edge reads ---

test('every edge carries the reading the engine gave it', () => {
  const p = picture({
    children: [child('src/db/a.rs', 0), child('src/db/b.rs', 1), child('src/db/c.rs', 2)],
    edges: [
      edge('src/db/a.rs', 'src/db/b.rs', 'step'),
      edge('src/db/a.rs', 'src/db/c.rs', 'skip'),
      edge('src/db/b.rs', 'src/db/c.rs', 'step'),
    ],
  });
  const v = shapeEdgeVerdicts(p);
  assert.equal(verdictFor(v, 'src/db/a.rs', 'src/db/b.rs'), 'step');
  assert.equal(verdictFor(v, 'src/db/a.rs', 'src/db/c.rs'), 'skip');
  assert.equal(verdictFor(v, 'src/db/c.rs', 'src/db/a.rs'), null, 'direction matters');
});

test('a boundary edge attaches to the circle, not to the file inside it', () => {
  // The breach names `rows.rs` so the fix has an address, but the line on
  // screen runs to the subfolder circle — the only thing drawn.
  const p = picture({
    children: [child('src/db/inner', 0, { kind: 'folder' })],
    outside: [outside('src/api.rs', 'src/db/inner/rows.rs', 'src/db/inner', 'breach')],
  });
  const v = shapeEdgeVerdicts(p);
  assert.equal(verdictFor(v, 'src/api.rs', 'src/db/inner'), 'breach');
  assert.equal(verdictFor(v, 'src/api.rs', 'src/db/inner/rows.rs'), null);
});

test('a breach does not hide behind an entry sharing its endpoints', () => {
  // Two files inside one subfolder, reached by one outsider: one through the
  // door and one past it. Both collapse onto a single drawn line, and if the
  // kinder reading won, the defect would be invisible — which is the one
  // outcome this whole view exists to prevent.
  const p = picture({
    children: [child('src/db/inner', 0, { kind: 'folder' })],
    outside: [
      outside('src/api.rs', 'src/db/inner/pool.rs', 'src/db/inner', 'entry'),
      outside('src/api.rs', 'src/db/inner/rows.rs', 'src/db/inner', 'breach'),
    ],
  });
  assert.equal(verdictFor(shapeEdgeVerdicts(p), 'src/api.rs', 'src/db/inner'), 'breach');
});

test('a loop outranks a skip on one drawn line', () => {
  const p = picture({
    children: [child('src/db/a.rs', 0), child('src/db/b.rs', 0)],
    edges: [
      edge('src/db/a.rs', 'src/db/b.rs', 'skip'),
      edge('src/db/a.rs', 'src/db/b.rs', 'back'),
    ],
  });
  assert.equal(verdictFor(shapeEdgeVerdicts(p), 'src/db/a.rs', 'src/db/b.rs'), 'back');
});

test('leaving the folder is not a defect', () => {
  // Otherwise almost every line on a real folder draws as a problem, and the
  // three that matter stop standing out.
  assert.equal(isViolation('exit'), false);
  assert.equal(isViolation('entry'), false);
  assert.equal(isViolation('step'), false);
  assert.equal(isViolation('back'), true);
  assert.equal(isViolation('skip'), true);
  assert.equal(isViolation('breach'), true);
});
