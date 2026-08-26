import { writable, derived, get } from 'svelte/store';
import type { GraphData, GraphLevel, D3Node, D3Link } from '../types/graph';
import { transformAnalysisJson } from '../transform';
import { publishGraph } from '../viewmodels/filterViewModel';
import { graphLevel, pruneHiddenFiles, expandedScopes } from './graph';
import {
  isInScope, compactRules, toggleRule, includeAll, hasExclusionInside,
} from '../utils/scopeRules';
import type { ScopeRule } from '../utils/scopeRules';
import { livePaths } from '../viewmodels/markSet';
import { noteLinkNeighbours } from '../viewmodels/noteScope';
import { clearMarks, markedPaths, pruneMarks } from './marks';

export type { ScopeRule } from '../utils/scopeRules';
export { isInScope, isDirectRule } from '../utils/scopeRules';
import { resetDetailsCache } from './details';
import { fetchBranch } from './branch';
import { apiUrl } from '../vscodeAdapter';

/** Selection size at which the scope tree starts flagging rows as large.
 *
 *  No longer gates the canvas. Until UI-061 this was the render gate, and
 *  it decided from the wrong quantity: the index's entity total for the
 *  selected paths, evaluated before collapse and before any filter ran.
 *  What the canvas costs is the number of nodes that reach the DOM, which
 *  is what `DRAW_CEILING` in `viewmodels/drawCeiling.ts` now measures.
 *
 *  What survives here is advisory — a hint in the scope tree that a
 *  selection is big. Whether a scope-level ceiling should exist at all,
 *  and what it would guard now that it no longer guards drawing, is
 *  UI-063. */
export const ENTITY_THRESHOLD = 2000;

/** Target node count the d3 simulation + DOM can keep responsive. When a
 *  scope's entity count exceeds this, the graph auto-escalates to file
 *  (and then folder) aggregation via `pickLevel`. The user can still drill
 *  in by clicking a collapsed node, which narrows the scope and usually
 *  drops the count back below the budget. */
export const RENDER_BUDGET = 400;

/** When true, `applySelection` overwrites `graphLevel` every publish with
 *  the coarsest level that fits `RENDER_BUDGET`. Toggle off to pin the
 *  level manually (the level toggle buttons set this to false on use). */
export const autoLevel = writable<boolean>(true);

/** Pick the lowest aggregation level that fits `budget`. Counts mirror the
 *  grouping in `collapseGraph`: entity = one per node, file = unique
 *  file_paths, folder = unique parent directories.
 *
 *  Ghosts (external/stdlib references, `file_path=""`) are excluded from
 *  the count: they all collapse to a single hidden bucket node whose
 *  visibility is controlled by the ghost toggle, not by the scope, so
 *  letting their count pressure the level toggle would wrongly force the
 *  view to file/folder even when only a handful of real entities are in
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
export function pickLevel(
  data: GraphData,
  budget: number = RENDER_BUDGET,
  expanded: ReadonlySet<string> = new Set<string>(),
): GraphLevel {
  const visible = data.nodes.filter((n) => !n.tags?.includes('ghost'));
  const heavyEntities = visible.filter((n) => !CHEAP_LANGUAGES.has(n.language));
  if (heavyEntities.length <= budget) return 'entity';

  // UI-057: count what would actually be *drawn*, not what the level names.
  // An expanded scope contributes its members instead of one circle, and a
  // level chosen without knowing that would pick File, see the expansion push
  // the count over budget, escalate to Folder, and take the user's expansion
  // with it — auto-level and the reader fighting each other one click apart.
  const atFile = new Set<string>();
  for (const n of visible) atFile.add(expanded.has(n.file_path) ? n.id : n.file_path);
  if (atFile.size <= budget) return 'file';

  // Folder is the coarsest level there is, so it is returned whether or not
  // it fits — the draw ceiling downstream is what refuses an impossible view.
  return 'folder';
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

/** Drop expansions and marks for scopes that are no longer in the graph.
 *
 *  One walk for both, because both hold paths for the same reason and would
 *  otherwise go stale in the same way: a scope the reader can no longer see
 *  is invisible state, and an expansion would silently re-open — or a mark
 *  silently widen a later drill — the moment they scoped back. */
function pruneScopeState(scoped: GraphData): void {
  const live = livePaths(scoped.nodes);
  expandedScopes.update((s) => {
    const next = new Set([...s].filter((p) => live.has(p)));
    return next.size === s.size ? s : next;
  });
  pruneMarks(live);
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
  if (rules.length === 0) return { nodes: [], links: [], files: [], folders: [] };
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
    const allFolders = (full.folders ?? []);
    return { nodes: full.nodes, links, files: allFiles, folders: allFolders };
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
  // Markdown closure: a note IS its file, so file scoping cuts every link it
  // has. One hop, notes only — see `viewmodels/noteScope.ts`.
  for (const note of noteLinkNeighbours(full, included)) {
    included.add(note.id);
    realNodes.push(note);
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
  // Scope-filter the file/folder rollups using the same predicate. Keeps
  // the Quality "Files/Folders" tabs honest — they reflect only what's in
  // the currently visualised scope.
  // Rollups are kept when the path is in scope *or* is an ancestor of
  // something that is — a folder row for `ui/src` stays meaningful when only
  // `ui/src/stores` was selected. `isInScope` alone answers only the first
  // half, so the include patterns are still consulted for the second.
  const includes = rules.filter((r) => !r.negate).map((r) => r.pattern);
  const inScope = (path: string) =>
    isInScope(path, rules) || includes.some((p) => p.startsWith(path + '/'));
  const files = (full.files ?? []).filter((f) => inScope(f.path));
  const folders = (full.folders ?? []).filter((m) => inScope(m.path));
  return { nodes, links, files, folders };
}

/** Apply the current selection: aggregate counts and publish the scoped
 *  graph. The graph is always published so downstream analysis views
 *  (Quality, Summary, Diff, Context) see the user's full scope.
 *
 *  Nothing here decides whether the canvas draws. It used to: a scope over
 *  `ENTITY_THRESHOLD` set a flag that `displayPlan` read before computing,
 *  which meant the decision was made from the selection's index total,
 *  upstream of collapse and of every filter. That gate now lives on the
 *  computed plan, measured on nodes actually drawn — see
 *  `viewmodels/drawCeiling.ts`. */
export async function applySelection(): Promise<void> {
  const idx = get(indexData);
  if (!idx) return;
  const rules = get(scopeRules);

  if (rules.length === 0) {
    publishGraph({ nodes: [], links: [] });
    return;
  }

  graphLoading.set(true);
  graphLoadError.set(null);

  try {
    const full = await ensureFullData();
    const scoped = filterToSelection(full, rules);
    // Auto-pick the aggregation level so the rendered node count stays
    // under RENDER_BUDGET. Users can pin the level via the level toggle,
    // which flips `autoLevel` off.
    // Expansions that fell out of the new scope are dropped: an open scope
    // the reader can no longer see is invisible state that would silently
    // re-open if they scoped back.
    pruneScopeState(scoped);
    const auto = get(autoLevel);
    const picked = pickLevel(scoped, RENDER_BUDGET, get(expandedScopes));
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

// ─────────────────────────────────────────────────────────────────────────────
// Navigation recording — the hook the wayback hangs on (UI-092)
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Called once, *before* a gesture replaces the picture, so the history can
 * bank what the reader was looking at.
 *
 * A registered callback rather than an import, and that is the whole design
 * decision here: the history has to read every store a view captures, which
 * means it depends on `stores/savedViews.ts`, which already depends on this
 * module. Importing it back would close the cycle on a file whose top level
 * builds a `derived` over `scopeRules` — the kind of cycle that does not
 * warn, it just hands the other module `undefined` at import time. So the
 * dependency points one way and the history reaches in.
 *
 * One subscriber, replaced rather than appended: there is exactly one history
 * and a second would be a bug rather than a feature. Until it registers this
 * is a no-op, which is the correct degradation — navigation still works, it
 * just is not remembered.
 */
let beforeNavigate: (() => void) | null = null;
let recording = true;
let navDepth = 0;

export function onBeforeNavigate(fn: () => void): void {
  beforeNavigate = fn;
}

/** Record the current picture, unless we are already inside a navigation or
 *  the caller has suspended recording. */
function markNavigation(): void {
  if (navDepth === 0 && recording) beforeNavigate?.();
}

/**
 * Run `fn` as one navigation step.
 *
 * The depth guard is what keeps `drillIn` — which sets `autoLevel` and *then*
 * calls `setScopes`, itself a navigation — from banking two frames, the
 * second of which would have the level flag already mutated. The outermost
 * gesture is the one the reader made, so it is the one that records.
 */
export async function asNavigation<T>(fn: () => Promise<T>): Promise<T> {
  markNavigation();
  navDepth++;
  try {
    return await fn();
  } finally {
    navDepth--;
  }
}

/** Run `fn` with recording off — for the history's own back and forward,
 *  which restore a frame through the same stores every gesture writes and
 *  would otherwise record the step they are undoing. */
export async function withoutNavigation<T>(fn: () => Promise<T>): Promise<T> {
  const was = recording;
  recording = false;
  try {
    return await fn();
  } finally {
    recording = was;
  }
}

/**
 * Flip one path in or out of scope and re-render.
 *
 * Deliberately *not* a navigation: this is the scope tree's checkbox, and a
 * reader who ticks five folders and then drills wants one step back to the
 * five-folder reading, not five steps back through the building of it
 * (UI-092). The commit gestures below record; the adjustments do not.
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
  markNavigation();
  scopeRules.set([]);
  publishGraph({ nodes: [], links: [] });
}

/**
 * Widen the scope: replace each selected path with its own parent folder
 * (root stays root), collapsing overlaps to the minimal covering set.
 * Pairs with "Visualize Current File" — start from a single file and grow
 * the scope one level per click.
 *
 * **Up, not back.** This climbs the folder tree; the wayback (UI-092)
 * returns to the previous picture. After `drillIntoMarks` — six files from
 * three folders — the parent folder is an answer to a question nobody asked
 * and "where was I" is exact, so the two controls both exist and neither
 * substitutes for the other.
 *
 * `autoLevel` goes back on for the same reason `drillIn` sets it: a wider
 * scope holds more entities than the narrow one did, and keeping the level
 * the narrow scope picked draws the parent folder at a detail it cannot
 * afford. Widening always means "as much as fits", never "this exact level".
 */
export function extendScopeToParents(): void {
  const sel = get(selectedScopes);
  if (sel.size === 0) return;
  const parents = new Set<string>();
  for (const p of sel) {
    const idx = p.lastIndexOf('/');
    parents.add(idx >= 0 ? p.slice(0, idx) : '');
  }
  markNavigation();
  autoLevel.set(true);
  scopeRules.set(compactRules(includeAll(parents)));
  applySelection();
}

/** Select every path in the index (equivalent to selecting the root scope). */
export function selectAllScope(): void {
  markNavigation();
  scopeRules.set(includeAll(['']));
  applySelection();
}

/**
 * Replace the current scope selection with an arbitrary set of paths and
 * re-render. Used by the VS Code native scope TreeView.
 *
 * Took a `force` flag until UI-061, to let a caller promise it had handled
 * the "too many entities" concern by enabling a filter that hides most of
 * them. That promise is now kept by the mechanism itself: filters run
 * upstream of the render gate, so a caller that enables one gets the effect
 * without an override, and there is no longer a check to skip.
 */
export async function setScopes(paths: string[]): Promise<void> {
  const idx = get(indexData);
  if (!idx) {
    // Index not loaded yet — remember the request and apply once it arrives.
    // A simple retry loop is fine here since the index loads quickly.
    for (let i = 0; i < 20 && !get(indexData); i++) {
      await new Promise((r) => setTimeout(r, 250));
    }
    if (!get(indexData)) return;
  }
  await asNavigation(async () => {
    scopeRules.set(compactRules(includeAll(paths)));
    await applySelection();
  });
}

/**
 * Widen the scope to also cover `paths`, keeping whatever is already there.
 *
 * Appended as includes, so an earlier exclusion that these paths fall under
 * is overridden for them and left standing for everything else — which is
 * what "add this to what I'm looking at" has to mean once exclusions exist.
 */
export async function addScopes(paths: string[]): Promise<void> {
  await asNavigation(async () => {
    scopeRules.update((rules) => compactRules([...rules, ...includeAll(paths)]));
    await applySelection();
  });
}

/** Drill into a scope: narrow selection AND re-enable auto-level so the
 *  view expands to the finest level the new (smaller) scope allows.
 *  Used by the double-click / "Drill in" button — always expresses the
 *  intent "show me as much detail as fits", overriding any prior manual
 *  level pin. */
export async function drillIn(path: string): Promise<void> {
  console.log(`[drill] drillIn() called path=${path} — resetting autoLevel=true and calling setScopes([${path}])`);
  // The wrapper, not `setScopes`'s, is the one that records: the frame has to
  // be captured before `autoLevel` moves, or back would restore the picture
  // with the flag the drill set (UI-092).
  await asNavigation(async () => {
    autoLevel.set(true);
    await setScopes([path]);
  });
  console.log(`[drill] drillIn() completed path=${path}`);
}

/**
 * Drill into a scope *without* changing the grain — the region gestures on
 * the canvas (UI-115's name click, UI-089's area double-click).
 *
 * `drillIn` re-enables auto-level because it answers "show me as much detail
 * as fits". A region is different: it is drawn *at* the level the reader is
 * reading at, and its name is a heading over nodes they are already looking
 * at. Clicking it says "just these" — it does not say "and now show me their
 * insides". A reader working at File level who focuses a folder and lands on
 * a canvas of entities has had their picture swapped out, not narrowed, and
 * has to walk the level toggle back every single time.
 *
 * Pinning `autoLevel` off is what makes the grain survive `applySelection`,
 * which would otherwise re-pick and re-publish — the same pin the level
 * toggle sets, and honest about it in the stats bar. Nothing here can
 * overflow the render budget: the new scope is a subset of the one on
 * screen, so a level that fit before still fits.
 *
 * Only File and Entity reach this. Regions are not drawn at Folder level
 * (`hullsEnabled` in GraphView), so there is no name to click and no
 * degenerate one-node picture to fall into.
 */
export async function drillInKeepingLevel(path: string): Promise<void> {
  console.log(`[drill] drillInKeepingLevel() called path=${path} — pinning level=${get(graphLevel)}`);
  await asNavigation(async () => {
    autoLevel.set(false);
    await setScopes([path]);
  });
}

/**
 * Drill into everything marked at once — the way from a picture of files to a
 * picture of the entities inside them.
 *
 * The same two moves as `drillIn`, and the plural is the entire point.
 * Relationships run *between* files, so a reader who wants to see one in
 * detail wants both of its ends and nothing else; drilling into one end throws
 * away the other, and drilling into the folder that holds both usually brings
 * back too much to render at entity level. `setScopes` compacts the paths, so
 * marking a folder and a file inside it is one scope rather than two.
 *
 * Nothing here mentions the aggregation level. `autoLevel` re-picks it from
 * `RENDER_BUDGET` against the new, smaller scope, so a handful of files opens
 * at Entity level for exactly the reason any small scope does. Marking half
 * the repo honestly gets File level back, which is what `markedStats` warns
 * about before the click rather than after.
 *
 * The marks are cleared once the scope has applied: the set has been spent,
 * and every node now drawn lives under a marked path, so keeping it would ring
 * the entire canvas. The scope itself is the durable record of the decision —
 * it is what the scope tree now shows.
 */
export async function drillIntoMarks(): Promise<void> {
  const paths = [...get(markedPaths)];
  if (paths.length === 0) return;
  console.log(`[drill] drillIntoMarks() called with ${paths.length} marked path(s)`);
  await asNavigation(async () => {
    autoLevel.set(true);
    await setScopes(paths);
  });
  clearMarks();
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
      // Recorded here rather than around the whole call: a path that never
      // arrives leaves the picture alone, and a frame banked for a navigation
      // that did not happen is a back press that does nothing (UI-092).
      await asNavigation(async () => {
        scopeRules.set(includeAll([path]));
        await applySelection();
      });
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

/**
 * What drilling into the marked set would load, and the level it would land
 * at — read from the index, so the button can say it before the click.
 *
 * `pickLevel` is the authority on the level and takes a graph, which is not
 * available for a scope that has not been loaded yet; `RENDER_BUDGET` against
 * the index's entity total is the same comparison against the same constant,
 * one step earlier. It can be wrong in the reader's favour — ghosts and
 * Elevator entities are excluded from the real count and not from this one —
 * so the warning is worded as the coarser outcome it predicts rather than as a
 * promise about what will be drawn.
 */
export const markedStats = derived(
  [indexData, markedPaths],
  ([$idx, $marks]) => {
    if (!$idx || $marks.size === 0) return { ...NO_STATS, fitsEntityLevel: true };
    const counts = scopeCounts(compactRules(includeAll($marks)), $idx);
    return { ...counts, fitsEntityLevel: counts.entities <= RENDER_BUDGET };
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
    const EMPTY: GraphData = { nodes: [], links: [], files: [], folders: [] };
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
      // A new root is a new checkout, and very often a different branch. No
      // `head` event announces this one — nothing moved on disk, the server
      // was repointed — so the chip is re-asked here (UI-114).
      void fetchBranch();
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
