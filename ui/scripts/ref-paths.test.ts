/**
 * Unit tests for code-reference path matching (`utils/refPaths.ts`).
 *
 * These exist because the bug they pin was invisible. `buildPathUniverse`
 * compared raw `file_path`s against refs that had already been normalized,
 * so under a bare `nao watch` — root `.`, paths emitted as `./src/…` — the
 * universe matched nothing and the UI marked every reference in the graph as
 * drift. A healthy-looking panel full of wrong badges; nothing crashed and
 * nothing was slow.
 *
 * The logic was inline in `stores/codeRefs.ts` at the time, reachable only by
 * booting the Svelte store graph, which is why nothing tested it and why it
 * lives in `utils/` now.
 *
 *   npm run test:refpaths
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  normalizeRefPath,
  buildPathUniverse,
  pathClaims,
  bySpecificity,
} from '../src/utils/refPaths.ts';

// ---------------------------------------------------------------------
// normalizeRefPath
// ---------------------------------------------------------------------

test('a dot-slash prefix is not part of the path', () => {
  assert.equal(normalizeRefPath('./src/parser'), 'src/parser');
});

test('a trailing slash is not part of the path', () => {
  assert.equal(normalizeRefPath('src/parser/'), 'src/parser');
});

test('the two spellings authors use compare equal', () => {
  assert.equal(normalizeRefPath('./src/parser/'), normalizeRefPath('src/parser'));
});

test('surrounding whitespace is not part of the path', () => {
  assert.equal(normalizeRefPath('  src/parser  '), 'src/parser');
});

test('an empty or missing path normalizes to empty rather than throwing', () => {
  assert.equal(normalizeRefPath(''), '');
  assert.equal(normalizeRefPath(undefined as unknown as string), '');
});

test('a dot segment inside the path is left alone', () => {
  // Only a *leading* `./` is decoration. `a/./b` is a real, if odd, path
  // and rewriting it would be a guess.
  assert.equal(normalizeRefPath('a/./b'), 'a/./b');
});

// ---------------------------------------------------------------------
// buildPathUniverse — the regression this file was written for
// ---------------------------------------------------------------------

test('a file contributes every ancestor folder, so a folder ref resolves', () => {
  const universe = buildPathUniverse(['src/parser/rust/mod.rs']);
  assert.ok(universe.has('src/parser/rust/mod.rs'));
  assert.ok(universe.has('src/parser/rust'));
  assert.ok(universe.has('src/parser'));
  assert.ok(universe.has('src'));
});

test('a dot-slash file path resolves a ref written without one', () => {
  // The regression. Before the fix the universe held './src/parser' and the
  // ref said 'src/parser', so this was false and the ref read as drift.
  const universe = buildPathUniverse(['./src/parser/rust/mod.rs']);
  assert.ok(universe.has(normalizeRefPath('src/parser')));
  assert.ok(universe.has(normalizeRefPath('src/parser/rust/mod.rs')));
});

test('the universe never keeps a dot-slash spelling', () => {
  const universe = buildPathUniverse(['./a/b.rs']);
  assert.deepEqual([...universe].sort(), ['a', 'a/b.rs']);
});

test('a ghost node with no file path contributes nothing', () => {
  assert.equal(buildPathUniverse(['', undefined as unknown as string]).size, 0);
});

test('a root-level file contributes itself and no phantom parent', () => {
  assert.deepEqual([...buildPathUniverse(['README.md'])], ['README.md']);
});

test('two files under one folder agree on the shared ancestors', () => {
  const universe = buildPathUniverse(['src/a/x.rs', 'src/a/y.rs']);
  assert.ok(universe.has('src/a'));
  assert.equal([...universe].filter((p) => p === 'src/a').length, 1);
});

// ---------------------------------------------------------------------
// pathClaims
// ---------------------------------------------------------------------

test('a folder claims the files beneath it', () => {
  assert.ok(pathClaims('src/parser', 'src/parser/rust/mod.rs'));
});

test('a file claims itself', () => {
  assert.ok(pathClaims('src/parser/mod.rs', 'src/parser/mod.rs'));
});

test('a folder does not claim a sibling that merely shares its prefix', () => {
  // The reason this is not a bare startsWith.
  assert.ok(!pathClaims('src/parser', 'src/parser_old/mod.rs'));
});

test('a claim survives either side being spelled with dot-slash', () => {
  assert.ok(pathClaims('./src/parser/', './src/parser/rust/mod.rs'));
  assert.ok(pathClaims('src/parser', './src/parser/rust/mod.rs'));
  assert.ok(pathClaims('./src/parser', 'src/parser/rust/mod.rs'));
});

test('an empty claim claims nothing', () => {
  assert.ok(!pathClaims('', 'src/a.rs'));
  assert.ok(!pathClaims('src', ''));
});

test('a file does not claim the folder above it', () => {
  assert.ok(!pathClaims('src/parser/mod.rs', 'src/parser'));
});

// ---------------------------------------------------------------------
// bySpecificity
// ---------------------------------------------------------------------

test('the precise claim is offered before the folder-wide one', () => {
  const ordered = bySpecificity(['src', 'src/parser/rust/mod.rs', 'src/parser']);
  assert.deepEqual(ordered, ['src/parser/rust/mod.rs', 'src/parser', 'src']);
});

test('paths of equal length order stably rather than by insertion', () => {
  assert.deepEqual(bySpecificity(['b/y', 'a/x']), bySpecificity(['a/x', 'b/y']));
});
