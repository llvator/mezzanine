/**
 * Unit tests for the diff scope rollup (UI-064).
 *
 * The bug these exist to keep dead: with a diff loaded, the canvas drew
 * *nothing* at File or Module aggregation. A collapsed node carries its
 * scope path in `original_id`, the status map is keyed by entity id, the
 * lookup missed on every node, and a miss was read as "unchanged" — which,
 * with `coreOnly` on and the dim slider at 0, is `display: none`.
 *
 * So the properties worth asserting are not "the numbers add up" but the two
 * things the filter asks this map:
 *
 *   1. does this scope contain a change (and is any of it *core*)
 *   2. did the diff look at this scope at all
 *
 * The second is why `has()` is tested as carefully as `get()`: it is the
 * only thing separating *unknown* from *unchanged*, and conflating those is
 * how a file created since the diff ran disappears at the moment it is most
 * interesting.
 *
 *   npm run test:diff
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { rollUpByScope, scopeChain, normalizeScopePath } from '../src/viewmodels/diffRollup.ts';
import { collapseGraph } from '../src/viewmodels/collapseGraph.ts';
import type { EntityDiff, ChangeStatus } from '../src/stores/diff.ts';
import type { D3Node, GraphData } from '../src/types/graph.ts';

/** A diff entry with only the fields the rollup reads. */
function entry(
  file_path: string,
  status: ChangeStatus,
  source_changed = true,
  name = 'e',
): EntityDiff {
  return {
    entity_id: `${file_path}:1:${name}`,
    name,
    kind: 'Function',
    file_path,
    status,
    source_changed,
    metric_deltas: [],
  };
}

// ── scopeChain ────────────────────────────────────────────────────────────

test('a file belongs to itself and to the module that draws it', () => {
  // Not every ancestor: `collapseGraph` puts a file in exactly one module,
  // so `ui/src` must not inherit what `ui/src/stores` already shows.
  assert.deepEqual(scopeChain('ui/src/stores/diff.ts'), [
    'ui/src/stores/diff.ts', 'ui/src/stores',
  ]);
});

test('a top-level file still reaches the root scope', () => {
  // `collapseGraph` gives a module-level node for a root file the scope id
  // '', so without this the root node has no rollup at all.
  assert.deepEqual(scopeChain('README.md'), ['README.md', '']);
});

test('the root scope is its own chain', () => {
  assert.deepEqual(scopeChain(''), ['']);
});

test('a base-worktree path is brought back to repo-relative', () => {
  assert.equal(
    normalizeScopePath('/var/folders/x/nao-diff-base-abc123/ui/src/App.svelte'),
    'ui/src/App.svelte',
  );
  assert.equal(normalizeScopePath('ui/src/App.svelte'), 'ui/src/App.svelte');
});

// ── what the collapsed canvas asks ────────────────────────────────────────

test('a file with one changed entity among many reads as changed', () => {
  const scopes = rollUpByScope([
    entry('ui/src/a.ts', 'unchanged', false, 'x'),
    entry('ui/src/a.ts', 'unchanged', false, 'y'),
    entry('ui/src/a.ts', 'modified', true, 'z'),
  ]);
  assert.equal(scopes.get('ui/src/a.ts')?.status, 'modified');
  assert.equal(scopes.get('ui/src/a.ts')?.sourceChanged, true);
});

test('a file whose entities all held still reads as unchanged', () => {
  const scopes = rollUpByScope([
    entry('ui/src/a.ts', 'unchanged', false, 'x'),
    entry('ui/src/a.ts', 'unchanged', false, 'y'),
  ]);
  assert.deepEqual(scopes.get('ui/src/a.ts'), { status: 'unchanged', sourceChanged: false });
});

test('a wholly new file reads as added, not merely modified', () => {
  const scopes = rollUpByScope([
    entry('ui/src/new.ts', 'added', true, 'a'),
    entry('ui/src/new.ts', 'added', true, 'b'),
  ]);
  assert.equal(scopes.get('ui/src/new.ts')?.status, 'added');
});

test('a wholly deleted file reads as removed', () => {
  const scopes = rollUpByScope([
    entry('ui/src/gone.ts', 'removed', true, 'a'),
    entry('ui/src/gone.ts', 'removed', true, 'b'),
  ]);
  assert.equal(scopes.get('ui/src/gone.ts')?.status, 'removed');
});

test('a file that gained a function is modified, not added', () => {
  // The mixed case: "added" would overstate it — the file was already there.
  const scopes = rollUpByScope([
    entry('ui/src/a.ts', 'unchanged', false, 'old'),
    entry('ui/src/a.ts', 'added', true, 'fresh'),
  ]);
  assert.equal(scopes.get('ui/src/a.ts')?.status, 'modified');
});

test('a directory rolls up the files it directly contains', () => {
  const scopes = rollUpByScope([
    entry('ui/src/stores/a.ts', 'unchanged', false),
    entry('ui/src/views/b.ts', 'modified', true),
  ]);
  assert.equal(scopes.get('ui/src/stores')?.status, 'unchanged');
  assert.equal(scopes.get('ui/src/views')?.status, 'modified');
});

test('a change does not travel up past the module that draws it', () => {
  // `ui/src` and `ui/src/views` are two nodes side by side on a module-level
  // canvas. If the change reached both, `changesOnly` would show a module
  // whose own files never moved.
  const scopes = rollUpByScope([entry('ui/src/views/b.ts', 'modified', true)]);
  assert.equal(scopes.has('ui/src'), false);
  assert.equal(scopes.has('ui'), false);
});

// ── core vs impact ────────────────────────────────────────────────────────

test('a scope whose only changes are impact-only is not core', () => {
  // `coreOnly` is ON by default, so this is the case that decides whether a
  // file of ripple appears in the default diff view.
  const scopes = rollUpByScope([
    entry('ui/src/ripple.ts', 'modified', false, 'a'),
    entry('ui/src/ripple.ts', 'unchanged', false, 'b'),
  ]);
  assert.deepEqual(scopes.get('ui/src/ripple.ts'), { status: 'modified', sourceChanged: false });
});

test('one core change makes the whole scope core', () => {
  const scopes = rollUpByScope([
    entry('ui/src/mixed.ts', 'modified', false, 'ripple'),
    entry('ui/src/mixed.ts', 'modified', true, 'edited'),
  ]);
  assert.equal(scopes.get('ui/src/mixed.ts')?.sourceChanged, true);
});

test('an unchanged entity marked source_changed does not make a scope core', () => {
  // `source_changed` defaults true in the payload, so an unchanged entity
  // carrying it must not count — otherwise every file is core and `coreOnly`
  // filters nothing.
  const scopes = rollUpByScope([entry('ui/src/still.ts', 'unchanged', true)]);
  assert.deepEqual(scopes.get('ui/src/still.ts'), { status: 'unchanged', sourceChanged: false });
});

// ── unknown is not unchanged ──────────────────────────────────────────────

test('the diff covers every file it reported on, changed or not', () => {
  const scopes = rollUpByScope([
    entry('ui/src/a.ts', 'unchanged', false),
    entry('ui/src/b.ts', 'modified', true),
  ]);
  assert.ok(scopes.has('ui/src/a.ts'), 'an unchanged file is still covered');
  assert.ok(scopes.has('ui/src'), 'so is its directory');
});

test('a file the diff never saw is absent, not unchanged', () => {
  const scopes = rollUpByScope([entry('ui/src/a.ts', 'unchanged', false)]);
  assert.equal(scopes.has('ui/src/created-after-the-diff.ts'), false);
  assert.equal(scopes.get('ui/src/created-after-the-diff.ts'), undefined);
});

test('an empty diff covers nothing at all', () => {
  // Not "covers everything as unchanged" — with no diff loaded the filter
  // must not have an opinion about a single node.
  assert.equal(rollUpByScope([]).size, 0);
});

// ── the seam that actually broke ──────────────────────────────────────────
//
// Everything above tests the rollup against paths this file wrote down. The
// bug was not in the rollup — it was that the two sides never met: the map
// was keyed by entity id and the canvas asked with a path. So the test that
// would have caught it runs the real `collapseGraph` and asks the rollup
// with whatever *it* produces, rather than with a string chosen to match.

/** A graph entity, with the fields collapse and the diff filter read. */
function node(id: string, file_path: string): D3Node {
  return {
    id, original_id: id, name: id.split(':').pop() ?? id, qualified_name: id,
    kind: 'function', kind_raw: 'Function', file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'TypeScript',
  } as unknown as D3Node;
}

const FILES = ['ui/src/stores/diff.ts', 'ui/src/stores/graph.ts', 'ui/src/App.svelte', 'README.md'];

const graph: GraphData = {
  nodes: FILES.map((f, i) => node(`${f}:${i}:fn`, f)),
  links: [],
} as unknown as GraphData;

for (const level of ['file', 'module'] as const) {
  test(`every ${level}-level node the canvas draws can be found in the rollup`, () => {
    const collapsed = collapseGraph(graph, level);
    const scopes = rollUpByScope(FILES.map((f) => entry(f, 'unchanged', false)));
    assert.ok(collapsed.nodes.length > 0, 'fixture should collapse to something');
    for (const n of collapsed.nodes) {
      assert.ok(
        scopes.has(n.original_id),
        `collapsed ${level} node '${n.original_id}' is invisible to the diff filter`,
      );
    }
  });
}

test('a collapsed file node carries its scope path, not an entity id', () => {
  // The premise of the fallback lookup. If this ever stops being true, the
  // rollup is being asked the wrong question and the tests above would pass
  // while the canvas went blank again.
  const collapsed = collapseGraph(graph, 'file');
  const ids = collapsed.nodes.map((n) => n.original_id).sort();
  assert.deepEqual(ids, [...FILES].sort());
});

test('a file that changed reaches its own module node and no other', () => {
  const collapsed = collapseGraph(graph, 'module');
  const scopes = rollUpByScope([
    entry('ui/src/stores/diff.ts', 'modified', true),
    entry('ui/src/stores/graph.ts', 'unchanged', false),
    entry('ui/src/App.svelte', 'unchanged', false),
    entry('README.md', 'unchanged', false),
  ]);
  const drawn = collapsed.nodes.map((n) => n.original_id);
  // Every module the canvas draws gets an answer...
  for (const path of drawn) assert.ok(scopes.has(path), `no rollup for drawn module '${path}'`);
  // ...and the answers are the ones a reader would give.
  assert.equal(scopes.get('ui/src/stores')?.status, 'modified');
  assert.equal(scopes.get('ui/src')?.status, 'unchanged', 'the module next to it holds still');
  // The root-level file collapses to the '' module — the case with no slash
  // to walk up from.
  assert.ok(drawn.includes(''), 'a top-level file should produce a root module node');
  assert.equal(scopes.get('')?.status, 'unchanged');
});
