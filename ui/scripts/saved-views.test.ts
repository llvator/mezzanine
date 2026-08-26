/**
 * Unit tests for what a saved view holds and what surviving a round trip
 * means (UI-082).
 *
 * Two failure modes are worth guarding, and both are silent:
 *
 *   1. A view read back from `.mezz/views.json` — a file a human can edit —
 *      that throws or half-restores. `normalizeState` is total for exactly
 *      this reason, so the tests feed it garbage on purpose.
 *   2. A view that restores to a *different* picture than it captured. The
 *      list marks the active view by comparing state, so an order-sensitive
 *      comparison would show "restored" and then immediately "modified"
 *      with nothing having changed.
 *
 *   npm run test:saved-views
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  droppedSummary,
  emptyState,
  normalizeState,
  normalizeViews,
  nothingDropped,
  pruneState,
  sameState,
  stateSummary,
  uniqueName,
  type ViewState,
} from '../src/viewmodels/savedViews.ts';

function state(over: Partial<ViewState> = {}): ViewState {
  return { ...emptyState(), ...over };
}

// --- normalizeState: total over anything the file can hold ---

test('a state missing every field falls back to defaults rather than throwing', () => {
  const s = normalizeState({});
  assert.deepEqual(s, emptyState());
});

test('a state that is not an object at all is still a state', () => {
  assert.deepEqual(normalizeState(null), emptyState());
  assert.deepEqual(normalizeState('whole repo'), emptyState());
  assert.deepEqual(normalizeState([1, 2, 3]), emptyState());
});

test('mistyped fields are dropped, correct ones survive', () => {
  const s = normalizeState({
    scope: [{ pattern: 'src', negate: false }, { negate: true }, 'ui'],
    level: 'galaxy',
    entityTypes: ['Function', 7, null],
    autoLevel: 'yes',
    hiddenFiles: ['a.rs'],
  });
  assert.deepEqual(s.scope, [{ pattern: 'src', negate: false }]);
  assert.equal(s.level, 'entity', 'an unknown level falls back rather than reaching the store');
  assert.deepEqual(s.entityTypes, ['Function']);
  assert.equal(s.autoLevel, true, 'a non-boolean leaves the default standing');
  assert.deepEqual(s.hiddenFiles, ['a.rs']);
});

test('level overrides keep their tri-states and drop invalid ones', () => {
  const s = normalizeState({
    levelOverrides: {
      1: { enabled: true, entityTypes: { Function: 'on', Class: 'sometimes' }, outgoing: 'off' },
      nope: { enabled: true },
    },
  });
  assert.deepEqual(Object.keys(s.levelOverrides), ['1']);
  assert.deepEqual(s.levelOverrides[1].entityTypes, { Function: 'on' });
  assert.equal(s.levelOverrides[1].outgoing, 'off');
  assert.equal(s.levelOverrides[1].incoming, 'general');
  assert.equal(s.levelOverrides[1].peerEdges, true, 'absent peerEdges means on, as in the store');
});

// --- normalizeViews: the list ---

test('entries with no id or no name are dropped, not repaired', () => {
  const views = normalizeViews([
    { id: 'a', name: 'Parsers', saved_at: '2026-08-05T10:00:00Z', state: {} },
    { id: '', name: 'Nameless id', state: {} },
    { id: 'b', name: '   ', state: {} },
    'not a view',
  ]);
  assert.equal(views.length, 1);
  assert.equal(views[0].name, 'Parsers');
});

test('a list that is not a list is an empty list', () => {
  assert.deepEqual(normalizeViews({ views: [] }), []);
});

// --- pruneState: the code moved under the view ---

const PRESENT = {
  files: new Set(['a.rs', 'b.rs']),
  entityIds: new Set(['e1', 'e2']),
  specIds: new Set(['f.parser']),
};

test('ids the graph no longer has are dropped and counted', () => {
  const { state: pruned, dropped } = pruneState(
    state({
      hiddenFiles: ['a.rs', 'gone.rs'],
      searchIds: ['e1', 'e9'],
      spec: ['f.parser', 'f.deleted'],
    }),
    PRESENT,
  );
  assert.deepEqual(pruned.hiddenFiles, ['a.rs']);
  assert.deepEqual(pruned.searchIds, ['e1']);
  assert.deepEqual(pruned.spec, ['f.parser']);
  assert.deepEqual(dropped, { files: 1, search: 1, spec: 1 });
  assert.equal(nothingDropped(dropped), false);
});

test('scope rules are never pruned — a path may not exist on this branch yet', () => {
  const { state: pruned, dropped } = pruneState(
    state({ scope: [{ pattern: 'feature/not-yet', negate: false }] }),
    PRESENT,
  );
  assert.deepEqual(pruned.scope, [{ pattern: 'feature/not-yet', negate: false }]);
  assert.ok(nothingDropped(dropped));
});

test('with no dataset loaded, nothing is pruned', () => {
  const empty = { files: new Set<string>(), entityIds: new Set<string>(), specIds: new Set<string>() };
  const { state: pruned, dropped } = pruneState(state({ hiddenFiles: ['a.rs'], spec: ['x'] }), empty);
  assert.deepEqual(pruned.hiddenFiles, ['a.rs']);
  assert.ok(nothingDropped(dropped), 'pruning against nothing would report everything as gone');
});

test('the dropped summary counts and pluralises, and is empty when nothing went', () => {
  assert.equal(droppedSummary({ files: 0, search: 0, spec: 0 }), '');
  assert.match(droppedSummary({ files: 2, search: 0, spec: 1 }), /2 hidden files/);
  assert.match(droppedSummary({ files: 0, search: 1, spec: 0 }), /1 search hit\b/);
});

// --- sameState: the "you are here" marker ---

test('order within a filter does not make two identical views differ', () => {
  const a = state({ entityTypes: ['Function', 'Class'], hiddenLanguages: ['Rust', 'Python'] });
  const b = state({ entityTypes: ['Class', 'Function'], hiddenLanguages: ['Python', 'Rust'] });
  assert.ok(sameState(a, b), 'filters are sets — insertion order is not a difference');
});

test('key order in the level overrides does not make two identical views differ', () => {
  const a = normalizeState({ levelOverrides: { 1: { enabled: true, entityTypes: { A: 'on', B: 'off' } } } });
  const b = normalizeState({ levelOverrides: { 1: { entityTypes: { B: 'off', A: 'on' }, enabled: true } } });
  assert.ok(sameState(a, b));
});

test('scope rule order IS a difference — last match wins (ADR 0009)', () => {
  const a = state({ scope: [{ pattern: '', negate: false }, { pattern: 'ui', negate: true }] });
  const b = state({ scope: [{ pattern: 'ui', negate: true }, { pattern: '', negate: false }] });
  assert.equal(sameState(a, b), false, 'these two rule lists select different code');
});

test('a real difference is seen', () => {
  assert.equal(sameState(state({ level: 'file' }), state({ level: 'folder' })), false);
  assert.equal(sameState(state({ showGhosts: true }), state({ showGhosts: false })), false);
});

// --- the pre-rename level spelling ---

test('a view saved at the old `module` level reopens at folder level', () => {
  // `.mezz/views.json` outlives the build that wrote it. `normalizeState` is
  // total, so an unrecognised level does not fail — it falls back to
  // `entity`, which would silently reopen a folder-level view as thousands
  // of circles. That is the failure this migration exists to prevent.
  assert.equal(normalizeState({ level: 'module' }).level, 'folder');
});

test('an unknown level is still the entity fallback, not a guess', () => {
  assert.equal(normalizeState({ level: 'package' }).level, emptyState().level);
  assert.equal(normalizeState({ level: 7 }).level, emptyState().level);
});

// --- naming ---

test('a name already taken gets a counter, case-insensitively', () => {
  assert.equal(uniqueName('Parsers', []), 'Parsers');
  assert.equal(uniqueName('Parsers', ['Parsers']), 'Parsers 2');
  assert.equal(uniqueName('Parsers', ['Parsers', 'Parsers 2']), 'Parsers 3');
  assert.equal(uniqueName('parsers', ['Parsers']), 'parsers 2');
  assert.equal(uniqueName('   ', []), 'View');
});

// --- the row subtitle ---

test('the summary names the scope and the level', () => {
  assert.match(stateSummary(state({ scope: [{ pattern: '', negate: false }] })), /whole repo/);
  assert.match(stateSummary(state({ scope: [{ pattern: 'src/parser', negate: false }] })), /src\/parser/);
  assert.match(
    stateSummary(state({ scope: [{ pattern: 'a', negate: false }, { pattern: 'b', negate: false }] })),
    /2 paths/,
  );
  assert.match(stateSummary(state({ autoLevel: false, level: 'folder' })), /folder/);
  assert.match(stateSummary(state({ spec: ['a', 'b'] })), /spec ×2/);
});

// --- UI-104: a mixed picture is a picture, so a view has to hold it ---

test('the ring focus survives a round trip', () => {
  const s = normalizeState(state({ ringFocus: 'ui/src/stores', ringReach: 3 }));
  assert.equal(s.ringFocus, 'ui/src/stores');
  assert.equal(s.ringReach, 3);
});

test('a view with rings is not the same picture as one without', () => {
  // The level alone names only the OUTER grain once a focus is set, so two
  // states agreeing on `level` can still be two different canvases. If this
  // compared equal the list would mark a mixed view active while showing a
  // uniform one.
  const uniform = state({ level: 'folder' });
  const mixed = state({ level: 'folder', ringFocus: 'ui/src' });
  assert.equal(sameState(uniform, mixed), false);
  assert.equal(sameState(mixed, state({ level: 'folder', ringFocus: 'ui/src', ringReach: 2 })), false);
});

test('an empty ring focus is no focus, not a focus on the repo root', () => {
  // `''` as a path seeds every entity in the repo, so a stray empty string in
  // a hand-edited file would silently draw the whole graph at entity grain.
  assert.equal(normalizeState({ ringFocus: '' }).ringFocus, null);
  assert.equal(normalizeState({ ringFocus: 42 }).ringFocus, null);
});

test('a hand-written reach is clamped rather than believed', () => {
  assert.equal(normalizeState({ ringReach: 900 }).ringReach, 4);
  assert.equal(normalizeState({ ringReach: -3 }).ringReach, 0);
  assert.equal(normalizeState({ ringReach: 'two' }).ringReach, emptyState().ringReach);
});

test('the row subtitle says a view is focused', () => {
  const summary = stateSummary(state({ level: 'folder', ringFocus: 'ui/src', ringReach: 2 }));
  assert.match(summary, /focus ui\/src \+2/);
});
