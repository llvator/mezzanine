/**
 * Unit tests for the scope rule list (UI-045 / ADR 0009).
 *
 * Same zero-dependency setup as `fuzzy-path.test.ts`: Node's built-in runner
 * plus type stripping. The interesting properties here are ordering ones —
 * last-match-wins, and whether compaction can change a verdict — and those
 * are exactly what a click-through in the browser cannot demonstrate.
 *
 *   npm run test:scope
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  isInScope, isDirectRule, compactRules, toggleRule, includeAll,
  patternMatches, hasExclusionInside,
} from '../src/utils/scopeRules.ts';
import type { ScopeRule } from '../src/utils/scopeRules.ts';

const inc = (pattern: string): ScopeRule => ({ pattern, negate: false });
const exc = (pattern: string): ScopeRule => ({ pattern, negate: true });

test('empty rule list is an empty scope', () => {
  assert.equal(isInScope('ui/src/stores/scope.ts', []), false);
});

test('prefix rules cover the subtree, not just the exact path', () => {
  const rules = [inc('ui/src')];
  assert.ok(isInScope('ui/src', rules));
  assert.ok(isInScope('ui/src/stores/scope.ts', rules));
  assert.ok(!isInScope('ui/scripts/ux-probe.mjs', rules));
  // a prefix must stop at a separator — `ui/s` is not a parent of `ui/src`
  assert.ok(!isInScope('ui/src/stores/scope.ts', [inc('ui/s')]));
});

test('root includes everything', () => {
  assert.ok(isInScope('anything/at/all.rs', [inc('')]));
});

test('last match wins', () => {
  const rules = [inc('ui'), exc('ui/src/legacy')];
  assert.ok(isInScope('ui/src/stores/scope.ts', rules));
  assert.ok(!isInScope('ui/src/legacy/old.ts', rules));
  // and a later include puts a branch of the exclusion back
  const reinstated = [...rules, inc('ui/src/legacy/keep.ts')];
  assert.ok(isInScope('ui/src/legacy/keep.ts', reinstated));
  assert.ok(!isInScope('ui/src/legacy/old.ts', reinstated));
});

test('order matters — the same rules reversed mean something else', () => {
  const a = [inc('ui'), exc('ui/src')];
  const b = [exc('ui/src'), inc('ui')];
  assert.equal(isInScope('ui/src/x.ts', a), false);
  assert.equal(isInScope('ui/src/x.ts', b), true);
});

test('this is the case the old model could not express', () => {
  // "everything under ui/ except ui/src/legacy", in two rules rather than
  // thirty enumerated siblings — and a file added under the excluded branch
  // afterwards stays excluded, which is what used to decay.
  const rules = [inc('ui'), exc('ui/src/legacy')];
  assert.ok(!isInScope('ui/src/legacy/added-later.ts', rules));
  assert.equal(rules.length, 2);
});

test('globs', () => {
  assert.ok(patternMatches('ui/**/*.svelte', 'ui/src/components/FileTree.svelte'));
  assert.ok(!patternMatches('ui/*/x.ts', 'ui/a/b/x.ts'), '* must not cross a separator');
  assert.ok(patternMatches('ui/**/x.ts', 'ui/a/b/x.ts'));
  // a leading **/ also has to match at the root
  assert.ok(patternMatches('**/*.test.ts', 'foo.test.ts'));
  assert.ok(patternMatches('**/*.test.ts', 'a/b/foo.test.ts'));
  assert.ok(!patternMatches('**/*.test.ts', 'foo.ts'));
});

test('glob exclusion is durable against paths that did not exist yet', () => {
  const rules = [inc('ui'), exc('**/*.test.ts')];
  assert.ok(isInScope('ui/src/stores/scope.ts', rules));
  assert.ok(!isInScope('ui/src/stores/scope.test.ts', rules));
});

test('toggleRule flips, and flipping twice returns to the start', () => {
  let rules = includeAll(['ui']);
  assert.ok(isInScope('ui/src/x.ts', rules));
  rules = toggleRule(rules, 'ui/src');
  assert.ok(!isInScope('ui/src/x.ts', rules), 'first toggle excludes');
  rules = toggleRule(rules, 'ui/src');
  assert.ok(isInScope('ui/src/x.ts', rules), 'second toggle restores');
});

test('repeated toggling does not grow the list', () => {
  let rules = includeAll(['ui']);
  for (let i = 0; i < 20; i++) rules = toggleRule(rules, 'ui/src');
  assert.ok(rules.length <= 2, `list grew to ${rules.length}`);
});

test('compaction never changes a verdict', () => {
  const paths = [
    'ui/src/stores/scope.ts', 'ui/src/legacy/old.ts', 'ui/src/legacy/keep.ts',
    'src/parser/mod.rs', 'ui/scripts/probe.mjs', 'README.md',
  ];
  const cases: ScopeRule[][] = [
    [inc('ui'), exc('ui/src/legacy'), inc('ui/src/legacy/keep.ts')],
    [inc('ui'), inc('ui/src'), inc('ui/src/stores')],
    [inc('ui'), exc('ui'), inc('ui')],
    [inc('src'), inc(''), exc('ui/src/legacy')],
    [inc('ui'), exc('**/*.ts'), inc('ui/src/legacy')],
  ];
  for (const rules of cases) {
    const compacted = compactRules(rules);
    for (const p of paths) {
      assert.equal(isInScope(p, compacted), isInScope(p, rules),
        `${p} changed under ${JSON.stringify(rules)} -> ${JSON.stringify(compacted)}`);
    }
  }
});

test('compaction drops redundant includes but keeps reinstating ones', () => {
  assert.deepEqual(
    compactRules([inc('ui'), inc('ui/src')]).map((r) => r.pattern),
    ['ui'], 'an include already covered is redundant');
  const reinstating = compactRules([inc('ui'), exc('ui/src'), inc('ui/src/stores')]);
  assert.equal(reinstating.length, 3, 'the include that undoes an exclusion must survive');
  assert.ok(isInScope('ui/src/stores/scope.ts', reinstating));
});

test('a root include clears what came before it', () => {
  const r = compactRules([inc('ui'), exc('ui/src'), inc('')]);
  assert.deepEqual(r.map((x) => x.pattern), ['']);
  assert.ok(isInScope('ui/src/x.ts', r));
});

test('isDirectRule distinguishes a named path from an inherited one', () => {
  const rules = [inc('ui')];
  assert.ok(isDirectRule('ui', rules));
  assert.ok(!isDirectRule('ui/src', rules));
  assert.ok(isInScope('ui/src', rules), 'still in scope, just not named');
});

test('hasExclusionInside guards the count fast-path', () => {
  assert.ok(!hasExclusionInside('ui', [inc('ui')]));
  assert.ok(hasExclusionInside('ui', [inc('ui'), exc('ui/src/legacy')]));
  assert.ok(!hasExclusionInside('src', [inc(''), exc('ui/src/legacy')]));
  // globs are opaque, so assume they might bite
  assert.ok(hasExclusionInside('src', [inc(''), exc('**/*.test.ts')]));
});
