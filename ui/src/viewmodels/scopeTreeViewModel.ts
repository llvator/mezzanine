/**
 * ScopeTreeViewModel — view-state and actions owned on behalf of
 * `ScopeTree.svelte`. The component itself should read these stores and call
 * these actions; it must not own derived state or reach into `stores/scope.ts`
 * directly.
 *
 * What this VM owns:
 *   - Component-local UI state: folder expansion, search text
 *   - Derived `flatList` (depth-first traversal respecting filters)
 *   - Action helpers for language filter and folder toggling
 *
 * What this VM re-exports (pass-through from the Model layer):
 *   - `indexData`, `selectedScopes`, `selectionStats`, `refreshing`
 *   - `treeLanguageFilter`, `availableLanguages`
 *   - `toggleScope`, `clearScope`, `selectAllScope`, `refreshData`
 *   - `ENTITY_THRESHOLD`
 */

import { writable, derived, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import {
  indexData, treeLanguageFilter, fullGraphDataStore,
  setScopes, addScopes, scopeCounts,
} from '../stores/scope';
import { compactRules, includeAll } from '../utils/scopeRules';
import { parseQuery, scoreQuery, violatesNegation } from '../utils/fuzzyPath';

// --- Pass-through re-exports for the view ---
export {
  indexData,
  selectedScopes,
  scopeRules,
  isInScope,
  isDirectRule,
  selectionStats,
  refreshing,
  treeLanguageFilter,
  availableLanguages,
  toggleScope,
  clearScope,
  selectAllScope,
  extendScopeToParents,
  refreshData,
  ENTITY_THRESHOLD,
} from '../stores/scope';

// --- View-owned UI state ---
export const filterText = writable<string>('');
export const openFolders = writable<Set<string>>(new Set(['']));

// --- Actions ---
export function setFilterText(text: string): void {
  filterText.set(text);
}

export function toggleFolderOpen(path: string): void {
  openFolders.update((s) => {
    const ns = new Set(s);
    if (ns.has(path)) ns.delete(path);
    else ns.add(path);
    return ns;
  });
}

export function toggleTreeLanguage(lang: string): void {
  treeLanguageFilter.update((s) => {
    const ns = new Set(s);
    if (ns.has(lang)) ns.delete(lang);
    else ns.add(lang);
    return ns;
  });
}

export function clearLanguageFilter(): void {
  treeLanguageFilter.set(new Set());
}

// --- Pure helpers (no store dependency, safe for inline template use) ---
export function displayName(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx >= 0 ? path.slice(idx + 1) : path;
}

export function formatCount(n: number): string {
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}

function nodeMatchesLanguage(langs: string[] | undefined, filter: Set<string>): boolean {
  if (filter.size === 0) return true;
  if (!langs || langs.length === 0) return false;
  return langs.some((l) => filter.has(l));
}

export interface FlatListItem {
  path: string;
  depth: number;
  isFolder: boolean;
  matches: boolean;
  /** Set only on entity rows — the entity whose declaration put this path
   *  in the results. Absent means the path itself matched. */
  entityName?: string;
}

/** One hit from the query, over either corpus. `path` is what a commit
 *  would put in the scope: the path itself for a path hit, the declaring
 *  file for an entity hit. */
export interface ScopeMatch {
  path: string;
  kind: 'folder' | 'file' | 'entity';
  /** Entity name, when `kind === 'entity'`. */
  name?: string;
  score: number;
}

/** True while the user has typed something into the filter box. Views use
 *  it to switch between the tree and the flat result list, and to show the
 *  whole path per row instead of the basename — two `mod.rs` hits from
 *  different folders are otherwise indistinguishable. */
export const queryActive: Readable<boolean> = derived(
  filterText,
  ($f) => $f.trim().length > 0,
);

/**
 * Entity scores are discounted against path scores.
 *
 * This panel picks a *scope*, so a path that matches is a direct answer and
 * an entity that matches is a hint that leads to one. Undiscounted, a short
 * entity name outscores every path — typing `graph` put an entity called
 * `graph` above `ui/src/stores/graph.ts`, which is not what anyone asking a
 * scope picker for "graph" means. A strong entity hit still beats a weak
 * path hit, which is the case where the entity corpus earns its place.
 */
const ENTITY_SCORE_FACTOR = 0.7;

/**
 * Entities whose "name" is longer than this are not offered as matches.
 *
 * Some parsers leak source text into the name — a Rust enum variant can
 * arrive carrying its whole body, several hundred characters of it (the
 * same class of defect as TS-004 and PY-026). Those names are unreadable in
 * a result row and nobody types them, so they contribute noise and nothing
 * else. The declaring file is still reachable through its path.
 */
const MAX_ENTITY_NAME = 80;

/** Cap on rendered result rows. The graph-wide search caps its list at 50
 *  for the same reason (`MAX_RESULTS_SHOWN` in FilterPanel): a one-letter
 *  query matches thousands of paths, and dropping thousands of rows into
 *  the sidebar costs more than it tells anyone. `queryMatches` stays
 *  complete — only the DOM is capped. */
export const MATCH_ROWS_SHOWN = 200;

/**
 * Everything matching the current query, best first, over two corpora.
 *
 * **Paths** come from the whole `IndexData.nodes` keyset. Expansion state is
 * a display concern and deliberately plays no part: gating the search on
 * `openFolders` meant a fresh session — where only the root is open — could
 * only ever find top-level entries, so the box returned nothing for any
 * query the user could not already answer themselves (UI-042).
 *
 * **Entity names** come from `fullGraphDataStore`, so typing a function name
 * finds the file that declares it. That store is only populated once
 * `ensureFullData()` has run, which the first scope selection triggers — a
 * query typed before then ranks paths alone rather than blocking on a
 * fetch. Degrading is correct here: the index is always present and is the
 * corpus this box is primarily about.
 *
 * Uncapped, because this is also the set that gets committed to the scope.
 */
export const queryMatches: Readable<ScopeMatch[]> = derived(
  [indexData, filterText, treeLanguageFilter, fullGraphDataStore],
  ([$idx, $filter, $langFilter, $full]) => {
    const terms = parseQuery($filter);
    if (!$idx || terms.length === 0) return [] as ScopeMatch[];

    const out: ScopeMatch[] = [];
    for (const [path, node] of Object.entries($idx.nodes)) {
      if (path === '') continue;
      if (!nodeMatchesLanguage(node.languages, $langFilter)) continue;
      const score = scoreQuery(terms, path);
      if (score === null) continue;
      out.push({ path, kind: node.type === 'folder' ? 'folder' : 'file', score });
    }

    // Entity hits are scored on the name alone, not the path: the path was
    // already its own candidate above, and letting the folder characters
    // contribute would rank a badly-named entity in a well-named folder
    // above the thing the user actually typed.
    const pathHits = new Set(out.map((m) => m.path));
    for (const n of $full?.nodes ?? []) {
      if (!n.file_path || n.tags?.includes('ghost')) continue;
      if (n.name.length > MAX_ENTITY_NAME) continue;
      if (!nodeMatchesLanguage([n.language], $langFilter)) continue;
      const score = scoreQuery(terms, n.name);
      if (score === null) continue;
      // Negation applies to the path this row would scope to, not just to
      // the name that matched. `^src !parser` otherwise returned
      // `src/parser/…` on the strength of an entity called `src`.
      if (violatesNegation(terms, n.file_path)) continue;
      // One row per file: a query matching thirty methods of one class says
      // the same thing thirty times, and the commit target is the file
      // either way.
      if (pathHits.has(n.file_path)) continue;
      pathHits.add(n.file_path);
      out.push({
        path: n.file_path,
        kind: 'entity',
        name: n.name,
        score: score * ENTITY_SCORE_FACTOR,
      });
    }

    return out.sort((a, b) => b.score - a.score || a.path.localeCompare(b.path));
  },
);

/** How many matches exist beyond the rendered cap, so the view can say so
 *  rather than silently truncating. */
export const matchOverflow: Readable<number> = derived(
  queryMatches,
  ($m) => Math.max(0, $m.length - MATCH_ROWS_SHOWN),
);

/** What a commit would put in the scope — the whole match set, not the
 *  rendered slice. Entity hits contribute their declaring file, which is
 *  already what `ScopeMatch.path` holds. */
export const queryScopePaths: Readable<string[]> = derived(
  queryMatches,
  ($m) => [...new Set($m.map((m) => m.path))],
);

/**
 * Entity and relationship totals the current query would bring into scope.
 *
 * Shown before Enter is pressed, so a query that would cross
 * `ENTITY_THRESHOLD` is visible as a number rather than as a warning that
 * appears once the graph has already refused to draw.
 */
export const queryProjection: Readable<{ entities: number; relationships: number } | null> = derived(
  [indexData, queryScopePaths],
  ([$idx, $paths]) => {
    if (!$idx || $paths.length === 0) return null;
    return scopeCounts(compactRules(includeAll($paths)), $idx);
  },
);

/**
 * Turn the current match set into the scope — the half of `fzf` that was
 * missing. Until this existed, a query that isolated exactly the twelve
 * files you wanted could only be acted on by clicking twelve checkboxes.
 *
 * `add` unions with the existing selection (Shift+Enter) instead of
 * replacing it. Both go through `setScopes`, so `minimizeSelection`, the
 * entity threshold and auto-level all apply exactly as they do to a click:
 * committing a query is not a way around the oversize guard.
 *
 * An empty match set is a no-op rather than a scope wipe — typing a typo
 * must not silently clear what you had.
 */
export async function commitQuery(add = false): Promise<void> {
  const paths = get(queryScopePaths);
  if (paths.length === 0) return;
  // Both paths land in rule compaction, which drops the enumeration a query
  // produces: matching a folder also matches everything under it, so the raw
  // list holds the folder *and* its twenty files — the same entities, but
  // twenty unrelated-looking choices in the tree and twenty things to undo.
  if (add) await addScopes(paths);
  else await setScopes(paths);
}

// --- Derived: what the tree renders — a flat result list while a query is
// active, the expandable tree otherwise. The query path never touches
// `openFolders`, so clearing the box restores the tree exactly as the user
// left it.
export const flatList: Readable<FlatListItem[]> = derived(
  [indexData, queryActive, queryMatches, openFolders, treeLanguageFilter],
  ([$idx, $querying, $matches, $open, $langFilter]) => {
    if (!$idx) return [] as FlatListItem[];

    if ($querying) {
      return $matches.slice(0, MATCH_ROWS_SHOWN).map((m) => ({
        path: m.path,
        depth: 0,
        isFolder: m.kind === 'folder',
        matches: true,
        entityName: m.name,
      }));
    }

    const result: FlatListItem[] = [];
    const visit = (path: string, depth: number) => {
      const node = $idx.nodes[path];
      if (!node) return;
      if (!nodeMatchesLanguage(node.languages, $langFilter)) return;
      const isFolder = node.type === 'folder';
      if (path !== '' || depth > 0) {
        result.push({ path, depth, isFolder, matches: true });
      }
      if (isFolder && $open.has(path)) {
        for (const c of node.children || []) visit(c, depth + 1);
      }
    };

    const rootNode = $idx.nodes[''];
    if (rootNode?.children) {
      for (const c of rootNode.children) visit(c, 0);
    }
    return result;
  },
);

export type { IndexData } from '../stores/scope';
