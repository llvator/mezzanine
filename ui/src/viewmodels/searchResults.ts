/**
 * One ranked result list for the entity search, across all three tiers.
 *
 * Before this, a query produced two disjoint lists — in-scope hits in graph
 * order, and a separate "outside the current scope" group — and a third
 * tier existed without ever being named: hits that *are* in the loaded scope
 * but are not on the canvas, because a kind toggle, a language chip, a
 * hidden file, the selection depth or the draw ceiling removed them. Those
 * rows were indistinguishable from drawn ones. You could check one, commit
 * it, and watch nothing happen.
 *
 * So the list here is a single ranked sequence — ranking across tiers is the
 * point, otherwise a strong match in tier 2 sorts below a weak one in tier 1
 * — with each row carrying why it is not visible, if it isn't, and enough
 * information for the caller to reverse that.
 *
 * Lives beside `entitySearch.ts` for the same reason that module does:
 * it needs `fullGraphDataStore` from `stores/scope`, and `stores/scope`
 * already imports `filterViewModel`. Nothing imports *this* module except
 * components, which keeps the store graph acyclic.
 */

import { derived, get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import type { D3Node } from '../types/graph';
import {
  rawEntityGraph,
  graphLevel,
  selectedNode,
  generalEntityTypes,
  generalLanguages,
  visibleFiles,
  showGhostNodes,
  showBuiltinGhosts,
  showTemplateVars,
} from '../stores/graph';
import { fullGraphDataStore, addScopes, autoLevel } from '../stores/scope';
import {
  scoredSearchMatches,
  scoreSearch,
  compareByScore,
  searchTerm,
  searchInEntityNames,
  searchInFileNames,
  searchInFolderNames,
  searchEntityKinds,
  toggleEntityType,
  toggleLanguage,
  toggleFile,
} from './filterViewModel';
import { displayPlan } from './displayPlan';
import { splitViewOpen } from '../stores/panes';
import { crossFilterPaths, clearSpecFocus, specSelection } from '../stores/crossFilter';
import {
  classifyBlock,
  COLLAPSED_BLOCK,
  DISTANCE_BLOCK,
  CEILING_BLOCK,
  type BlockReason,
  type FilterSnapshot,
} from '../utils/blockReason';

/** How many rows the list renders. The store itself is uncapped, because
 *  "commit all" and the bulk unblock act on the whole match set. */
export const MAX_RESULTS_SHOWN = 50;

export interface SearchResult {
  node: D3Node;
  score: number;
  /** False for hits that exist in the repo but not in the loaded scope —
   *  they have no id in `graphData` and cannot be committed until scoped. */
  inScope: boolean;
  /** Why the node is not on the canvas, or `null` when it is drawn.
   *  Always `null` for out-of-scope rows: being unloaded is the reason, and
   *  `inScope` already says so. */
  blocked: BlockReason | null;
}

/**
 * Ranked results across every tier.
 *
 * The blocked classification runs only over in-scope hits, and only over
 * the rendered head of the list plus whatever the bulk actions need — but
 * it is cheap (a handful of set lookups per node) and running it over the
 * whole match set keeps the counts in the status row honest, which is the
 * thing the user is deciding from.
 */
export const searchResults: Readable<SearchResult[]> = derived(
  [
    scoredSearchMatches,
    fullGraphDataStore,
    rawEntityGraph,
    displayPlan,
    selectedNode,
    generalEntityTypes,
    generalLanguages,
    visibleFiles,
    showGhostNodes,
    showBuiltinGhosts,
    showTemplateVars,
    searchTerm,
    searchInEntityNames,
    searchInFileNames,
    searchInFolderNames,
    searchEntityKinds,
    splitViewOpen,
    crossFilterPaths,
  ],
  ([
    $inScope, $full, $raw, $plan, $selected,
    $kinds, $langs, $files, $ghosts, $builtinGhosts, $templateVars,
    $term, $inNames, $inFiles, $inFolders, $searchKinds,
    $splitView, $crossPaths,
  ]) => {
    const s = $term.trim();
    if (!s) return [] as SearchResult[];

    const snap: FilterSnapshot = {
      kinds: $kinds,
      langs: $langs,
      files: $files,
      showGhosts: $ghosts,
      showBuiltinGhosts: $builtinGhosts,
      showTemplateVars: $templateVars,
      splitView: $splitView,
      crossFilterPaths: $crossPaths,
    };

    // The overflow card replaces the canvas wholesale, and every visibility
    // set on the plan is empty when it is showing. Attributing that to each
    // node's own filters would put a wrong badge on every row, so the
    // ceiling is reported once, for all of them.
    const overflowed = $plan.overflow !== null;
    // A selection restricts the drawn set to its BFS reach. That is the only
    // narrowing left once the hard filters have been checked, so it is what
    // an otherwise-unexplained absence means.
    const selectionActive = $selected !== null && !overflowed;

    // Three passes, one per tier, over three progressively wider corpora:
    // what the canvas draws, what the analysis scope loaded, and the repo.
    // Each node is claimed by the narrowest corpus that has it, so a hit is
    // reported at the tier it actually sits in.
    const out: SearchResult[] = [];

    // 1. Drawable — nodes `graphData` holds, at whatever granularity the
    //    canvas is using. Blocked here means a filter, the selection depth
    //    or the ceiling took it.
    const drawable = new Set<string>();
    for (const { node, score } of $inScope) {
      drawable.add(node.id);
      let blocked: BlockReason | null = null;
      if (overflowed) {
        blocked = CEILING_BLOCK;
      } else if (!$plan.visibleNodeIds.has(node.id)) {
        blocked = classifyBlock(node, snap);
        // Not held back by any hard filter, yet absent. Either the selection
        // depth excluded it, or this search's own commit did — and the
        // latter is the user's own doing, so it earns no badge.
        if (!blocked && selectionActive) blocked = DISTANCE_BLOCK;
      }
      out.push({ node, score, inScope: true, blocked });
    }

    // 2. Loaded but folded away. `rawEntityGraph` is the analysis scope at
    //    entity granularity; `graphData` is that same data collapsed to the
    //    current level. At File level the difference is every entity in the
    //    scope — which used to be reported as "out of scope", the opposite
    //    of the truth.
    const loaded = new Set<string>();
    for (const n of $raw.nodes) {
      loaded.add(n.id);
      if (drawable.has(n.id)) continue;
      const score = scoreSearch(n, s, $inNames, $inFiles, $inFolders, $searchKinds);
      if (score === null) continue;
      out.push({
        node: n,
        score,
        inScope: true,
        blocked: overflowed ? CEILING_BLOCK : COLLAPSED_BLOCK,
      });
    }

    // 3. In the repo, never loaded.
    if ($full) {
      for (const n of $full.nodes) {
        if (loaded.has(n.id) || drawable.has(n.id)) continue;
        const score = scoreSearch(n, s, $inNames, $inFiles, $inFolders, $searchKinds);
        if (score === null) continue;
        out.push({ node: n, score, inScope: false, blocked: null });
      }
    }

    return out.sort(compareByScore);
  },
);

/** The rendered head of the list. */
export const visibleSearchResults: Readable<SearchResult[]> = derived(
  searchResults,
  ($r) => $r.slice(0, MAX_RESULTS_SHOWN),
);

/** Counts for the status row, so the user can see the shape of the result
 *  set before deciding what to do with it. */
export const searchResultCounts: Readable<{
  total: number;
  drawn: number;
  blocked: number;
  outOfScope: number;
  hidden: number;
}> = derived(searchResults, ($r) => {
  let drawn = 0, blocked = 0, outOfScope = 0;
  for (const r of $r) {
    if (!r.inScope) outOfScope++;
    else if (r.blocked) blocked++;
    else drawn++;
  }
  return { total: $r.length, drawn, blocked, outOfScope, hidden: blocked + outOfScope };
});

// --- Reversing a block ---
//
// Decision: a row may undo the filter that is hiding it, per-row, and the
// undo is itself undoable. Relaxing a filter silently would be worse than
// the problem it solves — the user set that filter deliberately, and a
// search should not be able to quietly widen the view and leave them
// wondering why unrelated nodes came back.

export interface Relaxation {
  label: string;
  /** Puts the filter back exactly as it was. */
  undo: () => void;
}

/** Filters this search has relaxed, most recent last. The status row renders
 *  these so the widening is visible and reversible. */
export const relaxedFilters = writable<Relaxation[]>([]);

function record(label: string, undo: () => void): void {
  relaxedFilters.update((r) => [...r, { label, undo }]);
}

/**
 * Re-admit whatever `reason` says is blocking the node.
 *
 * Returns false when the reason cannot be reversed from here — the draw
 * ceiling, where the fix is to narrow the view rather than widen it, and
 * widening is exactly what would make it worse.
 */
export function unblock(reason: BlockReason): boolean {
  switch (reason.kind) {
    case 'ghost':
      showGhostNodes.set(true);
      record('ghosts shown', () => showGhostNodes.set(false));
      return true;
    case 'builtin-ghost':
      showBuiltinGhosts.set(true);
      record('builtin ghosts shown', () => showBuiltinGhosts.set(false));
      return true;
    case 'template-var':
      showTemplateVars.set(true);
      record('template vars shown', () => showTemplateVars.set(false));
      return true;
    case 'kind':
      toggleEntityType(reason.value, true);
      record(`${reason.value} shown`, () => toggleEntityType(reason.value, false));
      return true;
    case 'language':
      toggleLanguage(reason.value, true);
      record(`${reason.value} shown`, () => toggleLanguage(reason.value, false));
      return true;
    case 'file':
      toggleFile(reason.value, true);
      record(`${reason.value.split('/').pop()} shown`, () => toggleFile(reason.value, false));
      return true;
    case 'collapsed': {
      // Draw the same scope one level finer. Not a scope change — the data
      // is already loaded, it is only being folded into file nodes.
      const previous = get(graphLevel);
      if (previous === 'entity') return false;
      // Pinning `autoLevel` off is what makes the change survive. The level
      // is re-picked from the render budget on every `applySelection`, so a
      // scope this size — chosen *because* it exceeded the budget — is
      // collapsed straight back and the click looks like it did nothing.
      // Every other manual level setter pins it for the same reason.
      const previousAuto = get(autoLevel);
      autoLevel.set(false);
      graphLevel.set('entity');
      record(`${previous} level expanded`, () => {
        graphLevel.set(previous);
        autoLevel.set(previousAuto);
      });
      return true;
    }
    case 'distance': {
      // Clearing the selection is what widens the BFS reach back to the
      // whole filtered set. Restoring it is a plain set, so the undo is
      // exact rather than approximate.
      const previous = get(selectedNode);
      selectedNode.set(null);
      record('selection cleared', () => selectedNode.set(previous));
      return true;
    }
    case 'cross-filter': {
      // Drop the spec selection. Exact undo for the same reason `distance`
      // has one: the selection is a plain set, so putting it back restores
      // the filter as it was.
      const previous = get(specSelection);
      clearSpecFocus();
      record('spec filter cleared', () => specSelection.set(previous));
      return true;
    }
    // Not a block to undo — the node is drawn, in the other pane. Widening
    // anything here would be answering a question nobody asked.
    case 'spec-layer':
      return false;
    case 'ceiling':
      return false;
  }
}

/** Re-admit every distinct reversible block in the current result set.
 *
 * Deduplicated by kind+value: twenty results hidden by one kind toggle are
 * one relaxation, not twenty identical entries in the undo list. */
export function unblockAll(): number {
  const seen = new Set<string>();
  let count = 0;
  for (const r of get(searchResults)) {
    if (!r.blocked || !r.blocked.reversible) continue;
    const key = `${r.blocked.kind}:${r.blocked.value}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (unblock(r.blocked)) count++;
  }
  return count;
}

/**
 * Bring out-of-scope hits into the analysis scope, in one pass.
 *
 * `focusScope` — what the single-row action has always used — *replaces* the
 * scope and polls the index for up to five seconds waiting for the path to
 * appear. Calling it once per row would replace the scope N times and take N
 * timeouts to do it, so selecting three out-of-scope results would leave
 * only the third. `addScopes` unions the whole set in a single commit and
 * goes through the same rule compaction and oversize guard as a click in the
 * scope tree: bulk-adding is not a way around the threshold.
 *
 * Returns the file paths added, which the caller needs in order to re-find
 * the entities once the new scope has loaded.
 */
export async function scopeToResults(nodes: D3Node[]): Promise<string[]> {
  const paths = [...new Set(nodes.map((n) => n.file_path).filter(Boolean))];
  if (paths.length === 0) return [];
  await addScopes(paths);
  return paths;
}

/** Put every relaxed filter back, newest first so nested changes unwind in
 *  the order they were made. */
export function undoRelaxations(): void {
  const current = get(relaxedFilters);
  for (let i = current.length - 1; i >= 0; i--) current[i].undo();
  relaxedFilters.set([]);
}

// A new query is a new question, and the relaxations recorded against the
// previous one are no longer explicable from what is on screen. Drop the
// record rather than let it accumulate across unrelated searches — the
// filters themselves stay relaxed, which is why the entries are cleared
// only when the box is emptied.
searchTerm.subscribe((term) => {
  if (term.trim() === '') relaxedFilters.set([]);
});
