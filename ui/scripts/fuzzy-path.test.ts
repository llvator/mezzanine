/**
 * Unit tests for the fuzzy matcher (UI-043).
 *
 * Zero dependencies, in the spirit of `ux-probe.mjs`: Node's built-in test
 * runner plus its TypeScript type-stripping, so nothing is added to
 * package.json. Ranking assertions are the point — a matcher that returns
 * the right *set* and the wrong *order* is only half a matcher, and order is
 * exactly what a hand-check in the browser is worst at judging.
 *
 *   npm run test:fuzzy
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  parseQuery, scoreQuery, matchesQuery, isPurelyNegative, violatesNegation,
} from '../src/utils/fuzzyPath.ts';

/** Rank candidates best-first, dropping non-matches. */
function rank(query: string, candidates: string[]): string[] {
  const terms = parseQuery(query);
  return candidates
    .map((c) => ({ c, s: scoreQuery(terms, c) }))
    .filter((r): r is { c: string; s: number } => r.s !== null)
    .sort((a, b) => b.s - a.s || a.c.localeCompare(b.c))
    .map((r) => r.c);
}

function matches(query: string, candidate: string): boolean {
  return matchesQuery(parseQuery(query), candidate);
}

test('empty query yields no terms and matches nothing', () => {
  assert.equal(parseQuery('').length, 0);
  assert.equal(parseQuery('   ').length, 0);
  assert.equal(scoreQuery(parseQuery(''), 'anything'), null);
});

test('fuzzy subsequence matches out-of-order-looking abbreviations', () => {
  assert.ok(matches('uicmpsv', 'ui/src/components/ScopeTree.svelte'));
  assert.ok(matches('dP', 'ui/src/viewmodels/displayPlan.ts'));
  assert.ok(!matches('zzz', 'ui/src/viewmodels/displayPlan.ts'));
});

test('subsequence must be in order', () => {
  assert.ok(matches('scope', 'ui/src/stores/scope.ts'));
  assert.ok(!matches('epocs', 'ui/src/stores/scope.ts'));
});

test('ranks the structural match above an incidental subsequence', () => {
  const ranked = rank('uicmpsv', [
    'src/parser/typescript/calls.rs',           // incidental letters only
    'ui/src/components/ScopeTree.svelte',       // the intended answer
    'ui/src/viewmodels/collapseGraph.ts',
  ]);
  assert.equal(ranked[0], 'ui/src/components/ScopeTree.svelte');
});

test('prefers a boundary match over a mid-word one', () => {
  const ranked = rank('graph', [
    'ui/src/viewmodels/collapseGraph.ts',
    'ui/src/stores/graph.ts',
  ]);
  assert.equal(ranked[0], 'ui/src/stores/graph.ts');
});

test('shorter candidate wins an otherwise equal match', () => {
  const ranked = rank('scope', [
    'ui/src/viewmodels/scopeTreeViewModelHelpers.ts',
    'ui/src/stores/scope.ts',
  ]);
  assert.equal(ranked[0], 'ui/src/stores/scope.ts');
});

test('multiple terms are ANDed', () => {
  assert.ok(matches('ui svelte', 'ui/src/components/ScopeTree.svelte'));
  assert.ok(!matches('ui svelte', 'src/parser/svelte/mod.rs'));
  assert.ok(!matches('ui rust', 'ui/src/components/ScopeTree.svelte'));
});

test('negation rejects', () => {
  assert.ok(matches('ui !test', 'ui/src/components/ScopeTree.svelte'));
  assert.ok(!matches('ui !test', 'ui/src/components/ScopeTree.test.svelte'));
  // A term that only negates still matches everything else.
  assert.ok(matches('!test', 'ui/src/stores/scope.ts'));
  assert.ok(isPurelyNegative(parseQuery('!test !spec')));
  assert.ok(!isPurelyNegative(parseQuery('ui !test')));
});

test('negation is literal, not fuzzy', () => {
  // The trap this guards: `test` IS a fuzzy subsequence of
  // `ui/src/componen[t]s/Scop[e]Tree.[s]vel[t]e`, so a fuzzy `!test` would
  // reject nearly every path in the repo — the opposite of what anyone
  // typing it wants.
  assert.ok(matchesQuery(parseQuery('test'), 'ui/src/components/ScopeTree.svelte'),
    'precondition: `test` fuzzy-matches this path');
  assert.ok(matches('!test', 'ui/src/components/ScopeTree.svelte'));
  assert.ok(!matches('!test', 'ui/src/components/ScopeTree.test.svelte'));
});

test('violatesNegation checks the committed string, not the matched one', () => {
  // The scope query scores an entity on its name and then scopes to the file
  // declaring it. `^src !parser` matched an entity called `src` — whose name
  // contains no "parser" — and dragged src/parser/… into the results.
  const terms = parseQuery('^src !parser');
  assert.ok(matchesQuery(terms, 'src'), 'precondition: the entity name passes');
  assert.ok(violatesNegation(terms, 'src/parser/elevator/lexer.rs'),
    'but the path it would commit is excluded');
  assert.ok(!violatesNegation(terms, 'src/graph.rs'));
  // No negated terms means nothing to violate.
  assert.ok(!violatesNegation(parseQuery('^src'), 'src/parser/mod.rs'));
});

test("' is literal, not fuzzy", () => {
  assert.ok(matches("'FileTree", 'ui/src/components/FileTree.svelte'));
  // fuzzy would accept this; the literal form must not
  assert.ok(matches('FileTree', 'ui/src/fileTypeRestrictions.ts') === false
    || !matches("'FileTree", 'ui/src/fileTypeRestrictions.ts'));
  assert.ok(!matches("'FileTree", 'ui/src/fileTypeRestrictions.ts'));
});

test('anchors', () => {
  assert.ok(matches('^ui', 'ui/src/stores/scope.ts'));
  assert.ok(!matches('^ui', 'src/parser/ui.rs'));
  assert.ok(matches('.ts$', 'ui/src/stores/scope.ts'));
  assert.ok(!matches('.ts$', 'ui/src/components/ScopeTree.svelte'));
  // both anchors together mean equality
  assert.ok(matches('^ui/src/stores/scope.ts$', 'ui/src/stores/scope.ts'));
  assert.ok(!matches('^ui/src/stores/scope.ts$', 'ui/src/stores/scope.ts.bak'));
});

test('smart case', () => {
  // all-lowercase query is case-insensitive
  assert.ok(matches('scopetree', 'ui/src/components/ScopeTree.svelte'));
  // a query carrying an uppercase char is case-sensitive
  assert.ok(matches('ScopeTree', 'ui/src/components/ScopeTree.svelte'));
  assert.ok(!matches('ScopeTree', 'ui/src/components/scopetree.svelte'));
});

test('operators combine', () => {
  assert.ok(matches("!'test .ts$", 'ui/src/stores/scope.ts'));
  assert.ok(!matches("!'test .ts$", 'ui/src/stores/scope.test.ts'));
});

test('parse is memoised without going stale', () => {
  const a = parseQuery('ui svelte');
  const b = parseQuery('ui svelte');
  assert.equal(a, b, 'same query should hit the memo');
  const c = parseQuery('ui rust');
  assert.notEqual(a, c);
  assert.equal(c[1].needle, 'rust');
});

test('scoring is stable regardless of candidate order', () => {
  const candidates = [
    'ui/src/stores/graph.ts',
    'ui/src/viewmodels/collapseGraph.ts',
    'src/graph.rs',
  ];
  const forward = rank('graph', candidates);
  const backward = rank('graph', [...candidates].reverse());
  assert.deepEqual(forward, backward);
});
