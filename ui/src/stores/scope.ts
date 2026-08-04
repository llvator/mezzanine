import { writable, derived, get } from 'svelte/store';
import type { GraphData, GraphLevel, D3Node, D3Link } from '../types/graph';
import { transformAnalysisJson } from '../transform';
import { publishGraph } from '../viewmodels/filterViewModel';
import { graphLevel, pruneHiddenFiles } from './graph';
import {
  isInScope, compactRules, toggleRule, includeAll, hasExclusionInside,
} from '../utils/scopeRules';
import type { ScopeRule } from '../utils/scopeRules';

export type { ScopeRule } from '../utils/scopeRules';
export { isInScope, isDirectRule } from '../utils/scopeRules';
import { resetDetailsCache } from './details';
import { apiUrl } from '../vscodeAdapter';

/** Soft threshold: selections above this are fully analyzed (Quality,
 *  Summary, Diff, Context, etc. all work) but the graph canvas refuses
 *  to draw them because D3 + DOM gets sluggish past a few thousand
 *  nodes. The UI surfaces a warning and suggests narrowing the scope
 *  or enabling a filter (e.g. "Changes only" during a diff). */
export const ENTITY_THRESHOLD = 2000;
/** Alias kept for clarity at call sites — same value, but the name
 *  emphasises that the limit is a render concern, not an analysis one. */
export const RENDER_THRESHOLD = ENTITY_THRESHOLD;

/** Target node count the d3 simulation + DOM can keep responsive. When a
 *  scope's entity count exceeds this, the graph auto-escalates to file
 *  (and then module) aggregation via `pickLevel`. The user can still drill
 *  in by clicking a collapsed node, which narrows the scope and usually
 *  drops the count back below the budget. */
export const RENDER_BUDGET = 400;

/** When true, `applySelection` overwrites `graphLevel` every publish with
 *  the coarsest level that fits `RENDER_BUDGET`. Toggle off to pin the
 *  level manually (the level toggle buttons set this to false on use). */
export const autoLevel = writable<boolean>(true);

/** Pick the lowest aggregation level that fits `budget`. Counts mirror the
 *  grouping in `collapseGraph`: entity = one per node, file = unique
 *  file_paths, module = unique parent directories.
 *
 *  Ghosts (external/stdlib references, `file_path=""`) are excluded from
 *  the count: they all collapse to a single hidden bucket node whose
 *  visibility is controlled by the ghost toggle, not by the scope, so
 *  letting their count pressure the level toggle would wrongly force the
 *  view to file/module even when only a handful of real entities are in
 *  scope (e.g. single-file drill-in).
 *
 *  Elevator entities (Category / Feature / Functionality / Concept / UI
 *  Page from `.elv` specs) are also excluded. They render much cheaper
 *  than code entities — no metrics layer, fewer edges per node, smaller
 *  per-node payload — so the 400-budget tuned for code analysis is too
 *  tight for them. A real spec easily crosses 400 entities, and forcing
 *  it to file-level collapses the abstraction map: every Feature
 *  becomes a `.elv` file circle, defeating the point of the language.
 *
 *  Ansible-deploy entities are excluded for the same reason: they are
 *  cheap topology nodes (no metrics, few edges), and File-collapsing
 *  them turns every playbook/template/k8s-resource into a bare file
 *  circle — which is exactly the deploy topology the language exists to
 *  reveal. A real deploy repo easily crosses 400 nodes. */
const CHEAP_LANGUAGES = new Set(['Elevator', 'Ansible Deploy']);
export function pickLevel(data: GraphData, budget: number = RENDER_BUDGET): GraphLevel {
  const visible = data.nodes.filter((n) => !n.tags?.includes('ghost'));
  const heavyEntities = visible.filter((n) => !CHEAP_LANGUAGES.has(n.language));
  if (heavyEntities.length <= budget) return 'entity';
  const files = new Set<string>();
  for (const n of visible) {
    files.add(n.file_path);
  }
  if (files.size <= budget) return 'file';
  return 'module';
}

export interface IndexNode {
  path: string;
  type: 'folder' | 'file';
  entity_count: number;
  relationship_count: number;
  /** Distinct languages present in this scope (single value for files, set for folders) */
  languages?: string[];
  children?: string[];
}

export interface IndexData {
  root: string;
  nodes: Record<string, IndexNode>;
  total_entities: number;
  total_relationships: number;
}

// --- Index: loaded immediately at startup ---
export const indexData = writable<IndexData | null>(null);
export const indexLoadError = writable<string | null>(null);

export async function loadIndex(): Promise<void> {
  try {
    const resp = await fetch(apiUrl('/api/index'), { cache: 'no-store' });
    if (!resp.ok) {
      indexLoadError.set(`Failed to load index (HTTP ${resp.status})`);
      return;
    }
    const data = (await resp.json()) as IndexData;
    indexData.set(data);
  } catch (e) {
    indexLoadError.set(`Error loading index: ${e}`);
  }
}

// --- Tree language filter: when non-empty, only show paths matching one of these languages ---
export const treeLanguageFilter = writable<Set<string>>(new Set());

// --- Derived: all languages present in the index ---
export const availableLanguages = derived(indexData, ($idx) => {
  if (!$idx) return [] as string[];
  const root = $idx.nodes[''];
  return root?.languages ? [...root.languages].sort((a, b) => a.localeCompare(b)) : [];
});

// --- Selected scope: an ordered rule list (ADR 0009) ---
//
// The source of truth. Replaces the `Set<string>` of path prefixes, which had
// no way to say "not this": unchecking a row selected via its parent used to
// materialise every sibling on the way down, so one decision became thirty
// enumerated paths that silently re-admitted anything added under the
// excluded branch afterwards.
export const scopeRules = writable<ScopeRule[]>([]);

/** The include patterns, as a set.
 *
 *  Every consumer outside this module reads scope as "what did the user
 *  pick" — `.size > 0`, `.has(path)`, `[...set]` to the extension — and all
 *  of those keep their meaning against the includes. Only the writers moved,
 *  so this stays a read-only view rather than a second model. */
export const selectedScopes = derived(scopeRules, ($rules) =>
  new Set($rules.filter((r) => !r.negate).map((r) => r.pattern)),
);

// --- Analysis scopes: what the Quality / Summary panels aggregate over.
//
// Decoupled from `selectedScopes` so the user can analyze a much wider
// slice than the graph canvas can render. Defaults to the root '' (the
// whole codebase) so Quality works out of the box without requiring a
// second selection step. The user narrows it in the Analysis Scope tree
// view — same folder/file granularity as the visual scope.
export const analysisRules = writable<ScopeRule[]>(includeAll(['']));

/** Read-only include-pattern view, matching `selectedScopes`. */
export const analysisScopes = derived(analysisRules, ($rules) =>
  new Set($rules.filter((r) => !r.negate).map((r) => r.pattern)),
);

/** Replace the analysis scope with a plain path list — the extension's
 *  `setAnalysisScopes` message and the native Analysis Scope tree. */
export function setAnalysisScopes(paths: string[]): void {
  analysisRules.set(compactRules(includeAll(paths)));
}

// --- Full graph data cache (loaded on first scope selection, filtered after) ---
let fullDataPromise: Promise<GraphData> | null = null;
export const graphLoading = writable(false);
export const graphLoadError = writable<string | null>(null);
/** True when current selection exceeds the threshold (graph is not rendered). */
export const scopeOversized = writable(false);

/** The full unfiltered graph, written once per fetch. Exposed as a store
 *  (not just a promise) so derived stores like `analysisGraphData` can
 *  slice the full dataset independently of the visual scope. */
export const fullGraphDataStore = writable<GraphData | null>(null);

async function fetchFullData(): Promise<GraphData> {
  const resp = await fetch(apiUrl('/api/graph'), { cache: 'no-store' });
  if (!resp.ok) throw new Error(`Failed to load graph (HTTP ${resp.status})`);
  const raw = await resp.json();
  const data = transformAnalysisJson(raw);
  fullGraphDataStore.set(data);
  // The only honest moment to garbage-collect file exclusions: this is the
  // whole repo, so a path missing here is gone rather than merely out of
  // scope. Pruning against anything narrower would make hiding a file
  // forgettable by scoping away from it (UI-047).
  pruneHiddenFiles(new Set(data.nodes.map((n) => n.file_path)));
  return data;
}

export function ensureFullData(): Promise<GraphData> {
  if (!fullDataPromise) {
    fullDataPromise = fetchFullData();
  }
  return fullDataPromise;
}

/**
 * Entity and relationship totals for a rule list.
 *
 * Walks the index from the root: a folder with no exclusion inside it
 * contributes its pre-aggregated counts directly, and one with an exclusion
 * is recursed into. The recursion is what the old set-based version could not
 * do — it summed the counts of each minimal covering path, which has no way
 * to subtract anything.
 *
 * Summing only files would be simpler and wrong: `relationship_count` on a
 * file counts intra-file edges only, because cross-file edges are attributed
 * to the lowest common ancestor of their endpoints (see `render_index`). Take
 * folder aggregates where it's safe and the totals stay honest.
 */
export function scopeCounts(
  rules: ScopeRule[],
  idx: IndexData,
): { entities: number; relationships: number } {
  let entities = 0;
  let relationships = 0;

  const visit = (path: string) => {
    const node = idx.nodes[path];
    if (!node) return;
    if (node.type === 'file') {
      if (isInScope(path, rules)) {
        entities += node.entity_count;
        relationships += node.relationship_count;
      }
      return;
    }
    if (isInScope(path, rules) && !hasExclusionInside(path, rules)) {
      entities += node.entity_count;
      relationships += node.relationship_count;
      return;
    }
    for (const child of node.children ?? []) visit(child);
  };

  const root = idx.nodes[''];
  if (!root) return { entities, relationships };
  if (isInScope('', rules) && !hasExclusionInside('', rules)) {
    return { entities: root.entity_count, relationships: root.relationship_count };
  }
  for (const child of root.children ?? []) visit(child);
  return { entities, relationships };
}

/** Filter the full graph to entities/relationships within ANY selected scope.
 *
 *  Ghosts (external/stdlib refs with `file_path=""`, tagged `ghost`) are
 *  kept only when they attach to an in-scope real entity via at least one
 *  link. Without this scope-attachment step, narrowing to one file would
 *  still drag in every ghost referenced anywhere in the repo, defeating the
 *  perf win on large codebases. Both visual and analysis scopes use this
 *  filter — the `showGhosts` / `showBuiltinGhosts` toggles run on top, as a
 *  visualization-only layer that hides what's left. */
function filterToSelection(full: GraphData, rules: ScopeRule[]): GraphData {
  if (rules.length === 0) return { nodes: [], links: [], files: [], modules: [] };
  // Whole-repo fast path: a bare root include with nothing excluded means
  // every node qualifies, so skip the per-node evaluation entirely.
  const matchAll = isInScope('', rules) && !hasExclusionInside('', rules);
  if (matchAll) {
    const nodeIds = new Set(full.nodes.map((n) => n.id));
    const links: D3Link[] = full.links.filter((l) => {
      const srcId = typeof l.source === 'object' ? l.source.id : l.source;
      const tgtId = typeof l.target === 'object' ? l.target.id : l.target;
      return nodeIds.has(srcId) && nodeIds.has(tgtId);
    });
    const allFiles = (full.files ?? []);
    const allModules = (full.modules ?? []);
    return { nodes: full.nodes, links, files: allFiles, modules: allModules };
  }
  // First pass: real (non-ghost) entities under the selected prefixes.
  const realNodes: D3Node[] = full.nodes.filter((n) => {
    if (n.tags?.includes('ghost')) return false;
    return isInScope(n.file_path, rules);
  });
  // Elevator closure pass: a spec is one logical unit whose file split
  // is an editing convenience — cutting containment at a file boundary
  // would silently render a Category without the Features it declares
  // in another `.elv` file (the multi-file spec trap). Pull in the
  // transitive Contains-descendants of in-scope Elevator nodes, plus
  // any Concept attached to what's in scope, regardless of file_path.
  // Descendants only — ancestors stay out, so narrowing to one branch
  // still means something. Elevator-only: code entities keep strict
  // file scoping.
  const byId = new Map(full.nodes.map((n) => [n.id, n]));
  const included = new Set(realNodes.map((n) => n.id));
  const isElevator = (n: D3Node | undefined): n is D3Node =>
    !!n && n.language === 'Elevator' && !n.tags?.includes('ghost');
  const linkEnds = (l: D3Link): [string, string] => [
    typeof l.source === 'object' ? (l.source as D3Node).id : l.source,
    typeof l.target === 'object' ? (l.target as D3Node).id : l.target,
  ];
  let grew = realNodes.some((n) => isElevator(n));
  while (grew) {
    grew = false;
    for (const l of full.links) {
      if (l.kind_raw !== 'Contains') continue;
      const [src, tgt] = linkEnds(l);
      if (!included.has(src) || included.has(tgt)) continue;
      const child = byId.get(tgt);
      if (isElevator(byId.get(src)) && isElevator(child)) {
        included.add(tgt);
        realNodes.push(child);
        grew = true;
      }
    }
  }
  // Concepts cross-cut by design (used_by edges run consumer → concept),
  // so any Concept touching the closed set comes along too.
  for (const l of full.links) {
    const [src, tgt] = linkEnds(l);
    for (const [inId, outId] of [[src, tgt], [tgt, src]] as const) {
      if (!included.has(inId) || included.has(outId)) continue;
      const other = byId.get(outId);
      if (isElevator(byId.get(inId)) && isElevator(other) && other.kind_raw === 'Concept') {
        included.add(outId);
        realNodes.push(other);
      }
    }
  }
  const realIds = new Set(realNodes.map((n) => n.id));
  // Second pass: ghosts attached to at least one in-scope real entity.
  const attachedGhostIds = new Set<string>();
  for (const l of full.links) {
    const srcId = typeof l.source === 'object' ? l.source.id : l.source;
    const tgtId = typeof l.target === 'object' ? l.target.id : l.target;
    if (realIds.has(srcId) && !realIds.has(tgtId)) attachedGhostIds.add(tgtId as string);
    else if (realIds.has(tgtId) && !realIds.has(srcId)) attachedGhostIds.add(srcId as string);
  }
  const ghostNodes = attachedGhostIds.size === 0
    ? []
    : full.nodes.filter((n) => n.tags?.includes('ghost') && attachedGhostIds.has(n.id));
  const nodes = ghostNodes.length === 0 ? realNodes : [...realNodes, ...ghostNodes];
  const nodeIds = new Set(nodes.map((n) => n.id));
  const links: D3Link[] = full.links.filter((l) => {
    const srcId = typeof l.source === 'object' ? l.source.id : l.source;
    const tgtId = typeof l.target === 'object' ? l.target.id : l.target;
    return nodeIds.has(srcId) && nodeIds.has(tgtId);
  });
  // Scope-filter the file/module rollups using the same predicate. Keeps
  // the Quality "Files/Modules" tabs honest — they reflect only what's in
  // the currently visualised scope.
  // Rollups are kept when the path is in scope *or* is an ancestor of
  // something that is — a module row for `ui/src` stays meaningful when only
  // `ui/src/stores` was selected. `isInScope` alone answers only the first
  // half, so the include patterns are still consulted for the second.
  const includes = rules.filter((r) => !r.negate).map((r) => r.pattern);
  const inScope = (path: string) =>
    isInScope(path, rules) || includes.some((p) => p.startsWith(path + '/'));
  const files = (full.files ?? []).filter((f) => inScope(f.path));
  const modules = (full.modules ?? []).filter((m) => inScope(m.path));
  return { nodes, links, files, modules };
}

/** Apply the current selection: aggregate counts and publish the scoped
 *  graph. The graph is always published so downstream analysis views
 *  (Quality, Summary, Diff, Context) see the user's full scope. If the
 *  scope exceeds the render threshold, `scopeOversized` is set so the
 *  graph canvas can decline to render while the side panels still work.
 *
 *  `force` used to bypass the block entirely; kept for API compatibility
 *  but now only affects the warning flag (analysis runs either way). */
export async function applySelection(opts: { force?: boolean } = {}): Promise<void> {
  const idx = get(indexData);
  if (!idx) return;
  const rules = get(scopeRules);

  if (rules.length === 0) {
    scopeOversized.set(false);
    publishGraph({ nodes: [], links: [] });
    return;
  }

  const { entities } = scopeCounts(rules, idx);

  // Analysis-layer cap (2k entities): beyond this we still run every
  // analysis pass but refuse to even collapse-and-render, because the full
  // dataset is too large to hold efficiently in the browser. Below this
  // we always render — `pickLevel` picks an aggregation level that keeps
  // the d3 simulation responsive.
  scopeOversized.set(!opts.force && entities > ENTITY_THRESHOLD);
  graphLoading.set(true);
  graphLoadError.set(null);

  try {
    const full = await ensureFullData();
    const scoped = filterToSelection(full, rules);
    // Auto-pick the aggregation level so the rendered node count stays
    // under RENDER_BUDGET. Users can pin the level via the level toggle,
    // which flips `autoLevel` off.
    const auto = get(autoLevel);
    const picked = pickLevel(scoped);
    const visibleCount = scoped.nodes.filter((n) => !n.tags?.includes('ghost')).length;
    console.log(
      `[auto-level] scoped.nodes=${scoped.nodes.length} (visible=${visibleCount} ghosts=${scoped.nodes.length - visibleCount}) autoLevel=${auto} picked=${picked} (budget=${RENDER_BUDGET}) — currentLevel=${get(graphLevel)}`
    );
    if (auto) {
      graphLevel.set(picked);
    }
    // `publishGraph` seeds filter stores then sets graphData in one step,
    // so GraphView's subscription sees consistent state. This is the
    // MVVM seam: scope selection (Model) hands data to the filter VM,
    // which owns the sync between dataset and filter state.
    publishGraph(scoped);
  } catch (e) {
    graphLoadError.set(`${e}`);
    publishGraph({ nodes: [], links: [] });
  } finally {
    graphLoading.set(false);
  }
}

/**
 * Flip one path in or out of scope and re-render.
 *
 * One appended rule, whichever direction the click goes. The set-based
 * version had to `materializeExclusion` here — drop the covering ancestor and
 * re-add every sibling at every level down to `path` — because a set of
 * includes cannot say "not this". That turned one decision into thirty
 * entries and let anything added under the excluded branch back in later.
 */
export function toggleScope(path: string): void {
  scopeRules.update((rules) => toggleRule(rules, path));
  applySelection();
}

/** Clear all selections. */
export function clearScope(): void {
  scopeRules.set([]);
  scopeOversized.set(false);
  publishGraph({ nodes: [], links: [] });
}

/** Widen the scope: replace each selected path with its own parent folder
 *  (root stays root), collapsing overlaps to the minimal covering set.
 *  Pairs with "Visualize Current File" — start from a single file and grow
 *  the scope one level per click. */
export function extendScopeToParents(): void {
  const sel = get(selectedScopes);
  if (sel.size === 0) return;
  const parents = new Set<string>();
  for (const p of sel) {
    const idx = p.lastIndexOf('/');
    parents.add(idx >= 0 ? p.slice(0, idx) : '');
  }
  scopeRules.set(compactRules(includeAll(parents)));
  applySelection();
}

/** Select every path in the index (equivalent to selecting the root scope). */
export function selectAllScope(): void {
  scopeRules.set(includeAll(['']));
  applySelection();
}

/**
 * Replace the current scope selection with an arbitrary set of paths and
 * re-render. Used by the VS Code native scope TreeView.
 *
 * `force` skips the entity-threshold safety check — the caller is promising
 * it handled the "too many entities" concern some other way (e.g. by
 * enabling a filter that hides most of them).
 */
export async function setScopes(paths: string[], opts: { force?: boolean } = {}): Promise<void> {
  const idx = get(indexData);
  if (!idx) {
    // Index not loaded yet — remember the request and apply once it arrives.
    // A simple retry loop is fine here since the index loads quickly.
    for (let i = 0; i < 20 && !get(indexData); i++) {
      await new Promise((r) => setTimeout(r, 250));
    }
    if (!get(indexData)) return;
  }
  scopeRules.set(compactRules(includeAll(paths)));
  await applySelection({ force: opts.force });
}

/**
 * Widen the scope to also cover `paths`, keeping whatever is already there.
 *
 * Appended as includes, so an earlier exclusion that these paths fall under
 * is overridden for them and left standing for everything else — which is
 * what "add this to what I'm looking at" has to mean once exclusions exist.
 */
export async function addScopes(paths: string[]): Promise<void> {
  scopeRules.update((rules) => compactRules([...rules, ...includeAll(paths)]));
  await applySelection();
}

/** Drill into a scope: narrow selection AND re-enable auto-level so the
 *  view expands to the finest level the new (smaller) scope allows.
 *  Used by the double-click / "Drill in" button — always expresses the
 *  intent "show me as much detail as fits", overriding any prior manual
 *  level pin. */
export async function drillIn(path: string): Promise<void> {
  console.log(`[drill] drillIn() called path=${path} — resetting autoLevel=true and calling setScopes([${path}])`);
  autoLevel.set(true);
  await setScopes([path]);
  console.log(`[drill] drillIn() completed path=${path}`);
}

/**
 * Focus the visualization on a specific path (file or folder).
 *
 * Used by the VS Code extension when the user requests "Visualize Current File":
 * we set that path as the sole selected scope and re-render. If the path isn't
 * in the index yet (index still loading or file not analysed), this waits up
 * to `timeoutMs` for it to appear. Returns true if the scope was applied.
 */
export async function focusScope(path: string, timeoutMs = 5000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const idx = get(indexData);
    if (idx && idx.nodes[path]) {
      scopeRules.set(includeAll([path]));
      await applySelection();
      return true;
    }
    // Index not ready or path not present yet — wait briefly and retry
    await new Promise((r) => setTimeout(r, 200));
  }
  return false;
}

// --- Refresh: re-fetch all data files from the server ---
export const refreshing = writable(false);

/** Clear all in-memory caches, re-fetch the index and (if there was a selection)
 * re-apply it so the graph reflects freshly generated data.
 *
 * During a live-reload cycle, the old graph stays on screen while the new
 * data is being fetched — `publishGraph` fires once with the new data so
 * GraphView's incremental update path can diff the surviving nodes and
 * animate changes instead of tearing down the entire SVG. */
export async function refreshData(): Promise<void> {
  refreshing.set(true);
  try {
    // Bump cache-busting token and invalidate in-memory caches so fresh fetches happen
    fullDataPromise = null;
    resetDetailsCache();

    // Preserve the user's selection. Do NOT publish an empty graph here —
    // that would tear down the DOM and force a full rebuild when the new
    // data arrives. Instead, keep the current graph on screen; the
    // subsequent `applySelection` will publish the new data in one shot,
    // letting GraphView's incremental update path kick in.
    const previousSelection = get(selectedScopes);

    // Reload the index
    await loadIndex();

    // Re-apply the selection against the new index/data
    if (previousSelection.size > 0) {
      await applySelection();
    }
  } finally {
    refreshing.set(false);
  }
}

const NO_STATS = { entities: 0, relationships: 0 };

// --- Derived: aggregate of current selection ---
export const selectionStats = derived(
  [indexData, scopeRules],
  ([$idx, $rules]) => {
    if (!$idx || $rules.length === 0) return NO_STATS;
    return scopeCounts($rules, $idx);
  },
);

/** Same as selectionStats but for the analysis scope — used by the
 *  "Analysis Scope" native tree view to show per-folder selected counts
 *  and by the Quality view's header to show how big the analysis slice is. */
export const analysisStats = derived(
  [indexData, analysisRules],
  ([$idx, $rules]) => {
    if (!$idx || $rules.length === 0) return NO_STATS;
    return scopeCounts($rules, $idx);
  },
);

/** Full graph sliced by the analysis scope. Independent of `graphData`
 *  so Quality / Summary can see entities outside the visual scope. */
export const analysisGraphData = derived(
  [fullGraphDataStore, analysisRules],
  ([$full, $rules]): GraphData => {
    const EMPTY: GraphData = { nodes: [], links: [], files: [], modules: [] };
    if (!$full || $rules.length === 0) return EMPTY;
    return filterToSelection($full, $rules);
  },
);

// ─────────────────────────────────────────────────────────────────────────────
// Root path management
// ─────────────────────────────────────────────────────────────────────────────

export interface RootPathResponse {
  path: string;
  success: boolean;
  message?: string;
  entity_count?: number;
  relationship_count?: number;
}

/** Current analyzed root path. */
export const rootPath = writable<string>('');

/** Whether we're fetching/changing the root path. */
export const rootPathLoading = writable<boolean>(false);

/** Error message from root path operations. */
export const rootPathError = writable<string | null>(null);

/** Fetch the current root path from the server. */
export async function fetchRootPath(): Promise<void> {
  rootPathLoading.set(true);
  rootPathError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/root'));
    if (!resp.ok) {
      throw new Error(`HTTP ${resp.status}`);
    }
    const data: RootPathResponse = await resp.json();
    rootPath.set(data.path);
  } catch (err) {
    rootPathError.set(`Failed to fetch root path: ${err}`);
  } finally {
    rootPathLoading.set(false);
  }
}

/** Change the root path and trigger re-analysis. */
export async function setRootPath(newPath: string): Promise<RootPathResponse> {
  rootPathLoading.set(true);
  rootPathError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/root'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: newPath }),
    });
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    const data: RootPathResponse = await resp.json();
    if (data.success) {
      rootPath.set(data.path);
      // Clear selection and refresh data since the codebase changed
      scopeRules.set([]);
      fullDataPromise = null;
      resetDetailsCache();
      await loadIndex();
    } else {
      rootPathError.set(data.message || 'Unknown error');
    }
    return data;
  } catch (err) {
    const msg = `Failed to set root path: ${err}`;
    rootPathError.set(msg);
    return { path: newPath, success: false, message: msg };
  } finally {
    rootPathLoading.set(false);
  }
}
