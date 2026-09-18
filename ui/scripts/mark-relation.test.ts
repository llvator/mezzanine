/**
 * Unit tests for reading the marked set as a relationship (UI-147).
 *
 * The claims worth pinning down are the ones a reader would act on and be
 * wrong about: which *direction* the dependency runs, that "no direct edge"
 * and "unrelated" are different findings, and that nothing is double-counted
 * on the way from entity edges to a scope-level tally.
 *
 *   npm run test:relate
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  compactSides,
  markRelation,
  scopeLabel,
  type MarkRelation,
} from '../src/viewmodels/markRelation.ts';
import type { D3Link, D3Node } from '../src/types/graph.ts';

/** Enough of a node for the walk: an id, a file, and the tags that change how
 *  it is keyed. */
function node(id: string, file: string, extra: Partial<D3Node> = {}): D3Node {
  return {
    id, original_id: id, name: id, qualified_name: id,
    kind: 'function', kind_raw: 'Function', file_path: file, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags: [], source_code: null, fields: [],
    impl_blocks: [], language: 'Rust',
    ...extra,
  } as D3Node;
}

function ghost(id: string): D3Node {
  return node(id, '', { tags: ['ghost'] });
}

function link(source: string, target: string, kind = 'Calls', extra: Partial<D3Link> = {}): D3Link {
  return {
    source, target, kind: kind.toLowerCase(), kind_raw: kind,
    incoming_kind: 'called by', order: null,
    ...extra,
  } as D3Link;
}

/** `a.ts → b.ts`, one call, and nothing else in the repo. */
function oneWayRepo(): { nodes: D3Node[]; links: D3Link[] } {
  return {
    nodes: [node('a', 'src/a.ts'), node('b', 'src/b.ts')],
    links: [link('a', 'b')],
  };
}

const sideLabels = (r: MarkRelation) => r.sides.map((s) => s.label);

// ── sides ─────────────────────────────────────────────────────────────────

test('a mark inside another mark is one side, not two', () => {
  // Left as two, every node under the file would belong to both and the pair
  // would report edges "between" itself.
  assert.deepEqual(compactSides(['src', 'src/a.ts']), ['src']);
  assert.deepEqual(compactSides(['src/a.ts', 'src/b.ts']), ['src/a.ts', 'src/b.ts']);
});

test('a sibling directory sharing a prefix is still its own side', () => {
  // `src/serve` is not inside `src/server` — only a `/` boundary counts.
  assert.deepEqual(compactSides(['src/server', 'src/serve']), ['src/serve', 'src/server']);
});

test('sides come back in path order, not in the order the marks were made', () => {
  // Side *index* is what every flow, chain and shared count is keyed on, so
  // the order has to be a property of the paths rather than of the gesture.
  assert.deepEqual(compactSides(['src/z.ts', 'src/a.ts']), ['src/a.ts', 'src/z.ts']);
  assert.deepEqual(compactSides(['src/pkg', 'src/a.ts']), ['src/a.ts', 'src/pkg']);
});

test('fewer than two sides after compacting is a non-answer, not a relation', () => {
  const { nodes, links } = oneWayRepo();
  const r = markRelation(nodes, links, ['src/a.ts']);
  assert.equal(r.tone, 'none');
  assert.equal(r.flows.length, 0);
});

test('a side knows whether it is one file or a folder of them', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/pkg/b.ts'), node('c', 'src/pkg/c.ts')];
  const r = markRelation(nodes, [], ['src/a.ts', 'src/pkg']);
  assert.deepEqual(r.sides.map((s) => s.grain), ['file', 'folder']);
  assert.deepEqual(r.sides.map((s) => s.files), [1, 2]);
  assert.deepEqual(r.sides.map((s) => s.entities), [1, 2]);
});

// ── direct flow ───────────────────────────────────────────────────────────

test('a one-way dependency names the direction and says nothing comes back', () => {
  const { nodes, links } = oneWayRepo();
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.tone, 'one-way');
  assert.equal(r.flows.length, 1);
  assert.equal(sideLabels(r)[r.flows[0].from], 'a.ts');
  assert.equal(sideLabels(r)[r.flows[0].to], 'b.ts');
  assert.match(r.verdict, /a\.ts depends on b\.ts/);
});

test('edges both ways read as a cycle rather than as two dependencies', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts')];
  const links = [link('a', 'b'), link('b', 'a')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.tone, 'mutual');
  assert.equal(r.flows.length, 2);
  assert.match(r.verdict, /cycle/);
});

test('the flow is broken down by kind, heaviest first', () => {
  const nodes = [node('a1', 'src/a.ts'), node('a2', 'src/a.ts'), node('b', 'src/b.ts')];
  const links = [link('a1', 'b'), link('a2', 'b'), link('a1', 'b', 'UsesType')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.flows[0].total, 3);
  assert.deepEqual(r.flows[0].kinds.map((k) => [k.kind, k.count]), [['Calls', 2], ['UsesType', 1]]);
});

test('the flow lists the entity pairs, which is what a reader opens', () => {
  const { nodes, links } = oneWayRepo();
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.deepEqual(
    r.flows[0].pairs.map((p) => `${p.sourceName} ${p.label} ${p.targetName}`),
    ['a calls b'],
  );
  assert.equal(r.flows[0].more, 0);
});

test('a lifted twin is not counted a second time at scope grain', () => {
  // UI-113: the twin says what its real edge already said, one node out. Both
  // land on the same scope pair here.
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts')];
  const links = [link('a', 'b'), link('a', 'b', 'Calls', { lifted_from: ['branch'] })];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.flows[0].total, 1);
});

test('edges inside one side are not a relationship between the sides', () => {
  const nodes = [node('a1', 'src/pkg/a.ts'), node('a2', 'src/pkg/b.ts'), node('c', 'src/c.ts')];
  const r = markRelation(nodes, [link('a1', 'a2')], ['src/pkg', 'src/c.ts']);
  assert.equal(r.flows.length, 0);
});

// ── shared neighbours ─────────────────────────────────────────────────────

test('a dependency only one side has is not shared', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('u', 'src/util.ts')];
  const r = markRelation(nodes, [link('a', 'u')], ['src/a.ts', 'src/b.ts']);
  assert.equal(r.sharedDeps.files, 0);
});

test('two scopes with no edge between them are siblings when they share ground', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('u', 'src/util.ts')];
  const links = [link('a', 'u'), link('b', 'u'), link('b', 'u', 'UsesType')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.tone, 'siblings');
  assert.equal(r.sharedDeps.files, 1);
  assert.deepEqual(r.sharedDeps.rows[0].perSide, [1, 2]);
  assert.equal(r.sharedDeps.rows[0].path, 'src/util.ts');
  assert.equal(r.sharedDeps.rows[0].external, false);
});

test('a library both sides import is a shared dependency too', () => {
  // A ghost has no file and so can never be a side, but it is the most telling
  // thing two unconnected files can have in common.
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), ghost('serde')];
  const links = [link('a', 'serde', 'Imports'), link('b', 'serde', 'Imports')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.sharedDeps.rows.length, 1);
  assert.equal(r.sharedDeps.rows[0].external, true);
  assert.equal(r.sharedDeps.rows[0].label, 'serde');
});

test('who depends on both is read the other way round', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('m', 'src/main.ts')];
  const links = [link('m', 'a'), link('m', 'b')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.sharedDeps.files, 0);
  assert.equal(r.sharedDependents.files, 1);
  assert.equal(r.sharedDependents.rows[0].path, 'src/main.ts');
});

test('repo files outrank the standard library, however heavy it is', () => {
  // Found on this repo's own graph: two files in `src/server` shared eleven
  // dependencies, ten of which were `String`, `Ok`, `Vec`, `Some`. The one row
  // that said anything was a file, and it sorted last.
  const nodes = [
    node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('u', 'src/util.ts'),
    ghost('String'), ghost('Some'),
  ];
  const links = [
    link('a', 'u'), link('b', 'u'),
    ...Array.from({ length: 40 }, (_, i) => link(i % 2 ? 'a' : 'b', 'String')),
    ...Array.from({ length: 30 }, (_, i) => link(i % 2 ? 'a' : 'b', 'Some')),
  ];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.sharedDeps.rows[0].path, 'src/util.ts');
  assert.deepEqual(r.sharedDeps.rows.map((s) => s.external), [false, true, true]);
  assert.equal(r.sharedDeps.files, 1);
  assert.equal(r.sharedDeps.external, 2);
});

test('the verdict counts files, and admits when the overlap is all external', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), ghost('String')];
  const links = [link('a', 'String'), link('b', 'String')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  // "they share 1 dependency" would read as a fact about this repo. It is a
  // fact about the language.
  assert.match(r.verdict, /1 external dependency/);
});

test('a third marked side that does not share it drops it from the list', () => {
  // Intersection, not union — otherwise a reader concludes all three stand on
  // something only two of them do.
  const nodes = [
    node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('c', 'src/c.ts'), node('u', 'src/util.ts'),
  ];
  const links = [link('a', 'u'), link('b', 'u')];
  const two = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  const three = markRelation(nodes, links, ['src/a.ts', 'src/b.ts', 'src/c.ts']);
  assert.equal(two.sharedDeps.files, 1);
  assert.equal(three.sharedDeps.files, 0);
});

// ── chains ────────────────────────────────────────────────────────────────

test('no edge and nothing shared still finds the route through a mediator', () => {
  const nodes = [node('a', 'src/a.ts'), node('m', 'src/mid.ts'), node('b', 'src/b.ts')];
  const links = [link('a', 'm'), link('m', 'b')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.tone, 'indirect');
  assert.equal(r.chains.length, 1);
  assert.deepEqual(r.chains[0].via.map((v) => v.path), ['src/mid.ts']);
  assert.match(r.verdict, /2 hops, through mid\.ts/);
});

test('a chain is directed — a shared dependency is not a route', () => {
  // `a → u ← b` means they agree about `u`, not that a reaches b.
  const nodes = [node('a', 'src/a.ts'), node('u', 'src/util.ts'), node('b', 'src/b.ts')];
  const r = markRelation(nodes, [link('a', 'u'), link('b', 'u')], ['src/a.ts', 'src/b.ts']);
  assert.equal(r.chains.length, 0);
  assert.equal(r.tone, 'siblings');
});

test('a file at the repo root keeps its own name', () => {
  // The parent has to be taken from the last `/`, not by arithmetic: with no
  // directory to borrow, subtracting the basename slices into the name and
  // answers `main.r/main.rs`.
  assert.equal(scopeLabel('mod.rs'), 'mod.rs');
  assert.equal(scopeLabel('src/main.rs'), 'main.rs');
});

test('a route through mod.rs says which mod.rs', () => {
  // `handlers.rs → mod.rs → main.rs → mod.rs` is what the last path segment
  // gives you in a Rust repo, and it reads as a loop between two files that
  // are not the same file.
  const nodes = [
    node('a', 'src/a.ts'), node('m', 'src/server/mod.rs'), node('b', 'src/b.ts'),
  ];
  const r = markRelation(nodes, [link('a', 'm'), link('m', 'b')], ['src/a.ts', 'src/b.ts']);
  assert.equal(r.chains[0].via[0].label, 'server/mod.rs');
  assert.equal(r.chains[0].via[0].path, 'src/server/mod.rs');
});

test('a three-hop route is found when no two-hop one exists', () => {
  const nodes = [
    node('a', 'src/a.ts'), node('m', 'src/m.ts'), node('n', 'src/n.ts'), node('b', 'src/b.ts'),
  ];
  const links = [link('a', 'm'), link('m', 'n'), link('n', 'b')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.deepEqual(r.chains[0].via.map((v) => v.label), ['m.ts', 'n.ts']);
});

test('a shorter route wins outright over the longer one past it', () => {
  const nodes = [
    node('a', 'src/a.ts'), node('m', 'src/m.ts'), node('n', 'src/n.ts'), node('b', 'src/b.ts'),
  ];
  const links = [link('a', 'm'), link('m', 'b'), link('m', 'n'), link('n', 'b')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.chains.length, 1);
  assert.deepEqual(r.chains[0].via.map((v) => v.label), ['m.ts']);
});

test('a chain is only as strong as its thinnest hop', () => {
  const nodes = [
    node('a', 'src/a.ts'), node('a2', 'src/a.ts'),
    node('m', 'src/m.ts'), node('b', 'src/b.ts'),
  ];
  const links = [link('a', 'm'), link('a2', 'm'), link('m', 'b')];
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.chains[0].strength, 1);
});

test('a direct edge is not restated as a chain', () => {
  const { nodes, links } = oneWayRepo();
  const r = markRelation(nodes, links, ['src/a.ts', 'src/b.ts']);
  assert.equal(r.chains.filter((c) => c.from === 0 && c.to === 1).length, 0);
});

// ── the honest nothing ────────────────────────────────────────────────────

test('genuinely unrelated says so, and says how far it looked', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts')];
  const r = markRelation(nodes, [], ['src/a.ts', 'src/b.ts']);
  assert.equal(r.tone, 'independent');
  assert.match(r.verdict, /Independent/);
  assert.match(r.verdict, /3 hops/);
});

test('three or more sides get arithmetic instead of a sentence about two', () => {
  const nodes = [node('a', 'src/a.ts'), node('b', 'src/b.ts'), node('c', 'src/c.ts')];
  const r = markRelation(nodes, [link('a', 'b')], ['src/a.ts', 'src/b.ts', 'src/c.ts']);
  assert.equal(r.verdict, '3 marked · 1 of 3 pairs directly linked');
});
