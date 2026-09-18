/**
 * Unit tests for the region a reader points at by name (UI-141), or pins
 * there by clicking it (UI-148).
 *
 * Three rules carry the feature, and each is the kind that still *renders*
 * when it is wrong — which is why they are pinned here rather than left to a
 * browser:
 *
 *   - a region shows the rollup for its own path, at its own grain. A file
 *     region reading a folder's row would print a whole subtree's LOC under
 *     one file's name, and look entirely plausible doing it;
 *   - a rung carries a description only when the claim on it is *exact*. An
 *     inherited `cr: "ui/"` covers `ui/src/stores` but is a sentence about
 *     `ui`, and repeating it under `stores` asserts something the author
 *     never wrote;
 *   - the Details column shows the pinned subject over the pointed-at one.
 *     Getting that backwards makes a pin look like it silently failed.
 *
 *   npm run test:region-subject
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ScopeMetrics } from '../src/types/graph.ts';
import type { RegionSpecClaim } from '../src/viewmodels/regionSpec.ts';
import {
  detailSubject,
  isRegionEntry,
  normalizeRegionPath,
  regionChainEntries,
  regionMembership,
  scopeRowFor,
  type HoveredRegion,
} from '../src/viewmodels/regionSubject.ts';

function metrics(path: string, over: Partial<ScopeMetrics> = {}): ScopeMetrics {
  return {
    path,
    entity_count: 10,
    callable_count: 8,
    container_count: 2,
    loc: 400,
    internal_edges: 6,
    external_edges: 3,
    fan_in: 2,
    fan_out: 4,
    in_cycle: false,
    ...over,
  };
}

function region(over: Partial<HoveredRegion> & { path: string }): HoveredRegion {
  return {
    label: over.path.split('/').pop() ?? '',
    grain: 'folder',
    size: 3,
    traffic: null,
    ...over,
  };
}

/** A claim on `path`, exact against whatever subject is asked about it. */
function claim(over: Partial<RegionSpecClaim> & { claimPath: string }): RegionSpecClaim {
  return {
    id: `spec:${over.claimPath}`,
    name: 'Region grouping',
    kind: 'Feature',
    description: `about ${over.claimPath}`,
    exact: true,
    ...over,
  };
}

test('a rollup path matches its region however either side spells it', () => {
  const rows = [{ scope: metrics('ui/src/stores') }];
  assert.equal(scopeRowFor(rows, 'ui/src/stores')?.scope.path, 'ui/src/stores');
  assert.equal(scopeRowFor(rows, 'ui/src/stores/')?.scope.path, 'ui/src/stores');
  assert.equal(scopeRowFor(rows, './ui/src/stores')?.scope.path, 'ui/src/stores');
  assert.equal(normalizeRegionPath('./ui/src/'), 'ui/src');
});

test('a region outside the population gets null, not an empty row', () => {
  // The difference the panel renders as "never measured here" rather than as
  // zeroes, which would read as a folder measured and found empty.
  assert.equal(scopeRowFor([{ scope: metrics('ui/src') }], 'src/server'), null);
});

test('a file region and a folder region read different tables', () => {
  const folders = [{ scope: metrics('ui/src', { loc: 9000 }) }];
  const files = [{ scope: metrics('ui/src/App.svelte', { loc: 120 }) }];
  assert.equal(scopeRowFor(folders, 'ui/src/App.svelte'), null);
  assert.equal(scopeRowFor(files, 'ui/src/App.svelte')?.scope.loc, 120);
});

test('membership falls back to the hull size until the counts arrive', () => {
  assert.deepEqual(regionMembership(region({ path: 'ui/src', size: 7 })), {
    drawn: 7, total: 7, hidden: 0,
  });
});

test('membership reports what a filter is hiding', () => {
  const r = region({
    path: 'ui/src',
    traffic: { drawn: 12, total: 19, inside: 5, crossing: 4 },
  });
  assert.deepEqual(regionMembership(r), { drawn: 12, total: 19, hidden: 7 });
});

test('a folder with no claim anywhere is still its own rung', () => {
  const chain = regionChainEntries(region({ path: 'src/parser' }), () => null);
  assert.equal(chain.length, 1);
  assert.equal(chain[0].name, 'parser');
  assert.equal(chain[0].kind, 'Folder');
  assert.equal(chain[0].documentation, null);
  assert.equal(chain[0].attribution, undefined);
  // Never a source location: the VS Code host turns one into a file open, and
  // this is a directory.
  assert.equal(chain[0].filePath, '');
  assert.equal(chain[0].qualifiedName, 'src/parser');
});

test('an exact claim describes the rung, and is attributed', () => {
  const chain = regionChainEntries(region({ path: 'ui/src/stores' }), (p) =>
    p === 'ui/src/stores' ? claim({ claimPath: p, kind: 'Feature', name: 'State' }) : null);
  assert.equal(chain.length, 1);
  assert.equal(chain[0].documentation, 'about ui/src/stores');
  assert.equal(chain[0].attribution, 'Feature State');
});

test('an inherited claim lands on the folder it is actually about', () => {
  // `cr: "ui"` claims `ui/src/stores` too — but as a sentence about `ui`.
  const claimOf = (p: string): RegionSpecClaim | null => {
    if (p === 'ui') return claim({ claimPath: 'ui', name: 'The browser UI' });
    if (p === 'ui/src/stores' || p === 'ui/src') {
      return claim({ claimPath: 'ui', name: 'The browser UI', exact: false });
    }
    return null;
  };
  const chain = regionChainEntries(region({ path: 'ui/src/stores' }), claimOf);

  // The subject says nothing of its own — the pane shows "No description."
  assert.equal(chain[0].name, 'stores');
  assert.equal(chain[0].documentation, null);
  // `ui/src` inherits too, so it is dropped: an empty rung between the two
  // that matter is what buries the one that speaks.
  assert.equal(chain.length, 2);
  assert.equal(chain[1].name, 'ui');
  assert.equal(chain[1].documentation, 'about ui');
  assert.equal(chain[1].attribution, 'Feature The browser UI');
  assert.deepEqual(chain.map((e) => e.depth), [0, 1]);
});

test('every rung of a region chain is unselectable', () => {
  const chain = regionChainEntries(region({ path: 'ui/src' }), () => null);
  assert.ok(chain.every(isRegionEntry));
  assert.equal(isRegionEntry({ entityId: 'ui/src/App.svelte::render' }), false);
});

test('a file region is labelled as a file, not as a folder', () => {
  const chain = regionChainEntries(
    region({ path: 'ui/src/App.svelte', grain: 'file', label: 'App.svelte' }),
    () => null,
  );
  assert.equal(chain[0].kind, 'File');
});

/**
 * Which of the four candidates the Details column shows — UI-148.
 *
 * The rule renders either way, which is what puts it here: a column showing
 * the wrong one of two real subjects looks exactly like a column showing the
 * right one, and the only way to notice is to already know what you clicked.
 */

/** A stand-in for `D3Node`. `detailSubject` is generic over the node and
 *  reads nothing off it, so the test does not need d3's types to pin the
 *  ranking. */
type Node = { id: string };

function subjectOf(over: Partial<Parameters<typeof detailSubject<Node>>[0]> = {}) {
  return detailSubject<Node>({
    selectedNode: null,
    selectedRegion: null,
    hoveredNode: null,
    hoveredRegion: null,
    ...over,
  });
}

test('nothing pointed at and nothing pinned is no subject at all', () => {
  assert.equal(subjectOf(), null);
});

test('what the pointer is on, when nothing is pinned', () => {
  const node = { id: 'a' };
  const r = region({ path: 'ui/src' });
  assert.deepEqual(subjectOf({ hoveredNode: node }), { kind: 'node', node, pinned: false });
  assert.deepEqual(subjectOf({ hoveredRegion: r }), { kind: 'region', region: r, pinned: false });
  // Both at once: a region is drawn behind the nodes in it, so the pointer is
  // over both whenever it is over one and the finer subject is the aimed-at one.
  assert.deepEqual(
    subjectOf({ hoveredNode: node, hoveredRegion: r }),
    { kind: 'node', node, pinned: false },
  );
});

test('a pinned region outranks anything the pointer wanders over', () => {
  // The whole point of the pin: reading a file's rollup means moving the
  // pointer off the name and across the canvas to reach this column.
  const pinned = region({ path: 'ui/src/App.svelte', grain: 'file' });
  assert.deepEqual(
    subjectOf({ selectedRegion: pinned, hoveredNode: { id: 'a' } }),
    { kind: 'region', region: pinned, pinned: true },
  );
  assert.deepEqual(
    subjectOf({ selectedRegion: pinned, hoveredRegion: region({ path: 'ui/scripts' }) }),
    { kind: 'region', region: pinned, pinned: true },
  );
});

test('a pinned entity outranks a pinned region', () => {
  // The two are mutually exclusive at the store, so this is the formality the
  // module comment says it is — written down so a refactor that breaks the
  // exclusion fails here rather than silently picking whichever store fired
  // last.
  const node = { id: 'a' };
  assert.deepEqual(
    subjectOf({ selectedNode: node, selectedRegion: region({ path: 'ui/src' }) }),
    { kind: 'node', node, pinned: true },
  );
});
