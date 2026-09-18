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
 *   - The search box's timing: the match set trails the input, and the
 *     projection trails the match set, so a keystroke costs a render rather
 *     than a walk of the repo. `commitQuery` flushes, so Enter is never
 *     served a stale query.
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
import type { IndexData, IndexNode } from '../stores/scope';
import type { D3Node } from '../types/graph';
import { compactRules, includeAll } from '../utils/scopeRules';
import { parseQuery, scoreQuery, violatesNegation } from '../utils/fuzzyPath';
import { trailing } from '../utils/trailingStore';
import { bestMatches } from '../utils/topMatches';

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

/**
 * How long the match set waits behind the input.
 *
 * Long enough that a typed word costs one search instead of five, short
 * enough to read as instant. The prefixes skipped are the expensive ones:
 * `g` matches the whole repo, `graph` matches a corner of it.
 */
const FILTER_DEBOUNCE_MS = 120;

/**
 * Extra lag on the projection, on top of the above.
 *
 * The `⏎ N entities` hint is the most expensive thing the box computes and
 * the least urgent — it answers "what would Enter cost", which only matters
 * once the reader has stopped to look. Rows land first; the number follows.
 */
const PROJECTION_DEBOUNCE_MS = 200;

/** The query the match set is actually computed from. Clearing the box skips
 *  the wait: an empty query costs nothing to apply and Escape should feel
 *  immediate. */
const debouncedFilter = trailing(filterText, FILTER_DEBOUNCE_MS, (v) => v.trim() === '');

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
 *  different folders are otherwise indistinguishable.
 *
 *  Derived from the debounced query, not the raw input, so the view never
 *  flips to result mode before the results exist — reading it off `filterText`
 *  flashed "No matches" on the first character of every word. */
export const queryActive: Readable<boolean> = derived(
  debouncedFilter,
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
 * The two corpora, folded and flattened once per analysis run.
 *
 * A repo's paths and entity names are fixed between keystrokes, but the
 * scan re-derived them every time: `Object.entries` rebuilt a 65k-pair array,
 * and `toLowerCase` allocated a throwaway string per candidate *per term*.
 * None of that depends on the query, so it is hoisted here and keyed on the
 * identity of the data it came from — a re-analysis swaps the object and the
 * cache misses exactly once, which is the behaviour we want.
 *
 * `WeakMap` rather than a "last seen" pair so an old graph is collectable
 * with its folded copy, and so two corpora can be live at once.
 */
interface PathCorpus {
  paths: string[];
  nodes: IndexNode[];
  /** `paths` lowercased, positionally aligned. */
  lowered: string[];
}

const pathCorpora = new WeakMap<object, PathCorpus>();

function pathCorpus(idx: IndexData): PathCorpus {
  const hit = pathCorpora.get(idx.nodes);
  if (hit) return hit;

  const paths: string[] = [];
  const nodes: IndexNode[] = [];
  const lowered: string[] = [];
  for (const path of Object.keys(idx.nodes)) {
    if (path === '') continue;
    paths.push(path);
    nodes.push(idx.nodes[path]);
    lowered.push(path.toLowerCase());
  }

  const corpus: PathCorpus = { paths, nodes, lowered };
  pathCorpora.set(idx.nodes, corpus);
  return corpus;
}

interface EntityCorpus {
  nodes: D3Node[];
  /** Each node's `name`, lowercased, positionally aligned. */
  lowered: string[];
}

const entityCorpora = new WeakMap<object, EntityCorpus>();

/** Pre-filtered to the entities a query could ever return: the eligibility
 *  rules below don't depend on what was typed, so they belong here rather
 *  than in the scan. The language filter does, and stays there. */
function entityCorpus(nodes: D3Node[]): EntityCorpus {
  const hit = entityCorpora.get(nodes);
  if (hit) return hit;

  const kept: D3Node[] = [];
  const lowered: string[] = [];
  for (const n of nodes) {
    if (!n.file_path || n.tags?.includes('ghost')) continue;
    if (n.name.length > MAX_ENTITY_NAME) continue;
    kept.push(n);
    lowered.push(n.name.toLowerCase());
  }

  const corpus: EntityCorpus = { nodes: kept, lowered };
  entityCorpora.set(nodes, corpus);
  return corpus;
}

/**
 * Everything matching the current query, over two corpora.
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
 * Uncapped, because this is also the set that gets committed to the scope —
 * and **unordered**, because nothing downstream needs the order: the rows
 * take theirs from `rankedMatches`, and `queryScopePaths` collapses this into
 * a set. Sorting tens of thousands of matches to render two hundred of them
 * was work nobody read.
 */
export const queryMatches: Readable<ScopeMatch[]> = derived(
  [indexData, debouncedFilter, treeLanguageFilter, fullGraphDataStore],
  ([$idx, $filter, $langFilter, $full]) => {
    const terms = parseQuery($filter);
    if (!$idx || terms.length === 0) return [] as ScopeMatch[];

    const out: ScopeMatch[] = [];
    const { paths, nodes, lowered } = pathCorpus($idx);
    for (let i = 0; i < paths.length; i++) {
      const node = nodes[i];
      if (!nodeMatchesLanguage(node.languages, $langFilter)) continue;
      const score = scoreQuery(terms, paths[i], lowered[i]);
      if (score === null) continue;
      out.push({ path: paths[i], kind: node.type === 'folder' ? 'folder' : 'file', score });
    }

    // Entity hits are scored on the name alone, not the path: the path was
    // already its own candidate above, and letting the folder characters
    // contribute would rank a badly-named entity in a well-named folder
    // above the thing the user actually typed.
    const pathHits = new Set(out.map((m) => m.path));
    const entities = $full ? entityCorpus($full.nodes) : null;
    for (let i = 0; entities && i < entities.nodes.length; i++) {
      const n = entities.nodes[i];
      if (!nodeMatchesLanguage([n.language], $langFilter)) continue;
      // One row per file: a query matching thirty methods of one class says
      // the same thing thirty times, and the commit target is the file
      // either way. Checked before scoring — it is the cheaper test and it
      // rejects far more.
      if (pathHits.has(n.file_path)) continue;
      const score = scoreQuery(terms, n.name, entities.lowered[i]);
      if (score === null) continue;
      // Negation applies to the path this row would scope to, not just to
      // the name that matched. `^src !parser` otherwise returned
      // `src/parser/…` on the strength of an entity called `src`.
      if (violatesNegation(terms, n.file_path)) continue;
      pathHits.add(n.file_path);
      out.push({
        path: n.file_path,
        kind: 'entity',
        name: n.name,
        score: score * ENTITY_SCORE_FACTOR,
      });
    }

    return out;
  },
);

/** The slice the tree actually draws: best first, capped at the row limit.
 *  Not exported — `flatList` is the shape the view wants, and `matchOverflow`
 *  is the only other thing that needs to know how many rows survived. */
const rankedMatches: Readable<ScopeMatch[]> = derived(
  queryMatches,
  ($m) => bestMatches($m, MATCH_ROWS_SHOWN),
);

/** How many matches exist beyond the rendered cap, so the view can say so
 *  rather than silently truncating. */
export const matchOverflow: Readable<number> = derived(
  [queryMatches, rankedMatches],
  ([$all, $shown]) => Math.max(0, $all.length - $shown.length),
);

/** What a commit would put in the scope — the whole match set, not the
 *  rendered slice. Entity hits contribute their declaring file, which is
 *  already what `ScopeMatch.path` holds. */
export const queryScopePaths: Readable<string[]> = derived(
  queryMatches,
  ($m) => [...new Set($m.map((m) => m.path))],
);

/** The projection's own input, trailing the match set. Emptying is immediate
 *  so the hint disappears with the query rather than outliving it. */
const projectionPaths = trailing(
  queryScopePaths,
  PROJECTION_DEBOUNCE_MS,
  (paths) => paths.length === 0,
);

/**
 * Entity and relationship totals the current query would bring into scope.
 *
 * Shown before Enter is pressed, so a query that would cross
 * `ENTITY_THRESHOLD` is visible as a number rather than as a warning that
 * appears once the graph has already refused to draw.
 *
 * Reads the trailing paths, not the live ones. This is a whole-tree walk over
 * a rule per match, so it is the one derivation worth keeping off the typing
 * path entirely — the rows are the answer, this is the footnote.
 */
export const queryProjection: Readable<{ entities: number; relationships: number } | null> = derived(
  [indexData, projectionPaths],
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
  // Enter can beat the debounce — type a word and hit it in the same breath
  // and the match set is still the one for a prefix of what is on screen.
  // Committing that would scope to something the reader never saw, so the
  // pending query is forced through first. Synchronous, so the `get` below
  // sees it.
  debouncedFilter.flush();
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
  [indexData, queryActive, rankedMatches, openFolders, treeLanguageFilter],
  ([$idx, $querying, $matches, $open, $langFilter]) => {
    if (!$idx) return [] as FlatListItem[];

    if ($querying) {
      return $matches.map((m) => ({
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
