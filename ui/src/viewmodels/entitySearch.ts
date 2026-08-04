/**
 * Cross-scope entity search.
 *
 * Lives outside `filterViewModel` because it needs `fullGraphDataStore` from
 * `stores/scope`, and `stores/scope` already imports `filterViewModel` —
 * importing back would close a cycle, and the top-level `derived(...)` calls
 * make cycle-time evaluation order load-bearing rather than merely untidy.
 */

import { derived } from 'svelte/store';
import type { Readable } from 'svelte/store';
import type { D3Node } from '../types/graph';
import { graphData } from '../stores/graph';
import { fullGraphDataStore } from '../stores/scope';
import {
  matchesSearch,
  searchTerm,
  searchInEntityNames,
  searchInFileNames,
  searchInFolderNames,
  searchEntityKinds,
} from './filterViewModel';

/**
 * Matches that exist in the repo but not in the currently loaded scope.
 *
 * `searchMatches` deliberately stays scope-bound — it feeds the commit
 * mechanism, which can only filter nodes the graph actually holds. But
 * searching with a narrow scope (or none at all) and silently getting nothing
 * was the trap UI-016 exists to remove: in a 4000-entity repo with one folder
 * scoped, most searches land here. These are surfaced as a group the user can
 * scope into, rather than as absence.
 */
export const searchOutOfScopeMatches: Readable<D3Node[]> = derived(
  [
    fullGraphDataStore,
    graphData,
    searchTerm,
    searchInEntityNames,
    searchInFileNames,
    searchInFolderNames,
    searchEntityKinds,
  ],
  ([$full, $data, $term, $inNames, $inFiles, $inFolders, $kinds]) => {
    const s = $term.trim();
    if (!s || !$full) return [] as D3Node[];
    const inScope = new Set($data.nodes.map((n) => n.id));
    return $full.nodes.filter(
      (n) => !inScope.has(n.id) && matchesSearch(n, s, $inNames, $inFiles, $inFolders, $kinds),
    );
  },
);
