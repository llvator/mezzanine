/**
 * FilterViewModel — owns the contract between `graphData` and the filter
 * state stores. Its job is to keep the filter state consistent with whatever
 * dataset is currently published, so upstream code (scope selection) never
 * has to reach into filter internals to "pre-seed" them.
 *
 * The underlying stores still live in `stores/graph.ts` (the Model layer).
 * This module is the orchestration + action surface that components bind to.
 */

import { derived, get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import type { D3Node, GraphData, TriState } from '../types/graph';
import {
  graphData,
  rawEntityGraph,
  selectedNode,
  generalEntityTypes,
  generalRelTypes,
  generalOutgoing,
  generalIncoming,
  generalLanguages,
  hiddenLanguages,
  allLanguages,
  allFiles,
  visibleFiles,
  hiddenFiles,
  levelOverrides,
} from '../stores/graph';
import { parseQuery, scoreQuery, violatesNegation } from '../utils/fuzzyPath';
import { scoreEntity } from '../utils/entityScore';

export { showDirectEdges, showCrossLevelEdges } from '../stores/graph';

// --- Read-only re-exports for views ---
export {
  generalEntityTypes,
  generalRelTypes,
  generalOutgoing,
  generalIncoming,
  generalLanguages,
  hiddenLanguages,
  visibleFiles,
  hiddenFiles,
  levelOverrides,
  searchHidesNonMatches,
  searchDimOpacity,
  allEntityTypes,
  allRelTypes,
  allLanguages,
  allFiles,
} from '../stores/graph';

/**
 * Re-seed every filter to "show everything in the new dataset". Private to
 * this module — callers use `publishGraph` below so the seed-then-set order
 * is guaranteed.
 */
function seedFromGraph(data: GraphData): void {
  const entityTypes = new Set(data.nodes.map((n) => n.kind_raw));
  const relTypes = new Set(data.links.map((l) => l.kind_raw));

  generalEntityTypes.set(entityTypes);
  generalRelTypes.set(relTypes);
  // The file and language filters are deliberately absent here. Seeding the
  // two above to "everything present" is what stops a stale entry from a
  // previous dataset hiding nodes in the new one — they are dataset-derived
  // allow-lists and nothing is lost by rebuilding them. The other two are
  // not: they hold a deliberate user exclusion, so they persist in
  // `hiddenFiles` (UI-047) and `hiddenLanguages`, and `visibleFiles` /
  // `generalLanguages` derive from them.
  levelOverrides.update((lo) => {
    [1, 2, 3].forEach((level) => {
      entityTypes.forEach((t) => (lo[level].entityTypes[t] = 'general'));
      relTypes.forEach((t) => (lo[level].relTypes[t] = 'general'));
    });
    return { ...lo };
  });
}

/**
 * Publish a new entity-level graph dataset. `graphData` is now a derived
 * store, so this function only has to update the source of truth
 * (`rawEntityGraph`) — Svelte recomputes the visible graph automatically
 * when either the raw source OR `graphLevel` changes.
 *
 * Filter seeding and selection-clamping live in the `graphData`
 * subscription below so they fire on any change, regardless of whether it
 * came from a scope swap or a level toggle. That removes the entire class
 * of ordering bugs we had when publishGraph and graphLevel handlers each
 * owned part of the reconciliation.
 */
export function publishGraph(data: GraphData): void {
  rawEntityGraph.set(data);
}

// Single reconciliation point for every graph change. Fires whenever
// `rawEntityGraph` or `graphLevel` changes — including the first empty
// value at module load, which is handled by the length guard.
//
// Subscriber order matters: we register this BEFORE GraphView mounts
// (module-level subscribe in a store module), so by the time GraphView's
// own graphData subscriber runs, filter stores are already seeded and the
// selection has been clamped. GraphView's downstream `initGraph` /
// `applyFilters` pass therefore always sees consistent state.
graphData.subscribe((data) => {
  console.log('[filterVM] graphData changed — nodes:', data.nodes.length, 'links:', data.links.length);
  if (data.nodes.length === 0) { console.log('[filterVM] empty graph, skipping seed'); return; }

  seedFromGraph(data);
  const kinds = new Set(data.nodes.map((n) => n.kind_raw));
  console.log('[filterVM] seeded filters — kinds:', [...kinds], 'files:', new Set(data.nodes.map((n) => n.file_path)).size);

  const sel = get(selectedNode);
  if (sel && !data.nodes.some((n) => n.id === sel.id)) {
    // Selection's entity isn't in the new (collapsed or re-scoped) view.
    // Preserve user navigation by mapping it to the containing file or
    // module node when auto-escalation widened the view — so "selected
    // class X" becomes "selected file containing X" rather than disappearing.
    const remapped = remapToContainingScope(sel, data);
    if (remapped) {
      console.log('[filterVM] remapping selection', sel.id, '→', remapped.id, '(' + remapped.kind_raw + ')');
    } else {
      console.log('[filterVM] clamping stale selection:', sel.id);
    }
    selectedNode.set(remapped);
  }
});

/** When the graph escalates (entity → file, file → module) the currently
 *  selected node often disappears. Rather than drop silently, find the
 *  containing scope node in the new dataset so the user stays oriented.
 *  Returns null when no containing node exists (e.g. drilled-in view
 *  narrowed away from the previous selection's file). */
function remapToContainingScope(prev: D3Node, data: GraphData): D3Node | null {
  // Build the candidate paths to look up in the new dataset, in order of
  // specificity (most specific first, so a file-level remap wins over a
  // module-level one when both are available).
  const candidates: string[] = [];

  if (prev.kind_raw !== 'File' && prev.kind_raw !== 'Module') {
    // Entity → might remap to its file or its directory.
    candidates.push(prev.file_path);
    const i = prev.file_path.lastIndexOf('/');
    candidates.push(i >= 0 ? prev.file_path.slice(0, i) : '');
  } else if (prev.kind_raw === 'File') {
    // File → might remap to its parent directory (module view).
    const i = prev.original_id.lastIndexOf('/');
    candidates.push(i >= 0 ? prev.original_id.slice(0, i) : '');
  }
  // Module → nothing coarser to remap to; drop.

  for (const path of candidates) {
    const hit = data.nodes.find((n) => n.original_id === path);
    if (hit) return hit;
  }
  return null;
}

// --- Search state: typing vs. committing ---
//
// Two-phase workflow separates previewing results from filtering the graph:
//
//   1. `searchTerm`: what the user is typing. Drives `searchMatches` (a live
//      list of hits) but does NOT filter the graph on its own.
//   2. `committedSearchIds`: explicit commit — these ids drive the graph
//      filter. Becomes non-empty only via `commitAllMatches()` (Enter) or
//      `toggleCommittedMatch()` (per-row checkbox in the results list).
//
// Clearing the search box empties `committedSearchIds` — the `searchTerm`
// subscription below enforces that so the "clear = undo filter" rule holds
// regardless of which input event cleared the text.
export const searchTerm = writable<string>('');

// --- Search scope: which fields the term is matched against ---
// All default true so behavior matches the pre-scope-option build until
// the user narrows it. If none are on, no results — a deliberate "user
// told us not to match anywhere" state, not a bug.
export const searchInEntityNames = writable<boolean>(true);
export const searchInFileNames = writable<boolean>(true);
export const searchInFolderNames = writable<boolean>(true);
/** When non-empty, only nodes whose kind_raw is in this set can match.
 *  Empty means "all kinds allowed" (default) — users opt in to narrowing. */
export const searchEntityKinds = writable<Set<string>>(new Set());

export function toggleSearchEntityKind(kind: string): void {
  searchEntityKinds.update((s) => {
    const ns = new Set(s);
    if (ns.has(kind)) ns.delete(kind);
    else ns.add(kind);
    return ns;
  });
}
export function clearSearchEntityKinds(): void {
  searchEntityKinds.set(new Set());
}

/**
 * Score a node against the query, or `null` when it does not match.
 *
 * Wires the matcher (`fuzzyPath`) to the field-weighting policy
 * (`entityScore`); the policy lives apart so it can be unit-tested against a
 * stub scorer, and because the Node test runner cannot load a module with
 * extensionless imports.
 *
 * `s` is the raw trimmed term, *not* lowercased — the matcher does
 * smart-case itself, and pre-lowercasing would throw away the signal it
 * needs to tell `Graph` (the type) from `graph` (don't care).
 */
export function scoreSearch(
  n: D3Node,
  s: string,
  inNames: boolean,
  inFiles: boolean,
  inFolders: boolean,
  kinds: Set<string>,
): number | null {
  const terms = parseQuery(s);
  if (terms.length === 0) return null;
  return scoreEntity(
    n,
    { inNames, inFiles, inFolders },
    kinds,
    (c) => scoreQuery(terms, c),
    (c) => violatesNegation(terms, c),
  );
}

/** The single match predicate, shared by the in-scope search here and the
 *  cross-scope search in `entitySearch.ts`, so the two can never drift.
 *
 *  A thin wrapper over `scoreSearch` rather than a second implementation:
 *  the two agreeing is a property the ranked list depends on, and two
 *  parallel bodies would only agree until one of them was edited. */
export function matchesSearch(
  n: D3Node,
  s: string,
  inNames: boolean,
  inFiles: boolean,
  inFolders: boolean,
  kinds: Set<string>,
): boolean {
  return scoreSearch(n, s, inNames, inFiles, inFolders, kinds) !== null;
}

/** Rank order for search results: best score first, then a stable tiebreak
 *  so equal-scoring rows do not reshuffle between keystrokes. */
export function compareByScore(
  a: { score: number; node: D3Node },
  b: { score: number; node: D3Node },
): number {
  return (
    b.score - a.score ||
    a.node.name.localeCompare(b.node.name) ||
    a.node.id.localeCompare(b.node.id)
  );
}

/** In-scope matches, best-first.
 *
 *  Ranking is what makes the cap at the far end honest: the list is sliced
 *  for rendering, and before this the slice kept whichever matches graph
 *  order happened to put first, which on a 200-hit query meant the answer
 *  was routinely not among the fifty rows shown. */
export const scoredSearchMatches: Readable<{ node: D3Node; score: number }[]> = derived(
  [
    graphData,
    searchTerm,
    searchInEntityNames,
    searchInFileNames,
    searchInFolderNames,
    searchEntityKinds,
  ],
  ([$data, $term, $inNames, $inFiles, $inFolders, $kinds]) => {
    const s = $term.trim();
    if (!s) return [];
    const out: { node: D3Node; score: number }[] = [];
    for (const n of $data.nodes) {
      const score = scoreSearch(n, s, $inNames, $inFiles, $inFolders, $kinds);
      if (score === null) continue;
      out.push({ node: n, score });
    }
    return out.sort(compareByScore);
  },
);

export const searchMatches: Readable<D3Node[]> = derived(
  scoredSearchMatches,
  ($m) => $m.map((r) => r.node),
);

/** Committed set — the only thing that actually narrows the graph. */
export const committedSearchIds = writable<Set<string>>(new Set());

/** Legacy name kept as an alias so GraphView (and any future consumer)
 *  can still subscribe without caring about the rename. */
export const searchMatchIds: Readable<Set<string>> = derived(
  committedSearchIds,
  ($ids) => $ids,
);

/** Node ids one hop from a committed match (excluding matches themselves). */
export const searchNeighborIds: Readable<Set<string>> = derived(
  [graphData, committedSearchIds],
  ([$data, $matched]) => {
    const neighbors = new Set<string>();
    if ($matched.size === 0) return neighbors;
    for (const l of $data.links) {
      const src = typeof l.source === 'object' ? l.source.id : l.source;
      const tgt = typeof l.target === 'object' ? l.target.id : l.target;
      if ($matched.has(src) && !$matched.has(tgt)) neighbors.add(tgt);
      else if ($matched.has(tgt) && !$matched.has(src)) neighbors.add(src);
    }
    return neighbors;
  },
);

// Clearing the search text box unconditionally clears the commit. Users
// expect "empty input = no filter", whether they deleted the text or
// pressed Escape or hit a Clear button.
searchTerm.subscribe((term) => {
  if (term.trim() === '') committedSearchIds.set(new Set());
});

/** Commit all current matches (e.g. on Enter in the search input).
 *  Replaces, not adds — pressing Enter after editing the query means "apply
 *  the current result set", not "accumulate with previously checked items". */
export function commitAllMatches(): void {
  committedSearchIds.set(new Set(get(searchMatches).map((n) => n.id)));
}

/** Toggle a single result's membership in the committed set — drives the
 *  per-row checkboxes in the results list. */
export function toggleCommittedMatch(id: string): void {
  committedSearchIds.update((s) => {
    const ns = new Set(s);
    if (ns.has(id)) ns.delete(id);
    else ns.add(id);
    return ns;
  });
}

/** Set a run of ids to one state, for shift-click range selection in the
 *  results list. One store update rather than one per row, so the graph
 *  recomputes once instead of twenty times. */
export function setCommittedMatches(ids: string[], committed: boolean): void {
  committedSearchIds.update((s) => {
    const ns = new Set(s);
    for (const id of ids) {
      if (committed) ns.add(id);
      else ns.delete(id);
    }
    return ns;
  });
}

export function clearCommittedMatches(): void {
  committedSearchIds.set(new Set());
}

// --- Display search: "Ctrl+F within the current view" ---
// Second-level search that matches only among entities *currently on screen*
// (after all filters, entity-search, and selection-distance have been
// applied). It highlights in cyan but does not change visibility — so the
// user can see "where in this view is X" without reshaping the graph.

// All display-search state (term + match derivations) lives in displayPlan.ts
// — keeping it there means filterViewModel has zero imports from
// displayPlan, which keeps the module graph acyclic. Components import
// displaySearchTerm / displaySearchMatches / displaySearchMatchIds directly
// from `./displayPlan`.

// --- Set toggles: centralize the "update a Set<string> in a store" boilerplate ---
function toggleInSet<T>(
  store: { update: (fn: (s: Set<T>) => Set<T>) => void },
  key: T,
  checked: boolean,
): void {
  store.update((s) => {
    const ns = new Set(s);
    if (checked) ns.add(key);
    else ns.delete(key);
    return ns;
  });
}

export const toggleEntityType = (type: string, checked: boolean) =>
  toggleInSet(generalEntityTypes, type, checked);
export const toggleRelType = (type: string, checked: boolean) =>
  toggleInSet(generalRelTypes, type, checked);
/** Checked means visible, so the stored set — the exclusions — moves the
 *  other way. Same inversion as `toggleFile` below, and for the same reason:
 *  the exclusion is the decision that has to outlive a republish. */
export const toggleLanguage = (lang: string, checked: boolean) =>
  toggleInSet(hiddenLanguages, lang, !checked);
export const toggleFile = (path: string, checked: boolean) =>
  toggleInSet(hiddenFiles, path, !checked);

/** Show every language in the current dataset, or none of them. "None" is
 *  sourced from `allLanguages` (what the analyzer actually produced) rather
 *  than a static list, so it can never exclude a language the graph doesn't
 *  have; "all" is simply the empty exclusion set. */
export function setAllLanguages(checked: boolean): void {
  hiddenLanguages.set(checked ? new Set() : new Set(get(allLanguages)));
}

/** "These files, and nothing else." Expressed as the complement, since
 *  exclusions are what persists. */
export function setVisibleFiles(files: Set<string>): void {
  hiddenFiles.set(new Set(get(allFiles).filter((f) => !files.has(f))));
}

/** Un-hide everything. */
export function clearHiddenFiles(): void {
  hiddenFiles.set(new Set());
}

// --- Level override actions ---
export function toggleLevelEnabled(level: number): void {
  levelOverrides.update((lo) => {
    lo[level].enabled = !lo[level].enabled;
    return { ...lo };
  });
}

/** Toggle whether same-level peer edges (N↔N) are drawn for this level. */
export function toggleLevelPeerEdges(level: number): void {
  levelOverrides.update((lo) => {
    lo[level].peerEdges = !lo[level].peerEdges;
    return { ...lo };
  });
}

const triStates: TriState[] = ['general', 'on', 'off'];
function nextTriState(current: TriState): TriState {
  return triStates[(triStates.indexOf(current) + 1) % 3];
}

export function cycleEntityTypeTriState(level: number, key: string): void {
  levelOverrides.update((lo) => {
    lo[level].entityTypes[key] = nextTriState(lo[level].entityTypes[key] || 'general');
    return { ...lo };
  });
}

export function cycleRelTypeTriState(level: number, key: string): void {
  levelOverrides.update((lo) => {
    lo[level].relTypes[key] = nextTriState(lo[level].relTypes[key] || 'general');
    return { ...lo };
  });
}

export function cycleDirectionTriState(level: number, dir: 'outgoing' | 'incoming'): void {
  levelOverrides.update((lo) => {
    lo[level][dir] = nextTriState(lo[level][dir]);
    return { ...lo };
  });
}

/**
 * Convenience derived for views that want a single "is the filter state in
 * sync with the dataset" signal. Not used yet but exposed for future views.
 */
export const filterIsEmpty: Readable<boolean> = derived(
  [generalEntityTypes, visibleFiles],
  ([$et, $vf]) => $et.size === 0 && $vf.size === 0,
);

export function currentFilterSnapshot() {
  return {
    entityTypes: get(generalEntityTypes),
    relTypes: get(generalRelTypes),
    languages: get(generalLanguages),
    files: get(visibleFiles),
    outgoing: get(generalOutgoing),
    incoming: get(generalIncoming),
    levels: get(levelOverrides),
  };
}
