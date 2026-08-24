/**
 * Unit tests for the declaration / internals split (UI-113).
 *
 * Three failures matter, in descending order of how quietly they happen.
 *
 * A **method classified as an internal** empties the canvas of the thing the
 * reader opened it for. It is the failure the naive version of this feature
 * has — `parent_id === null` — and it is invisible in a language where free
 * functions dominate and catastrophic in Rust, Java or Kotlin, where almost
 * every callable is a member of something.
 *
 * A **call lost with its branch** is worse than noise, because the graph goes
 * on looking complete: a function whose calls all sit inside a `match` reads
 * as calling nothing at all. That is what `liftBodies` re-routing is for, and
 * most of these tests are about it.
 *
 * A **twin drawn beside the edge it stands in for** is the mirror: two lines
 * where the analyzer found one relationship, doubling the apparent fan-out of
 * every branching function.
 *
 *   npm run test:bodies
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  BODY_KINDS,
  bodyHidden,
  bodyOwners,
  liftBodies,
  linkDrawable,
  linkTraversable,
} from '../src/viewmodels/bodyScope.ts';
import type { D3Link, D3Node, GraphData } from '../src/types/graph.ts';

/** A node with just the fields this module reads. `id` doubles as
 *  `original_id`, which is what the real transform does for anything without
 *  characters to sanitize. */
function node(id: string, kind_raw: string, parent?: string, name?: string): D3Node {
  return {
    id,
    original_id: id,
    name: name ?? id,
    qualified_name: id,
    kind: kind_raw.toLowerCase(),
    kind_raw,
    file_path: 'src/a.rs',
    line: 1,
    end_line: 2,
    visibility: 'Public',
    parent_id: parent ?? null,
    parameters: [],
    return_type: null,
    extends: [],
    implements: [],
    tags: [],
    source_code: null,
    fields: [],
    impl_blocks: [],
    language: 'rust',
  };
}

function link(source: string, target: string, kind_raw = 'Calls', order: number | null = null): D3Link {
  return { source, target, kind_raw, kind: kind_raw.toLowerCase(), incoming_kind: 'x', order };
}

const graph = (nodes: D3Node[], links: D3Link[] = []): GraphData => ({ nodes, links });

const src = (l: D3Link) => (typeof l.source === 'object' ? l.source.id : l.source);
const tgt = (l: D3Link) => (typeof l.target === 'object' ? l.target.id : l.target);
const twins = (g: GraphData) => g.links.filter((l) => l.lifted_from);

// --- what counts as an internal ---

test('a member of a type is a declaration, however deep the ancestry', () => {
  // The whole reason this is an ancestry walk and not `parent_id === null`.
  // Every node here has a parent and not one of them is an internal.
  const nodes = [
    node('mod', 'Module'),
    node('Repo', 'Struct', 'mod'),
    node('Repo::load', 'Method', 'Repo'),
    node('Repo::path', 'Variable', 'Repo'),
    node('Reader', 'Interface', 'mod'),
    node('Reader::size', 'Property', 'Reader'),
    node('helper', 'Function', 'mod'),
  ];
  assert.deepEqual([...bodyOwners(nodes).keys()], []);
});

test('what a callable encloses is that callable’s internals', () => {
  const nodes = [
    node('Repo', 'Struct'),
    node('Repo::load', 'Method', 'Repo'),
    node('path', 'Parameter', 'Repo::load'),
  ];
  assert.equal(bodyOwners(nodes).get('path'), 'Repo::load');
  assert.equal(bodyOwners(nodes).get('Repo::load'), undefined);
});

test('the owner is the outermost enclosure, not the immediate parent', () => {
  // This is what makes the exemption in `displayPlan` a single equality:
  // selecting `parse` has to reopen everything in its body at once, and a
  // branch five arms deep must not answer "my owner is the branch above me".
  const nodes = [
    node('parse', 'Function'),
    node('loop1', 'Loop', 'parse'),
    node('br1', 'Branch', 'loop1'),
    node('br2', 'Branch', 'br1'),
    node('inner', 'Parameter', 'br2'),
  ];
  const owners = bodyOwners(nodes);
  for (const id of ['loop1', 'br1', 'br2', 'inner']) {
    assert.equal(owners.get(id), 'parse', `${id} belongs to parse`);
  }
  assert.equal(owners.get('parse'), undefined);
});

test('an ancestor outside the loaded graph is a scope edge, not a body', () => {
  // Hiding an entity because of an ancestor nobody can see is a claim the
  // reader has no way to check. A narrowed scope must not start deleting its
  // own top-level declarations.
  const nodes = [node('orphan', 'Function', 'some::module::not::loaded')];
  assert.deepEqual([...bodyOwners(nodes).keys()], []);
});

test('a parent named rather than identified still owns its body', () => {
  // Rust `impl` blocks carry the bare type name in `parent_id` — the quirk
  // `descriptionChain` and `qualityPopulation` both allow for.
  const nodes = [
    node('src/a.rs:1:Repo', 'Struct', undefined, 'Repo'),
    node('src/a.rs:4:load', 'Method', 'Repo', 'load'),
    node('src/a.rs:5:arg', 'Parameter', 'load', 'arg'),
  ];
  assert.equal(bodyOwners(nodes).get('src/a.rs:5:arg'), 'src/a.rs:4:load');
});

test('a parent_id cycle terminates instead of hanging the canvas', () => {
  const nodes = [
    node('a', 'Function', 'b'),
    node('b', 'Function', 'a'),
  ];
  assert.doesNotThrow(() => bodyOwners(nodes));
});

test('Branch and Loop own bodies; containers do not', () => {
  for (const k of ['Function', 'Method', 'Branch', 'Loop']) assert.ok(BODY_KINDS.has(k));
  for (const k of ['Struct', 'Class', 'Interface', 'Trait', 'Enum', 'Module', 'File']) {
    assert.ok(!BODY_KINDS.has(k), `${k} encloses declarations, not internals`);
  }
});

// --- the half that keeps the graph honest ---

test('a call made inside a branch is re-routed onto the function making it', () => {
  // The regression the whole lift exists for: on the wire this call hangs off
  // the Branch, so hiding bodies without this drops it, and `parse` reads as
  // calling nothing.
  const g = liftBodies(graph(
    [node('parse', 'Function'), node('br', 'Branch', 'parse'), node('emit', 'Function')],
    [link('parse', 'br', 'Contains'), link('br', 'emit')],
  ));
  const lifted = twins(g);
  assert.equal(lifted.length, 1);
  assert.equal(src(lifted[0]), 'parse');
  assert.equal(tgt(lifted[0]), 'emit');
  assert.equal(lifted[0].kind_raw, 'Calls');
  assert.deepEqual(lifted[0].lifted_from, ['br']);
});

test('a call nested several scopes deep is re-routed the whole way out', () => {
  const g = liftBodies(graph(
    [
      node('parse', 'Function'),
      node('loop1', 'Loop', 'parse'),
      node('br', 'Branch', 'loop1'),
      node('emit', 'Function'),
    ],
    [link('br', 'emit')],
  ));
  assert.equal(src(twins(g)[0]), 'parse');
});

test('the real edge is kept, not replaced', () => {
  // The filter is a view decision. Turning it off has to give the reader the
  // dataset the analyzer sent, branch nodes and branch edges included.
  const original = graph(
    [node('parse', 'Function'), node('br', 'Branch', 'parse'), node('emit', 'Function')],
    [link('br', 'emit')],
  );
  const g = liftBodies(original);
  assert.equal(g.links.filter((l) => !l.lifted_from).length, 1);
  assert.equal(src(g.links[0]), 'br');
});

test('an edge inside one body earns no twin', () => {
  // Both ends lift to `parse`, so the twin would be a loop from a node to
  // itself — 7 179 of them on this repo, every one a dot on top of a circle.
  const g = liftBodies(graph(
    [
      node('parse', 'Function'),
      node('arg', 'Parameter', 'parse'),
      node('br', 'Branch', 'parse'),
    ],
    [link('arg', 'parse', 'TakesParam'), link('parse', 'br', 'Contains')],
  ));
  assert.deepEqual(twins(g), []);
});

test('a relationship that survived on its own is not doubled', () => {
  // `parse` calls `emit` directly at #1 and again inside a branch at #1 —
  // same pair, same call site, one line.
  const g = liftBodies(graph(
    [node('parse', 'Function'), node('br', 'Branch', 'parse'), node('emit', 'Function')],
    [link('parse', 'emit', 'Calls', 1), link('br', 'emit', 'Calls', 1)],
  ));
  assert.deepEqual(twins(g), []);
});

test('two branches calling the same target keep their separate call sites', () => {
  // Distinct `order`s are distinct calls, which is how the canvas already
  // draws two direct calls to one target — the lift must not merge them.
  const g = liftBodies(graph(
    [
      node('parse', 'Function'),
      node('b1', 'Branch', 'parse'),
      node('b2', 'Branch', 'parse'),
      node('emit', 'Function'),
    ],
    [link('b1', 'emit', 'Calls', 3), link('b2', 'emit', 'Calls', 9)],
  ));
  assert.equal(twins(g).length, 2);
  assert.deepEqual(twins(g).map((l) => l.order).sort(), [3, 9]);
});

test('an incoming edge is lifted by its target end', () => {
  const g = liftBodies(graph(
    [node('caller', 'Function'), node('parse', 'Function'), node('br', 'Branch', 'parse')],
    [link('caller', 'br')],
  ));
  assert.equal(tgt(twins(g)[0]), 'parse');
  assert.deepEqual(twins(g)[0].lifted_from, ['br']);
});

test('every internal is stamped, and nothing else is', () => {
  const g = liftBodies(graph([
    node('Repo', 'Struct'),
    node('Repo::load', 'Method', 'Repo'),
    node('arg', 'Parameter', 'Repo::load'),
  ]));
  const stamped = Object.fromEntries(g.nodes.map((n) => [n.id, n.body_of]));
  assert.deepEqual(stamped, { Repo: undefined, 'Repo::load': undefined, arg: 'Repo::load' });
});

test('a graph with no bodies passes through untouched', () => {
  // A schema, a spec, a folder of notes. Nothing to hide, nothing to lift,
  // and no reason to hand every downstream store a fresh array to diff.
  const g = graph(
    [node('users', 'Table'), node('orders', 'Table')],
    [link('orders', 'users', 'References')],
  );
  assert.equal(liftBodies(g), g);
});

// --- reading the stamps back ---

const stamped = (id: string, kind: string, body?: string): D3Node => {
  const n = node(id, kind);
  if (body !== undefined) n.body_of = body;
  return n;
};

test('with the filter off, nothing is an internal and no twin is drawn', () => {
  const off = { structureOnly: false, exemptBody: null };
  assert.equal(bodyHidden(stamped('br', 'Branch', 'parse'), off), false);
  // The real `br → emit` edge draws, so its twin has to stand down or the
  // canvas shows two lines for one call.
  const twin = { ...link('parse', 'emit'), lifted_from: ['br'] };
  assert.equal(linkDrawable(twin, new Set(), off), false);
  assert.equal(linkTraversable(twin, off), false);
});

test('with the filter on, internals are hidden and their twins take over', () => {
  const on = { structureOnly: true, exemptBody: null };
  assert.equal(bodyHidden(stamped('br', 'Branch', 'parse'), on), true);
  assert.equal(bodyHidden(stamped('parse', 'Function'), on), false);
  const twin = { ...link('parse', 'emit'), lifted_from: ['br'] };
  assert.equal(linkDrawable(twin, new Set(['parse', 'emit']), on), true);
});

test('selecting a callable opens its body, and only its body', () => {
  // The gesture the whole filter is built around.
  const on = { structureOnly: true, exemptBody: 'parse' };
  assert.equal(bodyHidden(stamped('br', 'Branch', 'parse'), on), false);
  assert.equal(bodyHidden(stamped('arg', 'Parameter', 'parse'), on), false);
  assert.equal(bodyHidden(stamped('other', 'Branch', 'render'), on), true);
});

test('an opened body takes its own edges back from the twins standing in', () => {
  // `br` is on screen again, so `br → emit` is drawable and the twin
  // `parse → emit` would double it.
  const on = { structureOnly: true, exemptBody: 'parse' };
  const twin = { ...link('parse', 'emit'), lifted_from: ['br'] };
  assert.equal(linkDrawable(twin, new Set(['parse', 'emit', 'br']), on), false);
  // A body that is still hidden keeps its stand-in.
  assert.equal(linkDrawable(twin, new Set(['parse', 'emit']), on), true);
});

test('a real edge is judged by its endpoints, never by this filter', () => {
  const real = link('a', 'b');
  for (const state of [
    { structureOnly: true, exemptBody: null },
    { structureOnly: false, exemptBody: null },
  ]) {
    assert.equal(linkDrawable(real, new Set(), state), true);
    assert.equal(linkTraversable(real, state), true);
  }
});
