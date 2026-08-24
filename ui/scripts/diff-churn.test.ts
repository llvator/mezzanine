/**
 * Unit tests for the `Lines changed` size channel's domain (`diffChurn`).
 *
 * The case that justifies the whole module is the first one: `diff.json`
 * already carries a `loc` delta, and on a same-length rewrite that delta is
 * zero. Sizing on it would draw the biggest change in the commit as the
 * smallest circle on the canvas — a wrong answer that looks like a confident
 * one, which is the failure mode a picture is worst at admitting to.
 *
 * The rest is the zero-versus-unknown line, which this channel has to hold at
 * two grains: an unchanged entity really is zero and belongs at the bottom of
 * the ramp, while an entity whose before-source could not be read is not, and
 * has to fall out of the map so the canvas draws it hollow.
 *
 * Same zero-dependency setup as the sibling suites: `diffChurn` imports only
 * `diffRollup` and `lineDiff`, both store-free, and takes its id normalizer
 * and both source lookups as arguments — so nothing here reaches svelte.
 *
 *   npm run test:churn
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { buildChurnIndex, rowChurn, type ChurnSources } from '../src/viewmodels/diffChurn.ts';
import type { EntityDiff } from '../src/stores/diff.ts';

type Row = Partial<EntityDiff> & Pick<EntityDiff, 'entity_id' | 'status'>;

function row(r: Row): EntityDiff {
  return {
    name: 'f',
    kind: 'Function',
    file_path: 'src/a.rs',
    source_changed: r.status !== 'unchanged',
    metric_deltas: [],
    ...r,
  } as EntityDiff;
}

/** Sources as two plain tables, keyed the way the real ones are: head by the
 *  row's own id, base by its `base_entity_id`. */
function sources(
  head: Record<string, string>,
  base: Record<string, string> = {},
): ChurnSources {
  return {
    head: (e) => head[e.entity_id],
    base: (e) => (e.base_entity_id ? base[e.base_entity_id] : undefined),
    key: (e) => e.entity_id,
  };
}

test('a same-length rewrite is churn, where the loc delta is zero', () => {
  const before = 'fn a() {\n  one();\n  two();\n}';
  const after = 'fn a() {\n  three();\n  four();\n}';
  // What the engine would report: identical line counts, so `loc` delta 0.
  assert.equal(before.split('\n').length, after.split('\n').length);

  const e = row({ entity_id: 'src/a.rs:1:a', status: 'modified', base_entity_id: 'b' });
  // Two lines replaced: two removed and two added.
  assert.equal(rowChurn(e, sources({ 'src/a.rs:1:a': after }, { b: before })), 4);
});

test('an unchanged entity is zero, and present — not missing', () => {
  const e = row({ entity_id: 'src/a.rs:1:a', status: 'unchanged' });
  const idx = buildChurnIndex([e], sources({}));
  assert.equal(idx.byEntity.get('src/a.rs:1:a'), 0);
  assert.ok(idx.byEntity.has('src/a.rs:1:a'));
});

test('an unreadable before-source is unknown, and absent — not zero', () => {
  // The distinction the canvas draws as hollow rather than as smallest.
  const e = row({ entity_id: 'src/a.rs:1:a', status: 'modified', base_entity_id: 'b' });
  const idx = buildChurnIndex([e], sources({ 'src/a.rs:1:a': 'fn a() {}' }));
  assert.equal(idx.byEntity.has('src/a.rs:1:a'), false);
});

test('an entity that arrived or left is every line of itself', () => {
  const added = row({ entity_id: 'src/a.rs:1:new', status: 'added' });
  const removed = row({ entity_id: 'src/a.rs:9:old', status: 'removed', base_entity_id: 'b' });
  const src = sources({ 'src/a.rs:1:new': 'a\nb\nc' }, { b: 'x\ny' });
  assert.equal(rowChurn(added, src), 3);
  assert.equal(rowChurn(removed, src), 2);
});

test('a node that only moved because its neighbours did has moved no lines', () => {
  // `source_changed: false` is the fan_in/fan_out ripple. Zero is the honest
  // answer and keeps the ripple small instead of drawing it as unknown.
  const e = row({
    entity_id: 'src/a.rs:1:a', status: 'modified',
    source_changed: false, base_entity_id: 'b',
  });
  assert.equal(rowChurn(e, sources({}, {})), 0);
});

test('a scope carries the sum of the entities inside it, file and folder', () => {
  const rows = [
    row({ entity_id: 'src/a.rs:1:a', status: 'added' }),
    row({ entity_id: 'src/a.rs:9:b', status: 'unchanged' }),
    row({ entity_id: 'src/b.rs:1:c', status: 'added', file_path: 'src/b.rs' }),
  ];
  const idx = buildChurnIndex(rows, sources({
    'src/a.rs:1:a': 'one\ntwo',
    'src/b.rs:1:c': 'three\nfour\nfive',
  }));
  assert.equal(idx.byScope.get('src/a.rs'), 2);
  assert.equal(idx.byScope.get('src/b.rs'), 3);
  // The directory holding both — the Module grain the collapsed canvas draws.
  assert.equal(idx.byScope.get('src'), 5);
});

test('a scope whose only change could not be measured is unknown too', () => {
  const rows = [row({ entity_id: 'src/a.rs:1:a', status: 'modified', base_entity_id: 'b' })];
  const idx = buildChurnIndex(rows, sources({ 'src/a.rs:1:a': 'fn a() {}' }));
  assert.equal(idx.byScope.has('src/a.rs'), false);
});

test('a scope nothing touched is zero, and present', () => {
  const rows = [row({ entity_id: 'src/a.rs:1:a', status: 'unchanged' })];
  const idx = buildChurnIndex(rows, sources({}));
  assert.equal(idx.byScope.get('src/a.rs'), 0);
});
