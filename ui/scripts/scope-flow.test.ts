/**
 * Unit tests for the projection half of the flux reading (UI-146).
 *
 * `flowLayers` orders ids; this decides which ids. The rules worth pinning:
 *
 *   - a folder is read through its IMMEDIATE children, with a subfolder as one
 *     unit — so a file three directories down moves the subfolder, not itself;
 *   - a file is read through its declarations, with a function's internals
 *     folded into the function;
 *   - containment carries no flux, so a class does not float one layer above
 *     its own methods.
 *
 *   npm run test:scopeflow
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { carriesFlux, childUnder, scopeFlow, siblingStanding } from '../src/viewmodels/scopeFlow.ts';
import type { D3Link, D3Node } from '../src/types/graph.ts';

function node(id: string, filePath: string, extra: Partial<D3Node> = {}): D3Node {
  return {
    id,
    original_id: id,
    name: id,
    qualified_name: id,
    kind: 'function',
    kind_raw: 'Function',
    file_path: filePath,
    line: 1,
    end_line: 2,
    visibility: 'Public',
    parent_id: null,
    parameters: [],
    return_type: null,
    extends: [],
    implements: [],
    tags: [],
    source_code: null,
    fields: [],
    impl_blocks: [],
    language: 'TypeScript',
    ...extra,
  } as D3Node;
}

function link(source: string, target: string, kind_raw = 'Calls'): D3Link {
  return {
    source, target,
    kind: kind_raw.toLowerCase(),
    kind_raw,
    incoming_kind: kind_raw.toLowerCase(),
    order: null,
  } as D3Link;
}

test('childUnder answers the immediate child, never the folder itself', () => {
  assert.equal(childUnder('ui/src', 'ui/src/App.svelte'), 'ui/src/App.svelte');
  assert.equal(childUnder('ui/src', 'ui/src/stores/graph.ts'), 'ui/src/stores');
  assert.equal(childUnder('ui/src', 'ui/src/stores/deep/a.ts'), 'ui/src/stores');
  // A trailing slash names the same folder.
  assert.equal(childUnder('ui/src/', 'ui/src/App.svelte'), 'ui/src/App.svelte');
  // The repo root holds top-level entries.
  assert.equal(childUnder('', 'README.md'), 'README.md');
  assert.equal(childUnder('', 'ui/src/App.svelte'), 'ui');
  // Nothing outside it, and never itself.
  assert.equal(childUnder('ui/src', 'src/main.rs'), null);
  assert.equal(childUnder('ui/src', 'ui/src'), null);
});

test('a folder is layered by the relationships between its children', () => {
  const nodes = [
    node('app', 'ui/src/App.svelte'),
    node('store', 'ui/src/stores/graph.ts'),
    node('vm', 'ui/src/viewmodels/plan.ts'),
    node('types', 'ui/src/types/graph.ts'),
  ];
  // App uses the store, the store uses the viewmodel, both name the types.
  const links = [link('app', 'store'), link('store', 'vm'), link('vm', 'types')];
  const flow = scopeFlow(nodes, links, { grain: 'folder', path: 'ui/src' });

  assert.equal(flow.memberNoun, 'file');
  assert.deepEqual([...flow.members.keys()].sort(), [
    'ui/src/App.svelte', 'ui/src/stores', 'ui/src/types', 'ui/src/viewmodels',
  ]);
  assert.equal(flow.members.get('ui/src/stores')!.grain, 'folder');
  assert.equal(flow.members.get('ui/src/App.svelte')!.grain, 'file');
  assert.equal(flow.members.get('ui/src/App.svelte')!.label, 'App.svelte');
  assert.deepEqual(flow.reading.layers, [
    ['ui/src/types'], ['ui/src/viewmodels'], ['ui/src/stores'], ['ui/src/App.svelte'],
  ]);
});

test('a subfolder weighs what it holds', () => {
  const nodes = [
    node('a', 'ui/src/stores/a.ts'),
    node('b', 'ui/src/stores/b.ts'),
    node('c', 'ui/src/App.svelte'),
  ];
  const flow = scopeFlow(nodes, [], { grain: 'folder', path: 'ui/src' });
  assert.equal(flow.members.get('ui/src/stores')!.weight, 2);
  assert.equal(flow.members.get('ui/src/App.svelte')!.weight, 1);
});

test('a file is layered by the entities it declares', () => {
  const nodes = [
    node('main', 'a.ts'),
    node('helper', 'a.ts'),
    node('leaf', 'a.ts'),
    node('elsewhere', 'b.ts'),
  ];
  const links = [link('main', 'helper'), link('helper', 'leaf'), link('main', 'elsewhere')];
  const flow = scopeFlow(nodes, links, { grain: 'file', path: 'a.ts' });

  assert.equal(flow.memberNoun, 'entity');
  assert.deepEqual([...flow.members.keys()].sort(), ['helper', 'leaf', 'main']);
  // The call out to `b.ts` is not part of this file's internal order.
  assert.deepEqual(flow.reading.layers, [['leaf'], ['helper'], ['main']]);
});

test("a function's internals count as the function", () => {
  const nodes = [
    node('caller', 'a.ts'),
    node('branch', 'a.ts', { kind_raw: 'Branch', body_of: 'caller' }),
    node('callee', 'a.ts'),
  ];
  // The call is written inside an `if`, so on the wire it hangs off the
  // branch. Without the projection, `caller` and `callee` look unrelated.
  const links = [link('branch', 'callee')];
  const flow = scopeFlow(nodes, links, { grain: 'file', path: 'a.ts' });

  assert.deepEqual([...flow.members.keys()].sort(), ['callee', 'caller']);
  assert.equal(flow.members.get('caller')!.weight, 2);
  assert.deepEqual(flow.reading.layers, [['callee'], ['caller']]);
});

test('containment carries no flux', () => {
  assert.equal(carriesFlux('Contains'), false);
  assert.equal(carriesFlux('TakesParam'), false);
  assert.equal(carriesFlux('Calls'), true);
  assert.equal(carriesFlux('UsesType'), true);
  assert.equal(carriesFlux('DependsOn'), true);

  const nodes = [
    node('Cls', 'a.ts', { kind_raw: 'Class' }),
    node('method', 'a.ts', { kind_raw: 'Method', parent_id: 'Cls' }),
  ];
  const flow = scopeFlow(nodes, [link('Cls', 'method', 'Contains')], { grain: 'file', path: 'a.ts' });
  // Both are declarations of the file, and neither stands on the other.
  assert.deepEqual(flow.reading.layers, [['Cls', 'method']]);
});

test('a branch with no body_of is still an internal', () => {
  // Found on `ui/src/stores/graph.ts`: a branch inside `derived(…, () => {…})`
  // has a *Variable* for a parent, so `bodyOwners` stops climbing and leaves
  // it unstamped. Fourteen rows named `c1`/`c2`/`l1` before this.
  const nodes = [
    node('store', 'a.ts', { kind_raw: 'Variable' }),
    node('c1', 'a.ts', { kind_raw: 'Branch', parent_id: 'store', tags: ['branch_node'] }),
    node('callee', 'a.ts'),
  ];
  const flow = scopeFlow(nodes, [link('c1', 'callee')], { grain: 'file', path: 'a.ts' });
  assert.deepEqual([...flow.members.keys()].sort(), ['callee', 'store']);
  // And its edge is kept, routed onto the declaration that holds it.
  assert.deepEqual(flow.reading.layers, [['callee'], ['store']]);
});

test("a file's own module entity is not a thing inside it", () => {
  const nodes = [
    node('a.ts', 'a.ts', { kind_raw: 'Module', name: 'a.ts' }),
    node('helper', 'a.ts'),
    // A real `mod` declaration is named for the module, not for the file.
    node('submod', 'a.ts', { kind_raw: 'Module', name: 'submod' }),
  ];
  const flow = scopeFlow(nodes, [], { grain: 'file', path: 'a.ts' });
  assert.deepEqual([...flow.members.keys()].sort(), ['helper', 'submod']);
});

test('ghosts and parameters are not what a file declares', () => {
  const nodes = [
    node('real', 'a.ts'),
    node('p', 'a.ts', { kind_raw: 'Parameter' }),
    node('ghost', 'a.ts', { tags: ['ghost'] }),
  ];
  const flow = scopeFlow(nodes, [], { grain: 'file', path: 'a.ts' });
  assert.deepEqual([...flow.members.keys()], ['real']);
});

test('a scope with nothing in it reads as empty, not as a layer of nothing', () => {
  const flow = scopeFlow([node('a', 'x.ts')], [], { grain: 'folder', path: 'nowhere' });
  assert.equal(flow.members.size, 0);
  assert.deepEqual(flow.reading.layers, []);
});

test('sibling standing is the same computation one level up', () => {
  const nodes = [
    node('s', 'ui/src/stores/a.ts'),
    node('v', 'ui/src/viewmodels/b.ts'),
  ];
  const standing = siblingStanding(nodes, [link('s', 'v')], {
    grain: 'folder', path: 'ui/src/stores',
  });
  assert.ok(standing);
  assert.equal(standing!.parent, 'ui/src');
  assert.equal(standing!.key, 'ui/src/stores');
  assert.equal(standing!.flow.reading.standing.get('ui/src/stores')!.layer, 1);
  assert.equal(standing!.flow.reading.standing.get('ui/src/viewmodels')!.layer, 0);
});

test('an only child and the repo root have no standing to report', () => {
  const nodes = [node('a', 'ui/src/only.ts')];
  assert.equal(siblingStanding(nodes, [], { grain: 'file', path: 'ui/src/only.ts' }), null);
  assert.equal(siblingStanding(nodes, [], { grain: 'folder', path: '' }), null);
});
