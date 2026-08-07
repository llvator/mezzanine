/**
 * Unit tests for the Elevator subgraph (`viewmodels/specGraph.ts`).
 *
 * The split view's cross-filter is only as good as the subtree arithmetic
 * underneath it, and none of the properties that matter are visible by
 * clicking around: a Category with no `cr:` of its own must still filter to
 * everything its Features declare, a Feature claimed by two Categories must
 * not lose one of them, and a spec that declares a containment cycle must
 * render rather than hang the tab.
 *
 *   npm run test:spec-graph
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  buildSpecGraph,
  claimedPaths,
  claimedPathsForAll,
  filterSpecGraph,
  specOptions,
  pathsClaim,
  layoutSpecGraph,
  matchesSpecQuery,
  revealedIds,
  specRoots,
  specScopeStates,
  trailTo,
  specNodesClaiming,
  subtreeOf,
  withAncestors,
  isSpecNode,
  tierOf,
} from '../src/viewmodels/specGraph.ts';
import type { D3Node, D3Link, GraphData } from '../src/types/graph.ts';
import { buildPathUniverse } from '../src/utils/refPaths.ts';
import {
  regionSpecClaim, clampDescription, documentationLookup, hasSpecLayer,
} from '../src/viewmodels/regionSpec.ts';

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

function node(
  id: string,
  kind: string,
  opts: { refs?: string[]; language?: string; file?: string; tags?: string[] } = {},
): D3Node {
  return {
    id,
    original_id: id,
    name: id,
    qualified_name: id,
    kind,
    kind_raw: kind,
    file_path: opts.file ?? 'spec/nao.elv',
    line: 1,
    end_line: 1,
    visibility: 'public',
    parent_id: null,
    parameters: [],
    return_type: null,
    extends: [],
    implements: [],
    tags: opts.tags ?? ['elevator'],
    source_code: null,
    fields: [],
    impl_blocks: [],
    language: opts.language ?? 'Elevator',
    codeRefs: opts.refs?.map((path) => ({ tag: '', path })),
  } as D3Node;
}

function link(source: string, target: string, kind = 'Contains'): D3Link {
  return {
    source,
    target,
    kind,
    kind_raw: kind,
    incoming_kind: kind,
    order: null,
  } as D3Link;
}

/**
 *   c visualizer ── f canvas    ── fu render   (cr: ui/src/components/GraphView.svelte)
 *                └─ f filters   (cr: ui/src/viewmodels/)
 *   c engine     ── f filters                  (same Feature, second Category)
 *   concept theming (cr: ui/src/app.css)
 *   plus one code entity that must not enter the spec graph.
 */
function fixture(): GraphData {
  return {
    nodes: [
      node('c_visualizer', 'Category'),
      node('c_engine', 'Category'),
      node('f_canvas', 'Feature'),
      node('f_filters', 'Feature', { refs: ['ui/src/viewmodels/'] }),
      node('fu_render', 'Functionality', {
        refs: ['./ui/src/components/GraphView.svelte'],
      }),
      node('concept_theming', 'Concept', { refs: ['ui/src/app.css'] }),
      node('code_thing', 'Function', {
        language: 'Rust',
        file: 'src/graph.rs',
        tags: [],
      }),
    ],
    links: [
      link('c_visualizer', 'f_canvas'),
      link('c_visualizer', 'f_filters'),
      link('c_engine', 'f_filters'),
      link('f_canvas', 'fu_render'),
      link('f_canvas', 'concept_theming', 'References'),
      link('c_visualizer', 'code_thing'),
    ],
  } as GraphData;
}

// ---------------------------------------------------------------------
// isSpecNode / buildSpecGraph
// ---------------------------------------------------------------------

test('the spec layer is the Elevator language, not the whole graph', () => {
  const graph = buildSpecGraph(fixture());
  assert.equal(graph.nodes.length, 6);
  assert.ok(!graph.nodes.some((n) => n.id === 'code_thing'));
});

test('a ghost is not a node in the spec hierarchy', () => {
  assert.equal(isSpecNode(node('f_x', 'Feature', { tags: ['elevator', 'ghost'] })), false);
});

test('an edge with one end outside the spec layer is dropped', () => {
  const graph = buildSpecGraph(fixture());
  assert.ok(!graph.links.some((l) => l.target === 'code_thing'));
});

test('cross-links that are not containment survive as links but not as parents', () => {
  const graph = buildSpecGraph(fixture());
  assert.ok(graph.links.some((l) => l.kind_raw === 'References'));
  assert.deepEqual(graph.parents.get('concept_theming'), undefined);
});

test('a project with no spec layer reports empty rather than a blank graph', () => {
  const graph = buildSpecGraph({
    nodes: [node('code_thing', 'Function', { language: 'Rust', tags: [] })],
    links: [],
  } as GraphData);
  assert.equal(graph.empty, true);
});

test('no graph at all is empty, not a crash', () => {
  assert.equal(buildSpecGraph(null).empty, true);
});

// ---------------------------------------------------------------------
// claimedPaths — the subtree roll-up
// ---------------------------------------------------------------------

test('a behaviourless Category claims what its subtree declares', () => {
  // The whole point: `c visualizer` carries no `cr:` of its own, and without
  // the roll-up clicking it would filter the code pane to nothing.
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(
    claimedPaths(graph, 'c_visualizer').slice().sort(),
    ['ui/src/components/GraphView.svelte', 'ui/src/viewmodels'].sort(),
  );
});

test('a leaf claims exactly its own ref', () => {
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(claimedPaths(graph, 'fu_render'), ['ui/src/components/GraphView.svelte']);
});

test('rolled-up paths are normalized on the way in', () => {
  // `fu_render` declares `./ui/src/...`; the code pane compares against
  // `file_path`s that may carry either spelling.
  const graph = buildSpecGraph(fixture());
  assert.ok(!claimedPaths(graph, 'fu_render')[0].startsWith('./'));
});

test('a Feature under two Categories is claimed by both', () => {
  const graph = buildSpecGraph(fixture());
  assert.ok(claimedPaths(graph, 'c_engine').includes('ui/src/viewmodels'));
  assert.ok(claimedPaths(graph, 'c_visualizer').includes('ui/src/viewmodels'));
});

test('an entity with no refs anywhere in its subtree claims nothing', () => {
  const graph = buildSpecGraph({
    nodes: [node('c_empty', 'Category'), node('f_empty', 'Feature')],
    links: [link('c_empty', 'f_empty')],
  } as GraphData);
  assert.deepEqual(claimedPaths(graph, 'c_empty'), []);
});

test('claiming nothing is not the same as claiming everything', () => {
  // Guards the store contract: an unanchored entity must leave the code pane
  // filtered to zero, not unfiltered. `pathsClaim([])` is the predicate that
  // decides which of the two happens.
  assert.equal(pathsClaim([], 'src/graph.rs'), false);
});

test('a containment cycle terminates instead of hanging the tab', () => {
  const graph = buildSpecGraph({
    nodes: [
      node('f_a', 'Feature', { refs: ['src/a'] }),
      node('f_b', 'Feature', { refs: ['src/b'] }),
    ],
    links: [link('f_a', 'f_b'), link('f_b', 'f_a')],
  } as GraphData);
  assert.deepEqual(claimedPaths(graph, 'f_a').slice().sort(), ['src/a', 'src/b']);
});

test('nothing selected claims nothing', () => {
  assert.deepEqual(claimedPaths(buildSpecGraph(fixture()), null), []);
});

// ---------------------------------------------------------------------
// pathsClaim — the code-pane predicate
// ---------------------------------------------------------------------

test('a folder ref claims the files under it', () => {
  assert.equal(pathsClaim(['ui/src/viewmodels'], 'ui/src/viewmodels/displayPlan.ts'), true);
});

test('a folder ref does not claim its lookalike sibling', () => {
  // The bug `pathClaims` exists to prevent, asserted at this layer too because
  // this is the call the cross-filter actually makes.
  assert.equal(pathsClaim(['ui/src/view'], 'ui/src/viewmodels/displayPlan.ts'), false);
});

test('a ghost with no file is claimed by nobody', () => {
  assert.equal(pathsClaim(['ui/src'], ''), false);
});

// ---------------------------------------------------------------------
// specNodesClaiming / withAncestors — the reverse leg
// ---------------------------------------------------------------------

test('the reverse lookup names the entity that declared the ref', () => {
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(
    specNodesClaiming(graph, 'ui/src/components/GraphView.svelte'),
    ['fu_render'],
  );
});

test('the reverse lookup reads own refs, not rolled-up ones', () => {
  // If it rolled up, both Categories would claim GraphView.svelte and the
  // highlight would spread across the whole pane instead of pointing at the
  // Functionality that actually declared it.
  const graph = buildSpecGraph(fixture());
  assert.ok(!specNodesClaiming(graph, 'ui/src/components/GraphView.svelte').includes('c_visualizer'));
});

test('the most specific claimant is listed first', () => {
  const graph = buildSpecGraph({
    nodes: [
      node('f_broad', 'Feature', { refs: ['ui/src'] }),
      node('f_precise', 'Feature', { refs: ['ui/src/viewmodels/displayPlan.ts'] }),
    ],
    links: [],
  } as GraphData);
  assert.deepEqual(
    specNodesClaiming(graph, 'ui/src/viewmodels/displayPlan.ts'),
    ['f_precise', 'f_broad'],
  );
});

test('the trail to a claimant includes the Categories it hangs from', () => {
  const graph = buildSpecGraph(fixture());
  const trail = withAncestors(graph, ['fu_render']);
  assert.deepEqual([...trail].sort(), ['c_visualizer', 'f_canvas', 'fu_render'].sort());
});

test('the trail follows every parent of a multi-parent Feature', () => {
  const graph = buildSpecGraph(fixture());
  const trail = withAncestors(graph, ['f_filters']);
  assert.ok(trail.has('c_visualizer'));
  assert.ok(trail.has('c_engine'));
});

// ---------------------------------------------------------------------
// subtreeOf — the emphasis set, which must match the filter set
// ---------------------------------------------------------------------

test('the emphasised subtree is the one the filter was computed from', () => {
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(
    [...subtreeOf(graph, 'c_visualizer')].sort(),
    ['c_visualizer', 'f_canvas', 'f_filters', 'fu_render'].sort(),
  );
});

test('a Concept referenced but not contained is outside the subtree', () => {
  // `claimedPaths` walks Contains only, so the emphasis must too — otherwise
  // the pane lights up an entity whose code the filter did not admit.
  const graph = buildSpecGraph(fixture());
  assert.ok(!subtreeOf(graph, 'f_canvas').has('concept_theming'));
});

test('nothing focused emphasises nothing', () => {
  assert.equal(subtreeOf(buildSpecGraph(fixture()), null).size, 0);
});

test('a cyclic subtree terminates', () => {
  const graph = buildSpecGraph({
    nodes: [node('f_a', 'Feature'), node('f_b', 'Feature')],
    links: [link('f_a', 'f_b'), link('f_b', 'f_a')],
  } as GraphData);
  assert.deepEqual([...subtreeOf(graph, 'f_a')].sort(), ['f_a', 'f_b']);
});

test('a cyclic trail terminates', () => {
  const graph = buildSpecGraph({
    nodes: [node('f_a', 'Feature'), node('f_b', 'Feature')],
    links: [link('f_a', 'f_b'), link('f_b', 'f_a')],
  } as GraphData);
  assert.deepEqual([...withAncestors(graph, ['f_a'])].sort(), ['f_a', 'f_b']);
});

// ---------------------------------------------------------------------
// claimedPathsForAll — multi-select
// ---------------------------------------------------------------------

test('selecting two entities shows the union of what they claim', () => {
  // Union, not intersection: two Categories checked together asks for the code
  // belonging to either. An intersection would be empty here, and near-always.
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(
    claimedPathsForAll(graph, ['fu_render', 'concept_theming']).slice().sort(),
    ['ui/src/app.css', 'ui/src/components/GraphView.svelte'].sort(),
  );
});

test('overlapping claims are not double-counted', () => {
  const graph = buildSpecGraph(fixture());
  const both = claimedPathsForAll(graph, ['c_visualizer', 'f_filters']);
  assert.equal(new Set(both).size, both.length);
});

test('selecting nothing claims nothing', () => {
  assert.deepEqual(claimedPathsForAll(buildSpecGraph(fixture()), []), []);
});

test('an unanchored entity in the selection contributes nothing, not everything', () => {
  // The store reads `[]` as "claims no code" and draws an empty canvas; if this
  // returned every path instead, checking a Feature with no `cr:` would look
  // like clearing the filter.
  const graph = buildSpecGraph({
    nodes: [node('f_bare', 'Feature'), node('f_real', 'Feature', { refs: ['src/a'] })],
    links: [],
  } as GraphData);
  assert.deepEqual(claimedPathsForAll(graph, ['f_bare']), []);
  assert.deepEqual(claimedPathsForAll(graph, ['f_bare', 'f_real']), ['src/a']);
});

// ---------------------------------------------------------------------
// specOptions — the Filters pane's pick lists
// ---------------------------------------------------------------------

test('every tier is offered when not drilling down', () => {
  // The list's advantage over the pane: reach a Functionality without knowing
  // which Category it hangs from.
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(), false);
  assert.deepEqual(tiers.map((t) => t.kind), ['Category', 'Feature', 'Functionality', 'Concept']);
});

test('tiers come back in language order', () => {
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(), false);
  assert.ok(tiers[0].tier < tiers[1].tier);
});

test('drilling down offers only children of what is checked', () => {
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(['c_visualizer']), true);
  const features = tiers.find((t) => t.kind === 'Feature')!;
  assert.deepEqual(features.nodes.map((n) => n.id).sort(), ['f_canvas', 'f_filters']);
  assert.ok(!tiers.some((t) => t.kind === 'Functionality'), 'a grandchild waits for its own parent');
});

test('drilling down still offers the roots, with nothing checked', () => {
  // Otherwise the list opens empty and there is nothing to check first.
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(), true);
  assert.deepEqual(tiers.find((t) => t.kind === 'Category')!.nodes.length, 2);
});

test('a Concept is offered while drilling down, being contained by nothing', () => {
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(), true);
  assert.ok(tiers.some((t) => t.kind === 'Concept'));
});

test('a checked entity stays offered even when the drill rule would hide it', () => {
  // Otherwise unchecking its parent strands it checked and unreachable, still
  // filtering the canvas from a row that is no longer on screen.
  const graph = buildSpecGraph(fixture());
  const tiers = specOptions(graph, new Set(['f_canvas']), true);
  const features = tiers.find((t) => t.kind === 'Feature')!;
  assert.ok(features.nodes.some((n) => n.id === 'f_canvas'));
});

test('the pick lists are sorted by name, not graph order', () => {
  const graph = buildSpecGraph(fixture());
  const features = specOptions(graph, new Set(), false).find((t) => t.kind === 'Feature')!;
  assert.deepEqual(
    features.nodes.map((n) => n.name),
    features.nodes.map((n) => n.name).slice().sort(),
  );
});

// ---------------------------------------------------------------------
// matchesSpecQuery — the pick list's per-tier filter box
// ---------------------------------------------------------------------

test('an empty box is not a filter', () => {
  assert.equal(matchesSpecQuery(node('f_canvas', 'Feature'), ''), true);
  assert.equal(matchesSpecQuery(node('f_canvas', 'Feature'), '   '), true);
});

test('the filter is case-insensitive on the name', () => {
  assert.equal(matchesSpecQuery(node('f_canvas', 'Feature'), 'CANV'), true);
});

test('the qualified name matches too', () => {
  // What `elevator --list` prints, and so what a reader arriving from it types.
  const n = node('x', 'Functionality');
  n.name = 'cohere';
  n.qualified_name = 'f.grouping.cohere';
  assert.equal(matchesSpecQuery(n, 'grouping.cohere'), true);
  assert.equal(matchesSpecQuery(n, 'cohere'), true);
});

test('a non-match is excluded', () => {
  assert.equal(matchesSpecQuery(node('f_canvas', 'Feature'), 'zzz'), false);
});

test('surrounding whitespace is not part of the query', () => {
  assert.equal(matchesSpecQuery(node('f_canvas', 'Feature'), '  canvas  '), true);
});

// ---------------------------------------------------------------------
// Drill-down — specRoots / trailTo / revealedIds
// ---------------------------------------------------------------------

test('the pane opens on what nothing contains', () => {
  // Categories and the Concept; not the Features and Functionalities under
  // them. Derived from containment, so a spec with Extensions would open on
  // those instead without this needing to know about kinds.
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(specRoots(graph).sort(), ['c_engine', 'c_visualizer', 'concept_theming'].sort());
});

test('an orphan Feature is a root rather than being unreachable', () => {
  const graph = buildSpecGraph({
    nodes: [node('c_a', 'Category'), node('f_orphan', 'Feature')],
    links: [],
  } as GraphData);
  assert.ok(specRoots(graph).includes('f_orphan'));
});

test('the trail runs root-first down to the entity', () => {
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(trailTo(graph, 'fu_render'), ['c_visualizer', 'f_canvas', 'fu_render']);
});

test('a trail through a containment cycle terminates', () => {
  const graph = buildSpecGraph({
    nodes: [node('f_a', 'Feature'), node('f_b', 'Feature')],
    links: [link('f_a', 'f_b'), link('f_b', 'f_a')],
  } as GraphData);
  assert.ok(trailTo(graph, 'f_a').length <= 2);
});

test('nothing open draws only the roots', () => {
  // The whole point of the rework: 76 entities became 8.
  const graph = buildSpecGraph(fixture());
  assert.deepEqual(
    [...revealedIds(graph, [])].sort(),
    ['c_engine', 'c_visualizer', 'concept_theming'].sort(),
  );
});

test('opening a Category reveals its Features and no one else\'s', () => {
  const graph = buildSpecGraph(fixture());
  const shown = revealedIds(graph, ['c_visualizer']);
  assert.ok(shown.has('f_canvas'));
  assert.ok(shown.has('f_filters'));
  assert.ok(!shown.has('fu_render'), 'a grandchild must wait for its own parent to open');
});

test('a deeper path keeps the levels above it drawn', () => {
  // Children of *every* step, not just the last — otherwise the pane forgets
  // which Feature among its siblings you descended through.
  const graph = buildSpecGraph(fixture());
  const shown = revealedIds(graph, ['c_visualizer', 'f_canvas']);
  assert.ok(shown.has('f_filters'), 'the sibling Feature stays for context');
  assert.ok(shown.has('fu_render'), 'the newly opened level appears');
});

test('a path step the graph no longer holds is skipped, not fatal', () => {
  // Re-analysis or a scope change can strip an entity out from under an open
  // path; the levels above it must still draw.
  const graph = buildSpecGraph(fixture());
  const shown = revealedIds(graph, ['c_visualizer', 'f_gone', 'fu_render']);
  assert.ok(shown.has('f_canvas'));
  assert.ok(!shown.has('fu_render'), 'the broken link in the chain stops the descent');
});

test('a code selection reveals its claimant even with nothing open', () => {
  // The reverse leg has to survive progressive disclosure: a claimant three
  // levels down must be ringable without the reader having drilled to it.
  const graph = buildSpecGraph(fixture());
  const shown = revealedIds(graph, [], new Set(['fu_render']));
  assert.ok(shown.has('fu_render'));
  assert.ok(shown.has('f_canvas'), 'and its trail, so it is not floating');
  assert.ok(shown.has('c_visualizer'));
});

test('revealing for a highlight does not drag in the claimant\'s siblings', () => {
  const graph = buildSpecGraph(fixture());
  const shown = revealedIds(graph, [], new Set(['fu_render']));
  assert.ok(!shown.has('f_filters'), 'only the trail, not every level it passes');
});

test('drilling composes with the scope filter', () => {
  // Scope first, then reveal. An entity the scope removed must not come back
  // because the drill path names it.
  const graph = buildSpecGraph(fixture());
  const scoped = filterSpecGraph(graph, new Set(['c_visualizer', 'f_canvas']));
  const shown = revealedIds(scoped, ['c_visualizer', 'f_canvas']);
  assert.ok(!shown.has('fu_render'));
  assert.ok(shown.has('f_canvas'));
});

// ---------------------------------------------------------------------
// specScopeStates — why an entity will show nothing
// ---------------------------------------------------------------------

/** The fixture's claims are `ui/src/viewmodels/`, `ui/src/components/GraphView.svelte`
 *  and `ui/src/app.css`; `c_engine` has the first, `c_visualizer` both. */
const REPO = buildPathUniverse([
  'ui/src/viewmodels/displayPlan.ts',
  'ui/src/components/GraphView.svelte',
  'ui/src/app.css',
  'src/parser/mod.rs',
]);

test('an entity whose claims are loaded is in scope', () => {
  const graph = buildSpecGraph(fixture());
  const states = specScopeStates(graph, REPO, REPO);
  assert.equal(states.get('fu_render'), 'in-scope');
});

test('claims outside the analysis scope are out of scope, not drift', () => {
  // The distinction the whole marker exists for: the `cr:` is correct and the
  // code is real, it just is not loaded. Telling the reader this is drift
  // sends them editing a spec that is fine.
  const graph = buildSpecGraph(fixture());
  const loaded = buildPathUniverse(['src/parser/mod.rs']);
  const states = specScopeStates(graph, loaded, REPO);
  assert.equal(states.get('fu_render'), 'out-of-scope');
});

test('claims matching nothing analysed are reported without asserting drift', () => {
  // `unanalyzed`, not `stale`: this cannot distinguish a dead path from one
  // whose file type is outside the parsed language set. `f recall_bench`
  // claims a `.py` script this repo does not parse, and calling that drift put
  // a warning badge on a `cr:` that `elevator --drift` confirms is correct.
  const graph = buildSpecGraph(fixture());
  const empty = buildPathUniverse([]);
  const states = specScopeStates(graph, empty, empty);
  assert.equal(states.get('fu_render'), 'unanalyzed');
});

test('an entity with no cr: anywhere is unanchored, whatever the scope', () => {
  // Must not read as out-of-scope: widening the scope will never help, and
  // this is the state worth acting on.
  const graph = buildSpecGraph({
    nodes: [node('c_empty', 'Category'), node('f_empty', 'Feature')],
    links: [link('c_empty', 'f_empty')],
  } as GraphData);
  const states = specScopeStates(graph, REPO, REPO);
  assert.equal(states.get('c_empty'), 'unanchored');
});

test('a folder claim is in scope when anything under it is loaded', () => {
  // `f_filters` claims `ui/src/viewmodels`; the scope loads one file inside
  // it. Clicking would show that file, so the entity must not be marked.
  const graph = buildSpecGraph(fixture());
  const loaded = buildPathUniverse(['ui/src/viewmodels/displayPlan.ts']);
  assert.equal(specScopeStates(graph, loaded, REPO).get('f_filters'), 'in-scope');
});

test('a broad claim survives a narrowing inside it', () => {
  // The scope is narrower than the ref. Clicking still shows something, so
  // "in scope" is the honest answer even though most of what it claims is out.
  const graph = buildSpecGraph({
    nodes: [node('f_broad', 'Feature', { refs: ['src'] })],
    links: [],
  } as GraphData);
  const loaded = buildPathUniverse(['src/parser/mod.rs']);
  assert.equal(specScopeStates(graph, loaded, REPO).get('f_broad'), 'in-scope');
});

test('a partly-in-scope Category counts as in scope', () => {
  // `c_visualizer` rolls up refs under both `ui/src/viewmodels` and
  // `ui/src/components`; only the latter is loaded.
  const graph = buildSpecGraph(fixture());
  const loaded = buildPathUniverse(['ui/src/components/GraphView.svelte']);
  assert.equal(specScopeStates(graph, loaded, REPO).get('c_visualizer'), 'in-scope');
});

test('a Category is classified on its roll-up, not its own refs', () => {
  // Own-refs-only would mark every Category unanchored, since a Category
  // never carries a `cr:`.
  const graph = buildSpecGraph(fixture());
  assert.notEqual(specScopeStates(graph, REPO, REPO).get('c_visualizer'), 'unanchored');
});

// ---------------------------------------------------------------------
// filterSpecGraph — the "follow the analysis scope" mode
// ---------------------------------------------------------------------

test('following the scope drops the entities that are not in it', () => {
  const graph = buildSpecGraph(fixture());
  const kept = filterSpecGraph(graph, new Set(['c_visualizer', 'f_canvas']));
  assert.deepEqual(kept.nodes.map((n) => n.id).sort(), ['c_visualizer', 'f_canvas']);
});

test('an edge to a dropped entity is dropped with it', () => {
  const graph = buildSpecGraph(fixture());
  const kept = filterSpecGraph(graph, new Set(['c_visualizer', 'f_canvas']));
  assert.deepEqual(kept.children.get('f_canvas'), undefined);
  assert.deepEqual(kept.parents.get('f_canvas'), ['c_visualizer']);
});

test('a filtered Category still claims everything it claimed', () => {
  // The contract that makes the toggle safe: what a Category means to the code
  // pane cannot depend on whether its children happen to be drawn. Recomputing
  // the roll-up over survivors would silently shrink the filter.
  const graph = buildSpecGraph(fixture());
  const kept = filterSpecGraph(graph, new Set(['c_visualizer']));
  assert.deepEqual(
    claimedPaths(kept, 'c_visualizer').slice().sort(),
    claimedPaths(graph, 'c_visualizer').slice().sort(),
  );
});

test('keeping everything returns the same graph untouched', () => {
  const graph = buildSpecGraph(fixture());
  assert.equal(filterSpecGraph(graph, new Set(graph.nodes.map((n) => n.id))), graph);
});

test('keeping nothing is empty, not a graph with no nodes', () => {
  // The pane branches on `empty` to choose its message; a non-empty graph with
  // zero nodes would render a blank canvas and say nothing.
  const graph = buildSpecGraph(fixture());
  assert.equal(filterSpecGraph(graph, new Set()).empty, true);
});

// ---------------------------------------------------------------------
// layoutSpecGraph — wrapped tiers
// ---------------------------------------------------------------------

/** A tier of `count` Features hanging off one Category. */
function wideFixture(count: number): GraphData {
  const nodes = [node('c_root', 'Category')];
  const links: D3Link[] = [];
  for (let i = 0; i < count; i++) {
    nodes.push(node(`f_${String(i).padStart(3, '0')}`, 'Feature'));
    links.push(link('c_root', `f_${String(i).padStart(3, '0')}`));
  }
  return { nodes, links } as GraphData;
}

test('every entity gets a position', () => {
  const graph = buildSpecGraph(fixture());
  const layout = layoutSpecGraph(graph, 400);
  for (const n of graph.nodes) assert.ok(layout.positions.has(n.id), n.id);
});

test('a tier wider than the pane wraps instead of overflowing it', () => {
  // The reason this layout exists. This repo's own spec has 76 Features; on
  // one row they are ~2300px wide, which a 400px pane can neither show nor
  // shrink to legibly.
  const layout = layoutSpecGraph(buildSpecGraph(wideFixture(76)), 400);
  const xs = [...layout.positions.values()].map((p) => p.x);
  assert.ok(Math.max(...xs) <= 400, `widest node at ${Math.max(...xs)}`);
});

test('wrapping is what makes it taller, not shorter', () => {
  const narrow = layoutSpecGraph(buildSpecGraph(wideFixture(76)), 300);
  const wide = layoutSpecGraph(buildSpecGraph(wideFixture(76)), 700);
  assert.ok(narrow.height > wide.height);
});

test('a pane too narrow for even one column still lays out', () => {
  // `Math.floor` of a tiny width is 0 columns, and `index % 0` is NaN — every
  // node would land at `translate(NaN,NaN)` and vanish.
  const layout = layoutSpecGraph(buildSpecGraph(wideFixture(4)), 10);
  for (const p of layout.positions.values()) {
    assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y));
  }
});

test('tiers stack downwards in language order', () => {
  const graph = buildSpecGraph(fixture());
  const layout = layoutSpecGraph(graph, 400);
  const y = (id: string) => layout.positions.get(id)!.y;
  assert.ok(y('c_visualizer') < y('f_canvas'));
  assert.ok(y('f_canvas') < y('fu_render'));
});

test('a band caption names its tier and counts it', () => {
  const layout = layoutSpecGraph(buildSpecGraph(fixture()), 400);
  const features = layout.bands.find((b) => b.kind === 'Feature');
  assert.equal(features?.count, 2);
});

test('an empty tier gets no caption', () => {
  // The fixture has no Extension and no UI Page; captioning them would promise
  // rows that are not there.
  const layout = layoutSpecGraph(buildSpecGraph(fixture()), 400);
  assert.deepEqual(
    layout.bands.map((b) => b.kind),
    ['Category', 'Feature', 'Functionality', 'Concept'],
  );
});

test('children sit under the parent that declares them', () => {
  // Ordering by the parent's rank is what keeps the containment edges short.
  // `f_z` is declared by the first Category and must not sort after `f_a`,
  // which the second one declares, merely because of its name.
  const graph = buildSpecGraph({
    nodes: [
      node('c_1', 'Category'), node('c_2', 'Category'),
      node('f_z', 'Feature'), node('f_a', 'Feature'),
    ],
    links: [link('c_1', 'f_z'), link('c_2', 'f_a')],
  } as GraphData);
  const layout = layoutSpecGraph(graph, 400);
  assert.ok(layout.positions.get('f_z')!.x < layout.positions.get('f_a')!.x);
});

test('the same spec at the same width lays out identically', () => {
  const a = layoutSpecGraph(buildSpecGraph(fixture()), 400);
  const b = layoutSpecGraph(buildSpecGraph(fixture()), 400);
  assert.deepEqual([...a.positions.entries()], [...b.positions.entries()]);
});

test('an empty spec lays out to nothing rather than throwing', () => {
  const layout = layoutSpecGraph(buildSpecGraph(null), 400);
  assert.equal(layout.positions.size, 0);
  assert.equal(layout.height, 0);
});

// ---------------------------------------------------------------------
// tierOf
// ---------------------------------------------------------------------

test('the hierarchy draws in language order', () => {
  assert.ok(tierOf(node('e', 'Extension')) < tierOf(node('c', 'Category')));
  assert.ok(tierOf(node('c', 'Category')) < tierOf(node('f', 'Feature')));
  assert.ok(tierOf(node('f', 'Feature')) < tierOf(node('fu', 'Functionality')));
});

test('the same kind always draws on the same row', () => {
  // A Feature reachable at two depths (two Categories claim it) must not be
  // drawn on two different rows.
  assert.equal(tierOf(node('f_one', 'Feature')), tierOf(node('f_two', 'Feature')));
});

test('a kind the language grows later is visibly unplaced, not silently a Feature', () => {
  assert.ok(tierOf(node('x', 'SomethingNew')) > tierOf(node('f', 'Feature')));
});

// ---------------------------------------------------------------------
// regionSpecClaim (UI-090)
// ---------------------------------------------------------------------

/** A folder region asks the same question a file does — the join is prefix
 *  arithmetic and has no opinion about which side is a directory. */
const noDocs = () => null;
const docsFor = (map: Record<string, string>) => (id: string) => map[id] ?? null;

test('a folder claimed by a cr: gets that entity', () => {
  const g = buildSpecGraph(fixture());
  const claim = regionSpecClaim(g, 'ui/src/viewmodels', noDocs);
  assert.equal(claim?.name, 'f_filters');
  assert.equal(claim?.kind, 'Feature');
  assert.equal(claim?.exact, true);
});

test('the description comes from the sidecar, keyed by the analyzer id', () => {
  const g = buildSpecGraph(fixture());
  const claim = regionSpecClaim(g, 'ui/src/viewmodels', docsFor({ f_filters: 'Filtering.' }));
  assert.equal(claim?.description, 'Filtering.');
});

test('a claim inherited from a folder above says so', () => {
  // `cr: "ui/src/viewmodels/"` genuinely covers `ui/src/viewmodels/deep`, but
  // the words it carries are about the folder the author named. Reading them
  // as a description of the subfolder would put words in their mouth.
  const g = buildSpecGraph(fixture());
  const claim = regionSpecClaim(g, 'ui/src/viewmodels/deep', noDocs);
  assert.equal(claim?.name, 'f_filters');
  assert.equal(claim?.exact, false);
  assert.equal(claim?.claimPath, 'ui/src/viewmodels');
});

test('an unclaimed folder reports nothing rather than the nearest name', () => {
  const g = buildSpecGraph(fixture());
  assert.equal(regionSpecClaim(g, 'src/parser', noDocs), null);
});

test('a description that has not loaded is a null, not a claim of silence', () => {
  const g = buildSpecGraph(fixture());
  const claim = regionSpecClaim(g, 'ui/src/viewmodels', documentationLookup(null));
  assert.equal(claim?.description, null);
});

test('the most specific claim wins over the folder-wide one', () => {
  const g = buildSpecGraph({
    nodes: [
      node('f_wide', 'Feature', { refs: ['ui/'] }),
      node('f_tight', 'Feature', { refs: ['ui/src/stores'] }),
    ],
    links: [],
    files: [],
    modules: [],
  } as unknown as GraphData);
  assert.equal(regionSpecClaim(g, 'ui/src/stores', noDocs)?.name, 'f_tight');
  assert.equal(regionSpecClaim(g, 'ui/src/utils', noDocs)?.name, 'f_wide');
});

test('a project with no spec layer claims nothing', () => {
  const g = buildSpecGraph(null);
  assert.equal(regionSpecClaim(g, 'ui/src/stores', noDocs), null);
  assert.equal(hasSpecLayer(g), false);
});

test('a paragraph is cut to a card-sized line, on a word boundary', () => {
  const long = `${'word '.repeat(120)}end`;
  const out = clampDescription(long)!;
  assert.ok(out.length <= 221, `clamped to ${out.length}`);
  assert.ok(out.endsWith('…'));
  assert.ok(!out.includes('  '), 'newlines and runs of space should collapse');
});

test('a short description is left exactly as written', () => {
  assert.equal(clampDescription('Lexer/parser for .elv.'), 'Lexer/parser for .elv.');
  assert.equal(clampDescription(''), null);
  assert.equal(clampDescription(null), null);
});
