/**
 * Unit tests for the repo mean a single composite score is read against.
 *
 * The bug these guard against is silent: a mean that mixes a file's score with
 * a function's, or that quietly drops the backend's own number in favour of a
 * recomputed one, still renders as a plausible two-decimal figure. So does a
 * verdict that says "worse than average" about two numbers the panel prints
 * identically.
 *
 *   npm run test:quality-baseline
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { D3Node, EntityMetrics, GraphData, ScopeMetrics } from '../src/types/graph.ts';
import {
  baselinesFor,
  baselineNoun,
  compareToBaseline,
  grainOfKind,
  nodeScore,
} from '../src/viewmodels/qualityBaseline.ts';

/** Stand-in scorers: the real ones read a threshold cache from `stores/quality`,
 *  and what is under test here is which population each is applied to, not what
 *  either returns. Both are deliberately distinguishable in the output. */
const scoreScope = (s: ScopeMetrics, isFolder: boolean) => (isFolder ? 100 : 10);
const scoreEntity = (_m: EntityMetrics, _kind: string) => 1;

function metrics(over: Partial<EntityMetrics> = {}): EntityMetrics {
  return {
    loc: 10, fan_in: 0, fan_out: 0, in_cycle: false, method_count: 0, ...over,
  } as EntityMetrics;
}

function node(over: Partial<D3Node> & { id: string }): D3Node {
  return {
    original_id: over.id,
    name: over.id,
    qualified_name: over.id,
    kind: 'function',
    kind_raw: 'Function',
    file_path: 'a.rs',
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
    metrics: metrics(),
    ...over,
  } as D3Node;
}

function scope(path: string, over: Partial<ScopeMetrics> = {}): ScopeMetrics {
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
    ...over,
  } as unknown as ScopeMetrics;
}

function graph(over: Partial<GraphData> = {}): GraphData {
  return { nodes: [], links: [], files: [], folders: [], ...over } as GraphData;
}

test('grain follows the formula, not the node', () => {
  assert.equal(grainOfKind('File'), 'file');
  assert.equal(grainOfKind('Folder'), 'folder');
  assert.equal(grainOfKind('Function'), 'entity');
  assert.equal(grainOfKind('Struct'), 'entity');
});

test('each grain gets its own mean — files never average with entities', () => {
  const b = baselinesFor(
    graph({
      nodes: [node({ id: 'f' }), node({ id: 'g' })],
      files: [scope('a.rs'), scope('b.rs')],
      folders: [scope('src')],
    }),
    scoreScope,
    scoreEntity,
  );
  assert.equal(b.file?.mean, 10);
  assert.equal(b.file?.count, 2);
  assert.equal(b.folder?.mean, 100);
  assert.equal(b.entity?.mean, 1);
  assert.equal(b.entity?.count, 2);
});

test("the backend's own score wins over the fallback formula", () => {
  const b = baselinesFor(
    graph({
      nodes: [node({ id: 'f', metrics: metrics({ composite_score: 3 }) })],
      files: [scope('a.rs', { composite_score: 0.5 } as Partial<ScopeMetrics>)],
    }),
    scoreScope,
    scoreEntity,
  );
  assert.equal(b.file?.mean, 0.5);
  assert.equal(b.entity?.mean, 3);
});

test('scope rollups and synthetic parameters stay out of the entity mean', () => {
  // A File entity scored 99 by the fallback would drag the mean off the entity
  // scale entirely; it is counted through `files` instead. Parameters are
  // near-zero noise and would drag it the other way.
  const b = baselinesFor(
    graph({
      nodes: [
        node({ id: 'f' }),
        node({ id: 'src/a.rs', kind_raw: 'File', metrics: metrics({ composite_score: 99 }) }),
        node({ id: 'src', kind_raw: 'Folder', metrics: metrics({ composite_score: 99 }) }),
        node({ id: 'p', kind_raw: 'Parameter', metrics: metrics({ composite_score: 0 }) }),
      ],
    }),
    scoreScope,
    scoreEntity,
  );
  assert.equal(b.entity?.count, 1);
  assert.equal(b.entity?.mean, 1);
});

test('an empty population has no mean rather than a mean of zero', () => {
  // 0 is the *best* score, so a zero here would report a pristine repo.
  const b = baselinesFor(graph(), scoreScope, scoreEntity);
  assert.equal(b.file, null);
  assert.equal(b.folder, null);
  assert.equal(b.entity, null);
  const none = baselinesFor(null, scoreScope, scoreEntity);
  assert.equal(none.file, null);
});

test('a File node is scored off its rollup, not off its promoted metrics', () => {
  const file = node({
    id: 'src/a.rs',
    kind_raw: 'File',
    // What `collapseGraph` promotes: entity_count posing as field_count.
    metrics: metrics({ field_count: 17, method_count: 5 }),
    scope_metrics: scope('src/a.rs'),
  } as Partial<D3Node> & { id: string });
  assert.equal(nodeScore(file, scoreScope, scoreEntity), 10);

  const folder = node({
    id: 'src',
    kind_raw: 'Folder',
    metrics: metrics(),
    scope_metrics: scope('src'),
  } as Partial<D3Node> & { id: string });
  assert.equal(nodeScore(folder, scoreScope, scoreEntity), 100);
});

test('a scope node with no rollup scores nothing rather than guessing', () => {
  const file = node({ id: 'src/a.rs', kind_raw: 'File', metrics: metrics() });
  assert.equal(nodeScore(file, scoreScope, scoreEntity), undefined);
  assert.equal(nodeScore(node({ id: 'f', metrics: undefined }), scoreScope, scoreEntity), undefined);
});

test('the verdict never contradicts the two decimals printed beside it', () => {
  // Both render as "0.42"; calling that "worse than average" is the bug.
  assert.equal(
    compareToBaseline(0.4241, { grain: 'file', mean: 0.4238, count: 9 }).verdict,
    'at',
  );
  assert.equal(compareToBaseline(0.9, { grain: 'file', mean: 0.4, count: 9 }).verdict, 'above');
  assert.equal(compareToBaseline(0.1, { grain: 'file', mean: 0.4, count: 9 }).verdict, 'below');
});

test('above the mean reads as worse — the scale runs low-is-good', () => {
  const worse = compareToBaseline(1.2, { grain: 'file', mean: 0.4, count: 9 });
  assert.equal(worse.text, 'worse than average');
  assert.ok(worse.delta > 0);
  const better = compareToBaseline(0.2, { grain: 'file', mean: 0.4, count: 9 });
  assert.equal(better.text, 'better than average');
  assert.ok(better.delta < 0);
});

test('the label names the population, not the node', () => {
  assert.equal(baselineNoun('file'), 'file');
  assert.equal(baselineNoun('folder'), 'folder');
  assert.equal(baselineNoun('entity'), 'entity');
});
