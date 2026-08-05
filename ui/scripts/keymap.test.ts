/**
 * Unit tests for the shortcut layer's table (UI-075).
 *
 * The invariants here are the ones a keymap dies of: two bindings on one key
 * in one scope, a pane key that shadows a navigation digit, a binding whose
 * command nothing runs. None of them are visible in a screenshot and all of
 * them are cheap to assert, which is exactly the trade the other suites in
 * this directory are built on — `keymap.ts` imports no store and touches no
 * DOM, so it runs under the bare Node runner.
 *
 * What is deliberately *not* here: whether `graph.fit` fits the view. That
 * lives in `keymapActions.ts`, which reaches for stores, and asserting it
 * would mean mocking the module graph to re-test d3.
 *
 *   npm run test:keymap
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  BINDINGS, PANES, bindingsForScope, matchBinding, parseKeys, displayKeys,
  isTypingTarget, needsModifier, type Scope, type PaneId,
} from '../src/viewmodels/keymap.ts';

const SCOPES: Scope[] = ['global', ...PANES.map((p) => p.id)];

// --- the table itself ---

test('no key is bound twice in one scope', () => {
  for (const scope of SCOPES) {
    const seen = new Map<string, string>();
    for (const b of bindingsForScope(scope)) {
      const clash = seen.get(b.keys);
      assert.equal(clash, undefined,
        `${scope}: "${b.keys}" is both ${clash} and ${b.command}`);
      seen.set(b.keys, b.command);
    }
  }
});

test('no pane key shadows a global one', () => {
  // The resolution order (pane, then global) would silently pick the pane's,
  // and the bar would print a key whose meaning depends on focus in a way it
  // does not show. Cheaper to forbid than to explain.
  const globalKeys = new Set(bindingsForScope('global').map((b) => b.keys));
  for (const pane of PANES) {
    for (const b of bindingsForScope(pane.id)) {
      assert.ok(!globalKeys.has(b.keys),
        `${pane.id}: "${b.keys}" (${b.command}) shadows the global binding`);
    }
  }
});

test('every pane is reachable by its own digit, and nothing else claims digits', () => {
  for (const pane of PANES) {
    const nav = BINDINGS.find((b) => b.keys === pane.digit && b.scope === 'global');
    assert.ok(nav, `no global binding on ${pane.digit}`);
    assert.equal(nav!.command, `pane.focus.${pane.id}`);
  }
  const digits = new Set(PANES.map((p) => p.digit));
  for (const b of BINDINGS) {
    if (b.scope === 'global') continue;
    assert.ok(!digits.has(b.keys), `${b.scope} claims navigation digit ${b.keys}`);
  }
});

test('every binding carries a label short enough for a bar chip', () => {
  for (const b of BINDINGS) {
    assert.ok(b.label.length > 0, `${b.command} has no label`);
    assert.ok(b.label.length <= 18, `${b.command}'s label is ${b.label.length} chars`);
  }
});

test('every pane has at least one binding of its own', () => {
  // A pane the reader can focus but cannot then do anything in is a dead end
  // that the bar advertises with an empty row.
  for (const pane of PANES) {
    assert.ok(bindingsForScope(pane.id).length > 0, `${pane.id} has no keys`);
  }
});

// --- parsing ---

test('parseKeys reads modifiers, and "+" is a key rather than a separator', () => {
  assert.deepEqual(parseKeys('p'), { key: 'p', mod: false, shift: false, alt: false });
  assert.deepEqual(parseKeys('mod+k'), { key: 'k', mod: true, shift: false, alt: false });
  assert.deepEqual(parseKeys('+'), { key: '+', mod: false, shift: false, alt: false });
  assert.deepEqual(parseKeys('shift+p'), { key: 'p', mod: false, shift: true, alt: false });
});

test('displayKeys prints the platform modifier', () => {
  assert.equal(displayKeys('mod+k', true), '⌘K');
  assert.equal(displayKeys('mod+k', false), 'Ctrl-K');
  assert.equal(displayKeys('Escape', true), 'Escape');
  assert.equal(displayKeys('p', false), 'P');
});

// --- matching ---

const ev = (key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) =>
  ({ key, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...mods });

test('a pane key only fires in its own pane', () => {
  // `q` is the sidebar's Quality tab and nothing at all on the canvas.
  assert.equal(matchBinding(ev('q'), 'sidebar', false)?.command, 'sidebar.tab.quality');
  assert.equal(matchBinding(ev('q'), 'graph', false), null);
});

test('the same key means different things in different panes', () => {
  assert.equal(matchBinding(ev('f'), 'graph', false)?.command, 'graph.fit');
  assert.equal(matchBinding(ev('f'), 'sidebar', false)?.command, 'sidebar.tab.filters');
  assert.equal(matchBinding(ev('f'), 'view', false)?.command, 'view.level.file');
});

test('global keys fire from every pane', () => {
  for (const pane of PANES) {
    assert.equal(matchBinding(ev('l'), pane.id, false)?.command, 'hover.lock');
    assert.equal(matchBinding(ev('?'), pane.id, false)?.command, 'help.toggle');
  }
});

test('a bare letter is ignored while typing, a modifier binding is not', () => {
  assert.equal(matchBinding(ev('p'), 'graph', true), null);
  assert.equal(matchBinding(ev('k', { metaKey: true }), 'graph', true)?.command, 'search.focus');
  assert.equal(matchBinding(ev('k', { ctrlKey: true }), 'graph', true)?.command, 'search.focus');
});

test('an unmodified key does not match a modifier binding', () => {
  // Typing `k` in the search box must not re-focus the search box.
  assert.equal(matchBinding(ev('k'), 'graph', false), null);
});

test('case does not matter, so caps lock is not a broken keyboard', () => {
  assert.equal(matchBinding(ev('P'), 'graph', false)?.command, 'graph.pin');
});

test('both zoom-in spellings resolve, and only on the canvas', () => {
  assert.equal(matchBinding(ev('+'), 'graph', false)?.command, 'graph.zoomIn');
  assert.equal(matchBinding(ev('='), 'graph', false)?.command, 'graph.zoomIn');
  assert.equal(matchBinding(ev('='), 'details', false), null);
});

test('needsModifier separates the keys that survive a focused text field', () => {
  const modBindings = BINDINGS.filter(needsModifier).map((b) => b.keys);
  assert.deepEqual([...new Set(modBindings)], ['mod+k']);
});

// --- typing detection ---

test('isTypingTarget catches text fields and nothing else', () => {
  assert.equal(isTypingTarget({ tagName: 'INPUT' }), true);
  assert.equal(isTypingTarget({ tagName: 'textarea' }), true);
  assert.equal(isTypingTarget({ isContentEditable: true, tagName: 'DIV' }), true);
  assert.equal(isTypingTarget({ tagName: 'BUTTON' }), false);
  assert.equal(isTypingTarget({ tagName: 'SELECT' }), false);
  assert.equal(isTypingTarget(null), false);
});

// --- the pane list ---

test('the digits are contiguous from 0', () => {
  // Contiguity is the invariant that survived the spec pane (ADR 0011); layout
  // order did not. `5` is the Elevator pane, appended rather than inserted at
  // its screen position between the sidebar and the canvas — inserting would
  // have renumbered four panes people already have in their hands, to give a
  // *conditionally present* pane the most memorable digit of the set.
  assert.deepEqual(PANES.map((p) => p.digit), ['0', '1', '2', '3', '4', '5']);
});

test('the always-present panes stay in layout order', () => {
  const ids: PaneId[] = ['sidebar', 'graph', 'view', 'details', 'description'];
  assert.deepEqual(PANES.filter((p) => p.id !== 'spec').map((p) => p.id), ids);
});

test('the optional pane is the one out of position', () => {
  assert.equal(PANES.at(-1)?.id, 'spec');
});
