/**
 * Unit tests for what the Quality panel is measuring.
 *
 * The panel names a population and then reports numbers about it, so the bug
 * these guard against is silent by construction: a rollup table that answers
 * for a different set of files than the summary above it still renders, still
 * sorts, and still looks right. The three rules below are the ones that decide
 * membership.
 *
 *   npm run test:quality-population
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { D3Node, GraphData, ScopeMetrics } from '../src/types/graph.ts';
import {
  ancestorDirs,
  narrowRollups,
  scopeHolds,
  selectionPopulation,
} from '../src/viewmodels/qualityPopulation.ts';

function node(over: Partial<D3Node> & { id: string; file_path: string }): D3Node {
  return {
    original_id: over.id,
    name: over.id,
    qualified_name: over.id,
    kind: 'function',
    kind_raw: 'Function',
    line: 1,
    end_line: 2,
    visibility: 'Public',
    parent_id: null,
    parameters: [],
    return_type: null,
    extends: [],
    implements: [],
    tags: [],
    source_code: null,
    fields: [],
    impl_blocks: [],
    language: 'Rust',
    ...over,
  } as D3Node;
}

function scope(path: string): ScopeMetrics {
  return {
    path,
    entity_count: 1,
    callable_count: 1,
    container_count: 0,
    loc: 10,
    internal_edges: 0,
    external_edges: 0,
    cohesion: null,
    fan_in: 0,
    fan_out: 0,
    in_cycle: false,
  } as unknown as ScopeMetrics;
}

// --- scopeHolds -------------------------------------------------------------

test('a file rollup holds exactly its own file', () => {
  assert.equal(scopeHolds('src/a.rs', false, 'src/a.rs'), true);
  assert.equal(scopeHolds('src/a.rs', false, 'src/b.rs'), false);
});

test('a module rollup holds its whole subtree, and the root holds everything', () => {
  assert.equal(scopeHolds('src', true, 'src/a.rs'), true);
  assert.equal(scopeHolds('src', true, 'src/deep/nested/a.rs'), true);
  assert.equal(scopeHolds('', true, 'anything/at/all.rs'), true);
});

test('a module does not hold a sibling whose name it prefixes', () => {
  // `src/parser` must not swallow `src/parsed` — the separator is the whole
  // difference between a subtree and a string that starts the same way.
  assert.equal(scopeHolds('src/parser', true, 'src/parsed/x.rs'), false);
  assert.equal(scopeHolds('src/parser', true, 'src/parser/x.rs'), true);
});

// --- ancestorDirs -----------------------------------------------------------

test('ancestorDirs walks every directory above a file, root included', () => {
  assert.deepEqual(ancestorDirs('ui/src/stores/graph.ts'), ['', 'ui', 'ui/src', 'ui/src/stores']);
});

test('a file at the root has only the root above it', () => {
  assert.deepEqual(ancestorDirs('README.md'), ['']);
});

// --- narrowRollups ----------------------------------------------------------

test('rollups the population no longer holds are dropped, ancestors kept', () => {
  const g: GraphData = {
    nodes: [node({ id: 'a', file_path: 'ui/src/a.ts' }), node({ id: 'b', file_path: 'ui/lib/b.ts' })],
    links: [],
    files: [scope('ui/src/a.ts'), scope('ui/lib/b.ts')],
    folders: [scope(''), scope('ui'), scope('ui/src'), scope('ui/lib')],
  };

  const narrowed = narrowRollups(g, [g.nodes[0]]);

  assert.deepEqual(narrowed.files?.map((f) => f.path), ['ui/src/a.ts']);
  // `ui` survives because it is still an ancestor of what is left; `ui/lib`
  // does not, because nothing in the population lives under it.
  assert.deepEqual(narrowed.folders?.map((m) => m.path), ['', 'ui', 'ui/src']);
  assert.equal(narrowed.nodes.length, 1);
});

test('rollup numbers are left as the engine computed them', () => {
  const g: GraphData = {
    nodes: [node({ id: 'a', file_path: 'a.ts' }), node({ id: 'b', file_path: 'a.ts' })],
    links: [],
    files: [{ ...scope('a.ts'), entity_count: 2 } as ScopeMetrics],
    folders: [],
  };

  // Half the file's entities, but cohesion and entity_count still describe the
  // whole file — recomputing them from a slice would answer a different
  // question, so the row is either listed as-is or not listed at all.
  const narrowed = narrowRollups(g, [g.nodes[0]]);
  assert.equal(narrowed.files?.[0].entity_count, 2);
});

test('ghosts carry no path and pull no rollup in with them', () => {
  const g: GraphData = {
    nodes: [node({ id: 'ghost', file_path: '', tags: ['ghost'] })],
    links: [],
    files: [scope('a.ts')],
    folders: [scope('')],
  };

  const narrowed = narrowRollups(g);
  assert.deepEqual(narrowed.files, []);
  assert.deepEqual(narrowed.folders, []);
});

// --- selectionPopulation ----------------------------------------------------

const TREE: D3Node[] = [
  node({ id: 'Thing', file_path: 'src/thing.rs', kind_raw: 'Struct' }),
  node({ id: 'Thing::run', file_path: 'src/thing.rs', parent_id: 'Thing' }),
  node({ id: 'Thing::step', file_path: 'src/thing.rs', parent_id: 'Thing::run' }),
  node({ id: 'loose', file_path: 'src/thing.rs' }),
  node({ id: 'elsewhere', file_path: 'src/other.rs' }),
];

test('nothing selected is an empty population, not the whole graph', () => {
  assert.deepEqual(selectionPopulation(TREE, null), []);
});

test('selecting an entity takes it and everything it contains, transitively', () => {
  const picked = selectionPopulation(TREE, TREE[0]).map((n) => n.id).sort();
  assert.deepEqual(picked, ['Thing', 'Thing::run', 'Thing::step']);
});

test('selecting a leaf reports the leaf alone', () => {
  assert.deepEqual(selectionPopulation(TREE, TREE[3]).map((n) => n.id), ['loose']);
});

test('selecting a File node takes the whole file, containment aside', () => {
  const file = node({ id: 'src_thing_rs', file_path: 'src/thing.rs', kind_raw: 'File' });
  file.original_id = 'src/thing.rs';
  const picked = selectionPopulation(TREE, file).map((n) => n.id).sort();
  assert.deepEqual(picked, ['Thing', 'Thing::run', 'Thing::step', 'loose']);
});

test('selecting a Folder node takes its whole subtree', () => {
  const mod = node({ id: 'src', file_path: 'src', kind_raw: 'Folder' });
  mod.original_id = 'src';
  assert.equal(selectionPopulation(TREE, mod).length, 5);
});

test('a name-keyed parent only adopts children in its own file', () => {
  // Rust impl blocks record the bare type name as `parent_id`. A bare name is
  // not unique across a repo, so a same-named type in another file must not be
  // pulled into the population and inflate what the metrics claim to describe.
  const impl = node({ id: 'ids/Handler', file_path: 'src/a.rs', kind_raw: 'Struct' });
  impl.original_id = 'ids/Handler';
  impl.name = 'Handler';
  const all = [
    impl,
    node({ id: 'mine', file_path: 'src/a.rs', parent_id: 'Handler' }),
    node({ id: 'theirs', file_path: 'src/b.rs', parent_id: 'Handler' }),
  ];

  const picked = selectionPopulation(all, impl).map((n) => n.id).sort();
  assert.deepEqual(picked, ['ids/Handler', 'mine']);
});

test('a containment cycle terminates instead of hanging the panel', () => {
  const a = node({ id: 'a', file_path: 'x.rs', parent_id: 'b' });
  const b = node({ id: 'b', file_path: 'x.rs', parent_id: 'a' });
  assert.equal(selectionPopulation([a, b], a).length, 2);
});
