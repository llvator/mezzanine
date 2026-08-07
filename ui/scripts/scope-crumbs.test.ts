/**
 * Unit tests for the scope address — the crumbs that say where the reader is
 * (UI-095).
 *
 * The failure worth guarding is a strip that *lies*: one that shows a folder
 * path when the scope is six marked files from three folders, or a bare
 * `repo` when nothing is selected and the canvas is blank. Both read as a
 * confident answer, and nothing else on screen contradicts either.
 *
 *   npm run test:crumbs
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ScopeRule } from '../src/utils/scopeRules.ts';
import {
  MAX_CRUMBS,
  ROOT_LABEL,
  crumbTitle,
  crumbsFor,
  elideCrumbs,
  hiddenTitle,
  scopeAddress,
} from '../src/viewmodels/scopeCrumbs.ts';

const inc = (...paths: string[]): ScopeRule[] => paths.map((pattern) => ({ pattern, negate: false }));
const not = (pattern: string): ScopeRule => ({ pattern, negate: true });
const labels = (cs: { label: string }[]) => cs.map((c) => c.label);

// --- which shape a scope has ---

test('an empty scope has no address — it is not the repo', () => {
  // A bare `repo` crumb over a blank canvas claims the opposite of the truth.
  assert.deepEqual(scopeAddress([]), { kind: 'empty' });
});

test('a scope of nothing but exclusions is still empty', () => {
  assert.deepEqual(scopeAddress([not('ui/tests')]), { kind: 'empty' });
});

test('one plain path is climbable', () => {
  const a = scopeAddress(inc('ui/src/stores'));
  assert.equal(a.kind, 'path');
  if (a.kind !== 'path') return;
  assert.deepEqual(labels(a.crumbs), [ROOT_LABEL, 'ui', 'src', 'stores']);
});

test('several paths have no single address and are not given one', () => {
  const a = scopeAddress(inc('ui/src/a.ts', 'ui/tests/b.ts', 'docs/c.md'));
  assert.deepEqual(a, { kind: 'opaque', label: '3 paths', filtered: false });
});

test('a glob is a rule, not a place', () => {
  const a = scopeAddress(inc('ui/**/*.test.ts'));
  assert.equal(a.kind, 'opaque');
  if (a.kind !== 'opaque') return;
  assert.equal(a.label, 'ui/**/*.test.ts');
});

test('an exclusion leaves the address alone and flags it as filtered', () => {
  const a = scopeAddress([...inc('ui/src'), not('ui/src/tests')]);
  assert.equal(a.kind, 'path');
  if (a.kind !== 'path') return;
  assert.equal(a.filtered, true);
  assert.deepEqual(labels(a.crumbs), [ROOT_LABEL, 'ui', 'src']);
});

// --- the crumbs themselves ---

test('the root scope is one crumb, and it is where you are', () => {
  const cs = crumbsFor('');
  assert.deepEqual(cs, [{ path: '', label: ROOT_LABEL, current: true }]);
});

test('every crumb carries the path focusing it would scope to', () => {
  assert.deepEqual(
    crumbsFor('ui/src/stores').map((c) => c.path),
    ['', 'ui', 'ui/src', 'ui/src/stores'],
  );
});

test('only the deepest crumb is current', () => {
  const cs = crumbsFor('ui/src/stores');
  assert.deepEqual(cs.map((c) => c.current), [false, false, false, true]);
});

test('a file path addresses the file, not the folder holding it', () => {
  // Drilling into one file is a real scope, and the strip has to say so
  // rather than rounding up to its parent.
  const cs = crumbsFor('ui/src/stores/scope.ts');
  assert.equal(cs[cs.length - 1].label, 'scope.ts');
  assert.equal(cs[cs.length - 1].current, true);
});

// --- elision ---

test('a short address is shown whole', () => {
  const cs = crumbsFor('ui/src');
  assert.deepEqual(elideCrumbs(cs), { hidden: [], shown: cs });
});

test('a deep address loses its middle, never its depth', () => {
  const cs = crumbsFor('a/b/c/d/e/f/g');
  const { hidden, shown } = elideCrumbs(cs);
  assert.equal(shown.length, MAX_CRUMBS);
  assert.equal(shown[0].label, ROOT_LABEL);
  assert.equal(shown[shown.length - 1].label, 'g');
  assert.equal(shown[shown.length - 1].current, true);
  assert.deepEqual(labels(hidden), ['a', 'b', 'c']);
});

test('nothing is lost — hidden and shown account for every crumb', () => {
  const cs = crumbsFor('a/b/c/d/e/f/g/h/i');
  const { hidden, shown } = elideCrumbs(cs);
  assert.equal(hidden.length + shown.length, cs.length);
});

test('the ellipsis says what it is hiding', () => {
  const { hidden } = elideCrumbs(crumbsFor('a/b/c/d/e/f/g'));
  const title = hiddenTitle(hidden);
  assert.match(title, /3 more levels/);
  assert.match(title, /a › b › c/);
});

test('nothing hidden says nothing', () => {
  assert.equal(hiddenTitle([]), '');
});

// --- what the crumbs say ---

test('a climbable crumb names its destination, not its own label', () => {
  const cs = crumbsFor('ui/src/stores');
  assert.equal(crumbTitle(cs[2]), 'Focus ui/src');
});

test('the root crumb says it is the whole repo, not an empty path', () => {
  assert.match(crumbTitle(crumbsFor('ui')[0]), /whole repo/);
});

test('the current crumb offers a location rather than a gesture', () => {
  const cs = crumbsFor('ui/src');
  assert.match(crumbTitle(cs[cs.length - 1]), /^You are here: ui\/src$/);
});
