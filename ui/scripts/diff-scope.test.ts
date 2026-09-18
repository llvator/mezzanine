/**
 * Unit tests for the scope a comparison implies (UI-135).
 *
 * These two rules used to live inside a `case` of App.svelte's message
 * switch, where the only way to check them was to run a diff against a live
 * engine and read the console. Both of them fail *quietly* when they are
 * wrong — the ripple rule by widening the scope back to most of the
 * repository, the leaf rule by widening it to all of it — so a wrong answer
 * looks like the very condition the feature exists to relieve.
 *
 *   npm run test:diffscope
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { changedFileScope } from '../src/viewmodels/diffScope.ts';
import type { EntityDiff } from '../src/stores/diff.ts';
import type { IndexData } from '../src/stores/scope.ts';

function row(over: Partial<EntityDiff> & { file_path: string }): EntityDiff {
  return {
    entity_id: `id:${over.file_path}:${over.name ?? 'x'}`,
    name: 'x',
    kind: 'Function',
    status: 'modified',
    source_changed: true,
    metric_deltas: [],
    ...over,
  } as EntityDiff;
}

/** An index holding each named path as a file, and each folder as a folder. */
function index(files: string[], folders: string[] = []): IndexData {
  const nodes: IndexData['nodes'] = {};
  for (const p of files) {
    nodes[p] = { path: p, type: 'file', entity_count: 1, relationship_count: 0 };
  }
  for (const p of folders) {
    nodes[p] = { path: p, type: 'folder', entity_count: 9, relationship_count: 3 };
  }
  return { nodes } as IndexData;
}

test('the files a comparison edited become the scope', () => {
  const scope = changedFileScope(
    [row({ file_path: 'src/a.rs' }), row({ file_path: 'src/b.rs' })],
    index(['src/a.rs', 'src/b.rs']),
  );
  assert.deepEqual(scope.paths.sort(), ['src/a.rs', 'src/b.rs']);
  assert.equal(scope.droppedImpactOnly, 0);
});

test('one file with several changed entities is one scope entry', () => {
  const scope = changedFileScope(
    [
      row({ file_path: 'src/a.rs', name: 'one' }),
      row({ file_path: 'src/a.rs', name: 'two' }),
    ],
    index(['src/a.rs']),
  );
  assert.deepEqual(scope.paths, ['src/a.rs']);
});

test('an added and a removed entity both scope', () => {
  const scope = changedFileScope(
    [
      row({ file_path: 'src/new.rs', status: 'added' }),
      row({ file_path: 'src/gone.rs', status: 'removed' }),
    ],
    index(['src/new.rs', 'src/gone.rs']),
  );
  assert.deepEqual(scope.paths.sort(), ['src/gone.rs', 'src/new.rs']);
});

/** The rule that matters most. A single edit marks dozens of untouched files
 *  as `modified` through their fan-in/fan-out, and scoping to those puts the
 *  reader back where they started. */
test('a file that only felt the change is not a file that changed', () => {
  const scope = changedFileScope(
    [
      row({ file_path: 'src/edited.rs' }),
      row({ file_path: 'src/rippled.rs', source_changed: false }),
      row({ file_path: 'src/untouched.rs', status: 'unchanged' }),
    ],
    index(['src/edited.rs', 'src/rippled.rs', 'src/untouched.rs']),
  );
  assert.deepEqual(scope.paths, ['src/edited.rs']);
  assert.equal(scope.droppedImpactOnly, 2);
});

/** `minimizeSelection` reads a folder as covering everything beneath it, so
 *  one folder path here is the whole repository back again. */
test('a folder path is dropped rather than trusted', () => {
  const scope = changedFileScope(
    [row({ file_path: 'src' }), row({ file_path: 'src/a.rs' })],
    index(['src/a.rs'], ['src']),
  );
  assert.deepEqual(scope.paths, ['src/a.rs']);
  assert.deepEqual(scope.droppedNonLeaf, ['src']);
});

test('a ghost carries no file and is dropped', () => {
  const scope = changedFileScope(
    [row({ file_path: '' }), row({ file_path: 'src/a.rs' })],
    index(['src/a.rs']),
  );
  assert.deepEqual(scope.paths, ['src/a.rs']);
});

test('a path the index has never heard of is dropped', () => {
  const scope = changedFileScope(
    [row({ file_path: 'src/absent.rs' })],
    index(['src/a.rs']),
  );
  assert.deepEqual(scope.paths, []);
  assert.deepEqual(scope.droppedNonLeaf, ['src/absent.rs']);
});

/** Without an index there is nothing to check against, and an unscoped
 *  comparison is a better failure than an empty one. */
test('with no index the paths pass through', () => {
  const scope = changedFileScope([row({ file_path: 'src/a.rs' })], null);
  assert.deepEqual(scope.paths, ['src/a.rs']);
  assert.deepEqual(scope.droppedNonLeaf, []);
});

test('an empty or absent diff scopes to nothing', () => {
  for (const entities of [[], null, undefined]) {
    assert.deepEqual(changedFileScope(entities, index([])).paths, []);
  }
});

/** The caller reads `paths.length === 0` to decide *not* to re-scope. An
 *  impact-only comparison that came back with paths would blank the canvas. */
test('an impact-only comparison yields no scope at all', () => {
  const scope = changedFileScope(
    [
      row({ file_path: 'src/a.rs', source_changed: false }),
      row({ file_path: 'src/b.rs', source_changed: false }),
    ],
    index(['src/a.rs', 'src/b.rs']),
  );
  assert.equal(scope.paths.length, 0);
  assert.equal(scope.droppedImpactOnly, 2);
});
