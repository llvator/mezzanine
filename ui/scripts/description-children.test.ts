/**
 * Unit tests for the descendant half of the Description pane
 * (`viewmodels/descriptionChain.ts`).
 *
 * The pane only ever climbed: a Feature read as its own description
 * followed by its ancestors' and never mentioned the Functionalities under
 * it, which for an Elevator spec is most of what the Feature *means*.
 * `buildChildEntries` is the descent, and it is one level only.
 *
 * Same zero-dependency setup as the sibling suites — the module imports
 * nothing at runtime, which is what makes it testable here.
 *
 *   npm run test:children
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  buildChildEntries,
  buildDescriptionChain,
  type DocLookup,
} from '../src/viewmodels/descriptionChain.ts';
import type { D3Node } from '../src/types/graph.ts';

function node(over: Partial<D3Node> = {}): D3Node {
  return {
    id: 'n1',
    original_id: 'n1',
    name: 'Checkout',
    qualified_name: 'Checkout',
    kind: 'Feature',
    kind_raw: 'Feature',
    file_path: 'spec/shop.elv',
    line: 10,
    end_line: 12,
    visibility: 'public',
    parent_id: null,
    parameters: [],
    return_type: null,
    extends: [],
    implements: [],
    tags: [],
    source_code: null,
    fields: [],
    impl_blocks: [],
    language: 'elevator',
    ...over,
  } as D3Node;
}

const NO_DOCS: DocLookup = {};

// ---------------------------------------------------------------------
// The direct children, and only those
// ---------------------------------------------------------------------

test('a feature lists the functionalities declared under it', () => {
  const feature = node({ original_id: 'Checkout', name: 'Checkout' });
  const pay = node({ original_id: 'Checkout::Pay', name: 'Pay', kind_raw: 'Functionality', parent_id: 'Checkout', line: 20 });
  const ship = node({ original_id: 'Checkout::Ship', name: 'Ship', kind_raw: 'Functionality', parent_id: 'Checkout', line: 30 });

  const children = buildChildEntries(feature, [feature, pay, ship], NO_DOCS);

  assert.deepEqual(children.map((c) => c.name), ['Pay', 'Ship']);
  assert.deepEqual(children.map((c) => c.kind), ['Functionality', 'Functionality']);
});

test('grandchildren are not children — the list is one level, never a tree', () => {
  const feature = node({ original_id: 'Checkout' });
  const pay = node({ original_id: 'Checkout::Pay', name: 'Pay', parent_id: 'Checkout', line: 20 });
  const card = node({ original_id: 'Checkout::Pay::Card', name: 'Card', parent_id: 'Checkout::Pay', line: 25 });

  const children = buildChildEntries(feature, [feature, pay, card], NO_DOCS);

  assert.deepEqual(children.map((c) => c.name), ['Pay']);
});

test('an entity with nothing under it lists nothing', () => {
  const leaf = node({ original_id: 'Checkout::Pay', parent_id: 'Checkout' });
  assert.deepEqual(buildChildEntries(leaf, [leaf], NO_DOCS), []);
});

test('an entity that is its own parent does not list itself', () => {
  // Malformed data, but the walk already guards against a cycle and the
  // descent must not be the place a self-loop shows up as a child.
  const self = node({ original_id: 'Loop', parent_id: 'Loop' });
  assert.deepEqual(buildChildEntries(self, [self], NO_DOCS), []);
});

// ---------------------------------------------------------------------
// Ordering — declaration order, which for a `.elv` file is authoring order
// ---------------------------------------------------------------------

test('children come back in declaration order, not graph order', () => {
  const feature = node({ original_id: 'Checkout' });
  const third = node({ original_id: 'c', name: 'Third', parent_id: 'Checkout', line: 40 });
  const first = node({ original_id: 'a', name: 'First', parent_id: 'Checkout', line: 20 });
  const second = node({ original_id: 'b', name: 'Second', parent_id: 'Checkout', line: 30 });

  const children = buildChildEntries(feature, [feature, third, first, second], NO_DOCS);

  assert.deepEqual(children.map((c) => c.name), ['First', 'Second', 'Third']);
});

test('children split across files group by file before line', () => {
  const module = node({ original_id: 'mod', kind_raw: 'Module' });
  const bLate = node({ original_id: 'b2', name: 'bLate', parent_id: 'mod', file_path: 'b.rs', line: 5 });
  const aLate = node({ original_id: 'a2', name: 'aLate', parent_id: 'mod', file_path: 'a.rs', line: 90 });

  const children = buildChildEntries(module, [module, bLate, aLate], NO_DOCS);

  assert.deepEqual(children.map((c) => c.name), ['aLate', 'bLate']);
});

// ---------------------------------------------------------------------
// The `parent_id`-by-name quirk the ancestry walk already allows for
// ---------------------------------------------------------------------

test('a child that names its parent instead of addressing it still resolves', () => {
  // Rust impl blocks carry the bare type name in `parent_id`; the walk
  // indexes both spellings, so the descent has to accept both too.
  const type = node({ original_id: 'src/lib.rs::Parser', name: 'Parser', kind_raw: 'Struct' });
  const method = node({ original_id: 'src/lib.rs::Parser::parse', name: 'parse', parent_id: 'Parser', line: 20 });

  const children = buildChildEntries(type, [type, method], NO_DOCS);

  assert.deepEqual(children.map((c) => c.name), ['parse']);
});

test('a name match loses to the entity that actually owns that id', () => {
  // `Parser` is both the name of one entity and the id of another. The
  // children belong to the id holder; matching on the name as well would
  // let the namesake adopt them.
  const namesake = node({ original_id: 'src/a.rs::Parser', name: 'Parser' });
  const idHolder = node({ original_id: 'Parser', name: 'ParserModule' });
  const child = node({ original_id: 'kid', name: 'kid', parent_id: 'Parser', line: 20 });

  assert.deepEqual(buildChildEntries(namesake, [namesake, idHolder, child], NO_DOCS), []);
  assert.deepEqual(
    buildChildEntries(idHolder, [namesake, idHolder, child], NO_DOCS).map((c) => c.name),
    ['kid'],
  );
});

// ---------------------------------------------------------------------
// Descriptions — the reason the list exists at all
// ---------------------------------------------------------------------

test('each child carries its own description, and a missing one stays visible', () => {
  const feature = node({ original_id: 'Checkout' });
  const pay = node({ original_id: 'Checkout::Pay', name: 'Pay', parent_id: 'Checkout', line: 20 });
  const ship = node({ original_id: 'Checkout::Ship', name: 'Ship', parent_id: 'Checkout', line: 30 });
  const docs: DocLookup = { 'Checkout::Pay': { documentation: 'Takes the money.' } };

  const children = buildChildEntries(feature, [feature, pay, ship], docs);

  assert.deepEqual(children.map((c) => c.documentation), ['Takes the money.', null]);
});

test('the descent and the climb agree about who is whose parent', () => {
  const feature = node({ original_id: 'Checkout' });
  const pay = node({ original_id: 'Checkout::Pay', name: 'Pay', parent_id: 'Checkout', line: 20 });
  const nodes = [feature, pay];

  const child = buildChildEntries(feature, nodes, NO_DOCS)[0];
  const chain = buildDescriptionChain(pay, nodes, NO_DOCS);

  assert.equal(child.entityId, chain[0].entityId);
  assert.equal(chain[1].entityId, feature.original_id);
});
