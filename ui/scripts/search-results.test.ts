/**
 * Unit tests for the two decisions the ranked entity search added.
 *
 * Same zero-dependency setup as the sibling suites — Node's built-in runner
 * plus type stripping. Both modules under test import nothing at runtime,
 * which is why they are separate files rather than functions inside
 * `filterViewModel.ts` and `searchResults.ts`: the runner resolves no
 * extensionless `.ts` imports, so a module that reaches for a store is a
 * module that cannot be tested here.
 *
 * `scoreEntity` takes its per-string scorer as a parameter, so these tests
 * assert the *weighting policy* against a stub and never re-test the fuzzy
 * matcher — `test:fuzzy` already owns that.
 *
 *   npm run test:search
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { scoreEntity, splitPath, FIELD_WEIGHT } from '../src/utils/entityScore.ts';
import { classifyBlock, type FilterSnapshot } from '../src/utils/blockReason.ts';
import type { D3Node } from '../src/types/graph.ts';

const ALL_FIELDS = { inNames: true, inFiles: true, inFolders: true };
const NO_KINDS = new Set<string>();

function node(over: Partial<D3Node> = {}): D3Node {
  return {
    id: 'n1',
    original_id: 'n1',
    name: 'computeLayout',
    qualified_name: 'displayPlan::computeLayout',
    kind: 'Function',
    kind_raw: 'Function',
    file_path: 'ui/src/viewmodels/displayPlan.ts',
    language: 'typescript',
    line: 10,
    tags: [],
    ...over,
  } as D3Node;
}

/** Scores only exact substring hits, at a fixed value, so any difference in
 *  the result is the weighting and nothing else. */
const exact = (needle: string) => (c: string) => (c.includes(needle) ? 100 : null);
const never = () => false;

// --- scoreEntity: which field wins ---

test('a name hit outranks the same hit in the path', () => {
  const onName = scoreEntity(node(), ALL_FIELDS, NO_KINDS, exact('computeLayout'), never);
  const onPath = scoreEntity(node({ name: 'x', qualified_name: 'x' }), ALL_FIELDS, NO_KINDS,
    exact('displayPlan'), never);
  assert.equal(onName, 100 * FIELD_WEIGHT.name);
  assert.equal(onPath, 100 * FIELD_WEIGHT.filename);
  assert.ok(onName! > onPath!);
});

test('the best field wins rather than the sum', () => {
  // Matches name, qualified name and filename all at once. Summing would
  // give 270; the contract is that it stays the single best field.
  const s = scoreEntity(node({
    name: 'displayPlan', qualified_name: 'displayPlan', file_path: 'a/displayPlan.ts',
  }), ALL_FIELDS, NO_KINDS, exact('displayPlan'), never);
  assert.equal(s, 100 * FIELD_WEIGHT.name);
});

test('a folder-only hit scores below a filename hit', () => {
  const folder = scoreEntity(node({ name: 'x', qualified_name: 'x' }), ALL_FIELDS, NO_KINDS,
    exact('viewmodels'), never);
  assert.equal(folder, 100 * FIELD_WEIGHT.folder);
  assert.ok(folder! < 100 * FIELD_WEIGHT.filename);
});

test('a disabled field cannot contribute', () => {
  const n = node({ name: 'x', qualified_name: 'x' });
  assert.equal(
    scoreEntity(n, { inNames: true, inFiles: false, inFolders: false }, NO_KINDS,
      exact('displayPlan'), never),
    null,
  );
  assert.equal(
    scoreEntity(n, { inNames: true, inFiles: true, inFolders: false }, NO_KINDS,
      exact('displayPlan'), never),
    100 * FIELD_WEIGHT.filename,
  );
});

test('zero is a match, not an absence', () => {
  assert.equal(scoreEntity(node(), ALL_FIELDS, NO_KINDS, () => 0, never), 0);
});

test('a kind restriction rejects before any scoring', () => {
  assert.equal(
    scoreEntity(node(), ALL_FIELDS, new Set(['Class']), exact('computeLayout'), never),
    null,
  );
  assert.equal(
    scoreEntity(node(), ALL_FIELDS, new Set(['Function']), exact('computeLayout'), never),
    100 * FIELD_WEIGHT.name,
  );
});

// --- scoreEntity: negation spans every identifying string ---

test('negation on the path rejects a node whose name matched', () => {
  // The regression this encodes: `graph !test` used to return an entity
  // named `graph` sitting in `foo.test.ts`, because the name satisfied both
  // terms on its own.
  const n = node({ name: 'graph', qualified_name: 'graph', file_path: 'ui/foo.test.ts' });
  const rejectsTest = (c: string) => c.includes('test');
  assert.equal(scoreEntity(n, ALL_FIELDS, NO_KINDS, exact('graph'), rejectsTest), null);
});

test('negation applies even when the path fields are switched off', () => {
  const n = node({ name: 'graph', qualified_name: 'graph', file_path: 'ui/foo.test.ts' });
  assert.equal(
    scoreEntity(n, { inNames: true, inFiles: false, inFolders: false }, NO_KINDS,
      exact('graph'), (c) => c.includes('test')),
    null,
  );
});

test('splitPath separates folder from filename, and copes with neither', () => {
  assert.deepEqual(splitPath('a/b/c.ts'), { folder: 'a/b', filename: 'c.ts' });
  assert.deepEqual(splitPath('c.ts'), { folder: '', filename: 'c.ts' });
});

// --- classifyBlock: which control is holding a node back ---

function snap(over: Partial<FilterSnapshot> = {}): FilterSnapshot {
  return {
    kinds: new Set(['Function']),
    langs: new Set(['typescript']),
    files: new Set(['ui/src/viewmodels/displayPlan.ts']),
    showGhosts: true,
    showBuiltinGhosts: true,
    showTemplateVars: true,
    structureOnly: false,
    exemptBody: null,
    ...over,
  };
}

test('a node passing every filter is not blocked', () => {
  assert.equal(classifyBlock(node(), snap()), null);
});

test('each hard filter is named, and carries the value that reverses it', () => {
  assert.deepEqual(
    { ...classifyBlock(node(), snap({ kinds: new Set(['Class']) })) },
    { kind: 'kind', value: 'Function', label: 'hidden: Function filter', reversible: true },
  );
  assert.equal(classifyBlock(node(), snap({ langs: new Set(['rust']) }))?.value, 'typescript');
  assert.equal(
    classifyBlock(node(), snap({ files: new Set() }))?.value,
    'ui/src/viewmodels/displayPlan.ts',
  );
});

test('ghosts answer to their own toggles', () => {
  const ghost = node({ tags: ['ghost'] });
  assert.equal(classifyBlock(ghost, snap({ showGhosts: false }))?.kind, 'ghost');
  // A plain ghost is unaffected by the builtin sub-toggle.
  assert.equal(classifyBlock(ghost, snap({ showBuiltinGhosts: false })), null);
  const builtin = node({ tags: ['ghost', 'ghost_stdlib'] });
  assert.equal(classifyBlock(builtin, snap({ showBuiltinGhosts: false }))?.kind, 'builtin-ghost');
});

test('template vars are claimed by their toggle', () => {
  assert.equal(
    classifyBlock(node({ tags: ['template_var'] }), snap({ showTemplateVars: false }))?.kind,
    'template-var',
  );
});

test('the first failing filter is the one reported', () => {
  // Failing kind *and* language: the badge must name kind, because that is
  // the one `nodePassesFilters` reaches first. Reporting the other would
  // send the user to a control that is not the one deciding.
  const r = classifyBlock(node(), snap({ kinds: new Set(['Class']), langs: new Set(['rust']) }));
  assert.equal(r?.kind, 'kind');
});

test('an internal is blamed on the grain control, never on its kind', () => {
  // The badge has to name the control that is actually deciding. Every kind
  // box is ticked here — a Branch is held back by "Structure only", and
  // sending the reader to the Branch checkbox would send them to a control
  // already showing everything it can. UI-113.
  const branch = node({ kind_raw: 'Branch', kind: 'branch', body_of: 'parse' });
  const on = snap({ structureOnly: true, kinds: new Set(['Function', 'Branch']) });
  assert.deepEqual(
    { ...classifyBlock(branch, on) },
    { kind: 'internals', value: 'parse', label: 'inside a function body', reversible: true },
  );
  // Off, and it is drawn like anything else.
  assert.equal(classifyBlock(branch, snap({ kinds: new Set(['Function', 'Branch']) })), null);
  // Selected owner: its body is on screen, so nothing is blocking the row.
  assert.equal(classifyBlock(branch, snap({
    structureOnly: true, exemptBody: 'parse', kinds: new Set(['Function', 'Branch']),
  })), null);
});

test('a ghost with an empty file path is not blamed on the file filter', () => {
  // Ghosts carry no `file_path`; an empty path must not be read as "not in
  // the visible set", which would badge every ghost with the wrong control.
  const ghost = node({ tags: ['ghost'], file_path: '' });
  assert.equal(classifyBlock(ghost, snap({ files: new Set() })), null);
});
