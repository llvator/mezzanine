/**
 * Unit tests for what one window does with what another one says (UI-095).
 *
 * The failure modes are all cheap to describe and expensive to find by hand:
 * two windows echoing each other forever, a click costing a graph refetch on
 * the far screen, and a tab left open across a deploy shouting a payload this
 * version cannot read. None of them need a second browser window to test,
 * which is why the decision lives in a pure module.
 *
 *   npm run test:mirror
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptyState, type ViewState } from '../src/viewmodels/savedViews.ts';
import {
  channelName,
  emptyAttention,
  hoverToShow,
  isFresh,
  normalizeMessage,
  whatToApply,
  type Attention,
  type MirrorMessage,
} from '../src/viewmodels/mirror.ts';

function view(path: string, over: Partial<ViewState> = {}): ViewState {
  return { ...emptyState(), scope: [{ pattern: path, negate: false }], ...over };
}

function msg(over: Partial<MirrorMessage> = {}, at: Partial<Attention> = {}): MirrorMessage {
  return {
    origin: 'them',
    seq: 1,
    state: view('src/'),
    ...over,
    attention: { ...emptyAttention(), ...at, ...(over.attention ?? {}) },
  };
}

const MINE = msg({ origin: 'me' });

// ─── what to do with an arrival ──────────────────────────────────────────────

test('a window ignores its own echo', () => {
  const echo = msg({ origin: 'me', state: view('other/') }, { selected: 'a' });
  assert.equal(whatToApply(echo, MINE), 'none');
});

test('a message describing the picture already on screen asks for nothing', () => {
  assert.equal(whatToApply(msg(), MINE), 'none');
});

test('a moved selection on the same reading never costs a republish', () => {
  const clicked = msg({}, { selected: 'src/a.rs:10:foo' });
  assert.equal(whatToApply(clicked, MINE), 'attention');
});

test('deselecting on the far screen is an attention change too', () => {
  const mineWithSelection = msg({ origin: 'me' }, { selected: 'src/a.rs:10:foo' });
  assert.equal(whatToApply(msg(), mineWithSelection), 'attention');
});

// The pane that reads hover is most of why a second monitor is worth having,
// so a pointer crossing a node on the far screen has to be a change here —
// and has to be the cheap kind, since it happens far more than a click does.
test('a pointer crossing a node on the far screen is a change, and a cheap one', () => {
  assert.equal(whatToApply(msg({}, { hovered: 'src/a.rs:10:foo' }), MINE), 'attention');
});

test('the pointer leaving a node on the far screen is a change too', () => {
  const mineWithHover = msg({ origin: 'me' }, { hovered: 'src/a.rs:10:foo' });
  assert.equal(whatToApply(msg(), mineWithHover), 'attention');
});

test('freezing a preview on one screen is a change on the other', () => {
  assert.equal(whatToApply(msg({}, { hoverLocked: true }), MINE), 'attention');
});

test('switching to the tree is a change, and does not cost a refetch', () => {
  assert.equal(whatToApply(msg({}, { mode: 'tree' }), MINE), 'attention');
});

// Drilling into a Category is navigation within the spec pane, not a different
// reading — filing it with the scope would make opening a level on one screen
// refetch the graph on the other.
test('opening a level in the spec pane is a change, and does not cost a refetch', () => {
  assert.equal(whatToApply(msg({}, { specPath: ['cat'] }), MINE), 'attention');
});

test('climbing back out of the spec pane is a change too', () => {
  const drilled = msg({ origin: 'me' }, { specPath: ['cat', 'feat'] });
  assert.equal(whatToApply(msg({}, { specPath: ['cat'] }), drilled), 'attention');
});

// Unlike the filters, a drill path is a descent: the same two entities in the
// other order is a different place to be standing.
test('a drill path is compared as a sequence, not as a set', () => {
  const mine = msg({ origin: 'me' }, { specPath: ['a', 'b'] });
  assert.equal(whatToApply(msg({}, { specPath: ['b', 'a'] }), mine), 'attention');
  assert.equal(whatToApply(msg({}, { specPath: ['a', 'b'] }), mine), 'none');
});

test('a different reading is applied in full', () => {
  assert.equal(whatToApply(msg({ state: view('tests/') }), MINE), 'all');
});

test('the reading wins over attention when both moved', () => {
  const both = msg({ state: view('tests/') }, { selected: 'x', hovered: 'y' });
  assert.equal(whatToApply(both, MINE), 'all');
});

test('two readings that differ only in insertion order are one reading', () => {
  const a = msg({ origin: 'me', state: view('src/', { entityTypes: ['Function', 'Struct'] }) });
  const b = msg({ state: view('src/', { entityTypes: ['Struct', 'Function'] }) });
  assert.equal(whatToApply(b, a), 'none');
});

// ─── whose pointer wins ──────────────────────────────────────────────────────

test('a window with no pointer of its own shows the peer\'s', () => {
  assert.equal(hoverToShow(null, 'src/a.rs:10:foo', null), 'src/a.rs:10:foo');
});

test('the local pointer wins while it is on a node', () => {
  assert.equal(hoverToShow('mine', 'theirs', null), 'mine');
});

// The two readers would otherwise overwrite each other every few pixels, and
// the pane would settle on nothing at all.
test('a local pointer is not dragged off by a peer that keeps talking', () => {
  assert.equal(hoverToShow('mine', 'theirs', null), 'mine');
  assert.equal(hoverToShow('mine', 'other', null), 'mine');
});

test('a hover taken from the peer is not mistaken for a local one', () => {
  // `theirs` is on screen only because we adopted it, so the peer may move it.
  assert.equal(hoverToShow('theirs', 'moved', 'theirs'), 'moved');
});

test('the peer is picked back up once the local pointer leaves', () => {
  assert.equal(hoverToShow(null, 'theirs', null), 'theirs');
});

test('a peer with no pointer clears the hover it lent us', () => {
  assert.equal(hoverToShow('theirs', null, 'theirs'), null);
});

test('a peer with no pointer does not clear a local one', () => {
  assert.equal(hoverToShow('mine', null, 'theirs'), 'mine');
});

// ─── a local click outranks a borrowed hover ─────────────────────────────────
//
// The setup this feature is for puts the canvas on one screen and the panes on
// the other, so the pane window has no pointer on the graph and is always
// showing the peer's hover. Description prefers hover over selection, so
// without this a click in its own Spec pane moved Details and left Description
// narrating the peer's node.

test('selecting locally hands the panes back from a borrowed hover', () => {
  assert.equal(hoverToShow('theirs', 'theirs', 'theirs', 'theirs'), null);
});

test('the refusal is on one entity, not on mirroring', () => {
  // The peer's pointer moves on: a new gesture, adopted like any other.
  assert.equal(hoverToShow(null, 'moved', null, 'theirs'), 'moved');
});

test('a local pointer still wins outright, refusal or not', () => {
  assert.equal(hoverToShow('mine', 'theirs', null, 'theirs'), 'mine');
});

test('refusing nothing is the old two-window rule unchanged', () => {
  assert.equal(hoverToShow(null, 'theirs', null, null), 'theirs');
  assert.equal(hoverToShow(null, null, null, null), null);
});

// ─── ordering ────────────────────────────────────────────────────────────────

test('the first message from a window is always fresh', () => {
  assert.equal(isFresh(msg({ seq: 7 }), {}), true);
});

test('a payload behind one already accepted is dropped', () => {
  assert.equal(isFresh(msg({ seq: 3 }), { them: 5 }), false);
  assert.equal(isFresh(msg({ seq: 5 }), { them: 5 }), false, 'a repeat is not newer');
  assert.equal(isFresh(msg({ seq: 6 }), { them: 5 }), true);
});

test('windows are ordered independently of one another', () => {
  assert.equal(isFresh(msg({ origin: 'c', seq: 1 }), { a: 99, b: 99 }), true);
});

// ─── payloads off the wire ───────────────────────────────────────────────────

test('garbage is ignored rather than thrown on', () => {
  for (const junk of [null, undefined, 42, 'hello', [], {}, { origin: '' }]) {
    assert.equal(normalizeMessage(junk), null, `accepted ${JSON.stringify(junk)}`);
  }
});

test('a message missing a usable sequence number is not a message', () => {
  assert.equal(normalizeMessage({ origin: 'a', state: {} }), null);
  assert.equal(normalizeMessage({ origin: 'a', seq: NaN, state: {} }), null);
  assert.equal(normalizeMessage({ origin: 'a', seq: '3', state: {} }), null);
});

test('an unreadable state normalizes to an empty one rather than failing', () => {
  const out = normalizeMessage({ origin: 'a', seq: 1, state: { scope: 'not a list' } });
  assert.ok(out);
  assert.deepEqual(out.state, emptyState());
});

test('a selection that is not an id reads as nothing selected', () => {
  const at = { selected: 17, hovered: [] };
  assert.equal(normalizeMessage({ origin: 'a', seq: 1, state: {}, attention: at })?.attention.selected, null);
  assert.equal(normalizeMessage({ origin: 'a', seq: 1, state: {}, attention: at })?.attention.hovered, null);
});

// A tab left open across the deploy that added `attention` is the realistic
// version-skew case. The reading still syncs; the pointer does not.
test('a message with no attention at all reads as standing nowhere', () => {
  assert.deepEqual(
    normalizeMessage({ origin: 'a', seq: 1, state: {} })?.attention,
    emptyAttention(),
  );
  assert.deepEqual(
    normalizeMessage({ origin: 'a', seq: 1, state: {}, attention: 'nonsense' })?.attention,
    emptyAttention(),
  );
});

test('a drill path that is not a list of ids reads as nothing opened', () => {
  const at = (specPath: unknown) => normalizeMessage(
    { origin: 'a', seq: 1, state: {}, attention: { specPath } },
  )?.attention.specPath;
  assert.deepEqual(at('cat'), []);
  assert.deepEqual(at({ 0: 'cat' }), []);
  // A list that is partly ids keeps them: the readable steps still describe a
  // real descent, and refusing the whole path would close a pane over one bad
  // entry.
  assert.deepEqual(at(['cat', 7, null, 'feat']), ['cat', 'feat']);
});

test('a view mode this version does not know falls back to the graph', () => {
  const at = { mode: 'hyperbolic' };
  assert.equal(normalizeMessage({ origin: 'a', seq: 1, state: {}, attention: at })?.attention.mode, 'graph');
});

test('a well-formed message survives the round trip', () => {
  const sent = msg(
    { origin: 'w1', seq: 4, state: view('src/') },
    {
      selected: 'src/a.rs:1:f',
      hovered: 'src/b.rs:2:g',
      hoverLocked: true,
      mode: 'tree',
      specPath: ['spec/app.elv:1:Parsing', 'spec/app.elv:9:Rust'],
    },
  );
  const got = normalizeMessage(JSON.parse(JSON.stringify(sent)));
  assert.deepEqual(got, sent);
});

// ─── which windows talk to each other ────────────────────────────────────────

test('two windows on the same repo share a channel, two on different repos do not', () => {
  assert.equal(channelName('alpha'), channelName('alpha'));
  assert.notEqual(channelName('alpha'), channelName('beta'));
});

test('the single-repo case has a name of its own', () => {
  assert.equal(channelName(''), 'nao-mirror:');
});
