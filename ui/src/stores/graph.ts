import { writable, derived, get } from 'svelte/store';
import type { D3Node, GraphData, GraphLevel, ViewMode, LevelOverrides, TriState } from '../types/graph';
import { collapseGraph } from '../viewmodels/collapseGraph';
import type { HoverMode } from '../viewmodels/hoverHighlight';

// Entity-level source of truth, written by `publishGraph`. Every other
// view derives from this + `graphLevel` — this is the single place scope
// selection lands data, and there's no path that mutates the displayed
// graph without going through here.
export const rawEntityGraph = writable<GraphData>({ nodes: [], links: [] });

// Aggregation level for the graph view. 'entity' is the default (every
// entity is its own node); 'file' collapses nodes to one-per-file; 'module'
// collapses to one-per-directory.
export const graphLevel = writable<GraphLevel>('entity');

/**
 * Scopes opened one level finer than `graphLevel` (UI-057).
 *
 * Holds **paths**, never ids: `collapseGraph` builds fresh node objects on
 * every level change and `sanitizeId` rewrites paths into ids, so an
 * id-keyed set would stop matching the moment the view moved.
 *
 * This is what makes the level a property of a *region* rather than of the
 * whole canvas. Before it, a repo above `RENDER_BUDGET` could never show a
 * single entity as an entity without narrowing the scope and throwing away
 * the context that made it interesting.
 */
export const expandedScopes = writable<Set<string>>(new Set());

/** Open or close one scope, leaving the rest of the view alone. */
export function toggleExpanded(path: string): void {
  expandedScopes.update((s) => {
    const next = new Set(s);
    if (!next.delete(path)) next.add(path);
    return next;
  });
}

export function collapseAllScopes(): void {
  expandedScopes.set(new Set());
}

// --- Graph data ---
//
// The graph that components render. Purely derived from `rawEntityGraph +
// graphLevel` so there is exactly one path that can change what's on
// screen: update the source. No ordering-sensitive `.set()` sequences,
// no races between level-toggle and scope-select handlers.
//
// Svelte recomputes this synchronously whenever either input fires, and
// the recomputation is atomic: every subscriber sees the same consistent
// (raw, level) pair.
export const graphData = derived(
  [rawEntityGraph, graphLevel, expandedScopes],
  ([$raw, $level, $expanded]) => {
    const result = collapseGraph($raw, $level, $expanded);
    console.log(`[graphData derived] level=${$level} expanded=${$expanded.size} rawNodes=${$raw.nodes.length} → collapsedNodes=${result.nodes.length} collapsedLinks=${result.links.length}`);
    return result;
  },
);

// --- Selection state ---
export const selectedNode = writable<D3Node | null>(null);

// Viewport dimensions (written by GraphView on mount + resize, read by
// displayPlan so the tree layout can wrap levels to fit the screen).
export const viewportWidth = writable<number>(1200);

/** Tree layout density: controls vertical spacing, separation multiplier,
 *  and sub-row gap. Cycles via a button in the controls bar. */
export type TreeDensity = 'compact' | 'normal' | 'spacious';
export const treeDensity = writable<TreeDensity>('normal');

/** Maximum tree depth (BFS hops from the selection). The Level 1/2
 *  toggles in the filter panel still override per-level, but this gives
 *  a quick 1/2/3 control from the top bar. */
export const treeMaxDepth = writable<number>(2);
export const hoveredNode = writable<D3Node | null>(null);
export const hoverLocked = writable(false);

/**
 * A hover lasts exactly as long as the canvas draws the node.
 *
 * `mouseout` is the only thing that clears it, and it never fires when the
 * re-render destroys the element the pointer was over — so navigating from
 * a side panel (a Description rung, a relationship row, a search hit) left
 * the hover pinned to an entity that was no longer on screen, and the
 * Description pane, which prefers hover over selection, went on narrating
 * it. The lock is the one case where a hover is meant to outlive the
 * pointer, so it is honoured here too.
 */
graphData.subscribe(($data) => {
  const hovered = get(hoveredNode);
  if (!hovered || get(hoverLocked)) return;
  if (!$data.nodes.some((n) => n.id === hovered.id)) hoveredNode.set(null);
});

/**
 * Make `node` the subject, from a side panel.
 *
 * Selecting is all the canvas needs, but the pointer is over a panel when
 * this runs, so whatever the graph still thinks is hovered is a leftover —
 * and the Description pane reads hover *before* selection. Without the
 * clear, following a relationship or a child moved the Details column and
 * the canvas while the prose column went on describing the node you left.
 *
 * A frozen preview (`L`) is the one hover the reader asked to keep, so it
 * survives; the Details column shows the new selection either way.
 */
export function focusNode(node: D3Node) {
  if (!get(hoverLocked)) hoveredNode.set(null);
  selectedNode.set(node);
}

/** Number of relationship hops to highlight when hovering a node.
 *  1 = direct connections only (default), 2 = friends-of-friends, 3 = three hops. */
export const hoverDepth = writable<number>(1);

/** What a hover lights up: what this node connects to, or what it lives with
 *  (UI-054). Ephemeral like `hoverDepth` — it is a way of reading the current
 *  picture, not a preference about how the tool should open. */
export const hoverMode = writable<HoverMode>('connections');

// --- View mode ---
export const viewMode = writable<ViewMode>('graph');

// --- Display toggles ---
export const showLabels = writable(true);
export const showKindLabels = writable(true);
/** Off by default: at file aggregation nearly every edge is the same kind,
 *  so labelling them all buried the node names under ~68 identical pills
 *  (UI-015). One click away when the relationship mix actually matters. */
export const showLinkLabels = writable(false);
export const showGhostNodes = writable(true);
/** Independent toggle for `ghost_stdlib` entities (Python `print`,
 *  `len`, Rust `Vec`/`HashMap`, JS `console`, …). Off by default — the
 *  calls are in the dataset but hidden, so users can flip this when
 *  they explicitly want to audit builtin usage without cluttering the
 *  business-logic view the rest of the time. */
export const showBuiltinGhosts = writable(false);

/** Independent toggle for the ansible-deploy templating layer — nodes
 *  tagged `template_var` (the `{{ … }}` config variables and their
 *  definitions). Off by default: the layer is high-volume (hundreds of
 *  nodes) and would bury the deploy topology; flip it on to inspect
 *  variable usage / change-impact. No effect on non-ansible graphs. */
export const showTemplateVars = writable(false);

/** What a committed entity search does to everything it didn't match.
 *
 *  Dimming by default: the control reads as "show me where X is", and
 *  deleting the surrounding graph removes exactly the context that answers
 *  *where*. Hiding stays available for a scope busy enough that a dimmed
 *  wash is unreadable — and it is the cheaper of the two, since a hidden
 *  node leaves the force simulation and a dimmed one does not. */
export const searchHidesNonMatches = writable(false);

/** Opacity for search-dimmed nodes. Separate from `diffDimOpacity`, which
 *  defaults to 0 — sharing one control would let a diff slider parked at
 *  zero erase the context a search is trying to preserve. */
export const searchDimOpacity = writable(0.15);

// --- General filters ---
export const generalEntityTypes = writable<Set<string>>(new Set());
export const generalRelTypes = writable<Set<string>>(new Set());
export const generalOutgoing = writable(true);
export const generalIncoming = writable(true);

// --- Language filter ---
//
// Stored as what the user *excluded*, for the same reason as the file filter
// below: `generalLanguages` used to be the writable, re-seeded to "every
// language in the new dataset" on every publish — so unticking Markdown
// survived until the next level toggle, scope change, or auto-level
// escalation, all of which republish. Switching Module → File is exactly that
// republish, and it silently restored every language the user had turned off.
//
// Language names, not ids, so nothing needs translating between levels — a
// collapsed File or Module node carries the language of the entities it rolls
// up. `generalLanguages` is derived from this, next to `visibleFiles`, since
// it needs `allLanguages`.
export const hiddenLanguages = writable<Set<string>>(new Set());

// --- File filter ---
//
// Stored as what the user *excluded*, not as what is left. `visibleFiles`
// used to be the writable, reset to "every file in the new dataset" on every
// publish — which meant hiding six noisy files survived until the next level
// toggle, scope change, or auto-level escalation, and auto-level fires on its
// own (UI-047). An exclusion is a decision the user made and nothing in the
// dataset can imply it, so it is the thing that has to persist; the visible
// set is derived and can be recomputed from whatever is on screen.
//
// Paths only, never ids: `collapseGraph` rewrites node ids at File and Module
// level but carries `file_path` through, so a path-keyed exclusion needs no
// translation between levels. At Module level the aggregated nodes carry a
// *directory* path, so nothing matches and nothing hides — correct, since
// there is no per-file node there to hide, and dropping back to File or
// Entity level restores the exclusions intact.
export const hiddenFiles = writable<Set<string>>(new Set());

// --- Level overrides ---
export const levelOverrides = writable<Record<number, LevelOverrides>>({
  1: { enabled: true, entityTypes: {}, relTypes: {}, outgoing: 'general', incoming: 'general', peerEdges: true },
  2: { enabled: false, entityTypes: {}, relTypes: {}, outgoing: 'general', incoming: 'general', peerEdges: true },
  3: { enabled: false, entityTypes: {}, relTypes: {}, outgoing: 'general', incoming: 'general', peerEdges: true },
});

/**
 * When a node is selected, controls whether edges incident to the selected
 * node itself are drawn. Decoupled from the Level 1 `enabled` flag so the
 * user can view peer-only topology (L1↔L1, L2↔L2) without the visual noise
 * of the selected node's own edges, or the inverse.
 */
export const showDirectEdges = writable<boolean>(true);

/**
 * Tree depth as the toolbar and the editor host's `setTreeDepth` command both
 * mean it: the BFS hop limit *and* the per-level `enabled` flags, kept in
 * step. Setting `treeMaxDepth` alone leaves the filter panel claiming level 3
 * is on while the tree stops at level 2.
 */
export function setTreeDepth(depth: number) {
  treeMaxDepth.set(depth);
  levelOverrides.update((lo) => {
    for (let i = 1; i <= 3; i++) {
      if (lo[i]) lo[i].enabled = i <= depth;
    }
    return { ...lo };
  });
}

export const DENSITY_LABELS: Record<TreeDensity, string> = {
  compact: 'Compact', normal: 'Normal', spacious: 'Spacious',
};
const DENSITY_CYCLE: TreeDensity[] = ['compact', 'normal', 'spacious'];

export function cycleTreeDensity() {
  treeDensity.update((d) => DENSITY_CYCLE[(DENSITY_CYCLE.indexOf(d) + 1) % DENSITY_CYCLE.length]);
}

/**
 * When a node is selected and Level 2 is enabled, controls whether edges
 * between nodes at different levels (L1↔L2) are drawn. These are the
 * "expansion" edges that bring level-2 nodes into view — hiding them shows
 * level-2 nodes floating disconnected, which can be useful for focusing on
 * peer topology within each level without the cross-level scaffolding.
 */
export const showCrossLevelEdges = writable<boolean>(true);

// --- Derived: all entity types present in data ---
export const allEntityTypes = derived(graphData, ($data) =>
  [...new Set($data.nodes.map((n) => n.kind_raw))].sort()
);

// --- Derived: all relationship types present in data ---
export const allRelTypes = derived(graphData, ($data) =>
  [...new Set($data.links.map((l) => l.kind_raw))].sort()
);

// --- Derived: all languages present in data ---
export const allLanguages = derived(graphData, ($data) =>
  [...new Set($data.nodes.map((n) => n.language))].sort()
);

// --- Derived: all file paths ---
export const allFiles = derived(graphData, ($data) =>
  [...new Set($data.nodes.map((n) => n.file_path))].sort()
);

/** What the language filter lets through: every language in the current
 *  dataset the user hasn't hidden. Read by `displayPlan` as an allow-list,
 *  exactly as when it was a writable — only the thing that writes it changed.
 *  Declared here rather than beside `hiddenLanguages` because it reads
 *  `allLanguages`, which is defined above. */
export const generalLanguages = derived(
  [allLanguages, hiddenLanguages],
  ([$all, $hidden]) => new Set($all.filter((l) => !$hidden.has(l))),
);

/** What the file filter lets through: everything in the current dataset the
 *  user hasn't hidden. Read by `displayPlan` as an allow-list, exactly as
 *  before — only the thing that writes it changed. */
export const visibleFiles = derived(
  [allFiles, hiddenFiles],
  ([$all, $hidden]) => new Set($all.filter((f) => !$hidden.has(f))),
);

/**
 * Drop exclusions for files that no longer exist.
 *
 * Called with the paths of the *whole repo* (not the current scope), so
 * scoping away from a hidden file and back again remembers that it was
 * hidden — which is the entire point — while a file deleted from the repo
 * stops accumulating in the set. Anything narrower would reintroduce the bug
 * one level down.
 */
export function pruneHiddenFiles(existing: Set<string>): void {
  hiddenFiles.update((s) => {
    const next = new Set([...s].filter((f) => existing.has(f)));
    return next.size === s.size ? s : next;
  });
}

// --- Helper: resolve tri-state ---
export function resolveTriState(triValue: TriState, generalValue: boolean): boolean {
  if (triValue === 'on') return true;
  if (triValue === 'off') return false;
  return generalValue;
}

// Filter seeding lives in viewmodels/filterViewModel.ts (publishGraph).
// Components that need to push a new dataset call publishGraph there
// rather than touching graphData directly.
