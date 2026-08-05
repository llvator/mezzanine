/**
 * displayPlan — single-source-of-truth viewmodel for the GraphView.
 *
 * Derived from every input that affects what's drawn (graph data, view
 * mode, selection, filters, level overrides, search). Recomputes
 * atomically on any change. The View just renders the plan; it never
 * decides what to show.
 *
 * This is the MVVM seam: prior to its introduction, GraphView itself
 * orchestrated visibility + layout via ten independent store
 * subscribers, each calling `applyFilters` / `applyTreeLayout` /
 * `exitTreeLayout` in mutually-incompatible orders. That made every
 * data flow ordering-sensitive and produced a long tail of "switch
 * level + tree view = empty graph" bugs. Centralising the decision
 * here means there's exactly one place where "what should be drawn"
 * is computed, and it's pure.
 */

import { derived, type Readable } from 'svelte/store';
import * as d3 from 'd3';
import type { D3Node, D3Link, GraphData, ViewMode, LevelOverrides, TriState } from '../types/graph';
import { isSpecNode } from '../types/graph';
import { pathsClaim } from '../utils/refPaths';
import {
  graphData,
  selectedNode,
  viewMode,
  generalEntityTypes,
  generalRelTypes,
  generalLanguages,
  visibleFiles,
  generalOutgoing,
  generalIncoming,
  levelOverrides,
  showDirectEdges,
  showCrossLevelEdges,
  viewportWidth,
  treeDensity,
  treeMaxDepth,
  resolveTriState,
  showGhostNodes,
  showBuiltinGhosts,
  showTemplateVars,
  searchHidesNonMatches,
  searchDimOpacity,
  type TreeDensity,
} from '../stores/graph';
import { searchMatchIds, searchNeighborIds } from './filterViewModel';
import { diffActive, diffChangesOnly, diffCoreOnly, diffFiltersEnabled, diffStatusMap, diffSourceChangedMap, diffScopeChanges, diffDimOpacity, normalizeEntityId, type ChangeStatus } from '../stores/diff';
import type { ScopeChange } from './diffRollup';
import { gateByDrawCeiling, type DrawOverflow } from './drawCeiling';
import { rankHubs } from './hubs';
import { demoteHubs, hubCount } from '../stores/settings';
import { crossFilterPaths } from '../stores/crossFilter';
import { splitViewOpen } from '../stores/panes';

export interface DisplayPlan {
  /** 'tree' when viewMode === 'tree' AND a selection exists in the
   *  current graph; otherwise 'force'. The View uses this to decide
   *  between pinning nodes to tree positions vs. running the simulation. */
  mode: 'force' | 'tree';
  /** Ids of nodes that should be visible in the current view. Includes
   *  filter, search, and selection-distance gating. */
  visibleNodeIds: Set<string>;
  /** Stable per-link identifiers for visible links. Format: `${src}->${tgt}|${kind}` */
  visibleLinkKeys: Set<string>;
  /** Selected node id, or null. Plain pass-through for the .selected class. */
  selectedId: string | null;
  /** Tree-mode positions, keyed by node id. Empty in force mode. */
  treePositions: Map<string, { x: number; y: number }>;
  /** BFS distance from the selection to each node, capped at maxLevel.
   *  Populated in BOTH modes when there's a selection — used by the View
   *  to dim non-neighbours and to classify edges in tree mode. */
  nodeDistances: Map<string, number> | null;
  /** Primary-search highlight sets. Pass-through — the plan only uses them
   *  to narrow visibility when a search is committed. The display-search
   *  highlight (Ctrl+F within the view) is NOT part of the plan: it's a
   *  purely cosmetic overlay and the View subscribes to it directly,
   *  which keeps displayPlan out of the displaySearch ↔ visibleNodeIds
   *  feedback loop. */
  searchMatched: Set<string>;
  searchNeighbors: Set<string>;
  /** Ids of nodes whose *incoming* edges are suppressed because they are
   *  among the most-depended-on in the current view (UI-056). The nodes
   *  themselves are still drawn and still selectable — this hides edges,
   *  never entities. */
  demotedHubIds: Set<string>;
  /** Node ids that passed every filter except one that dims rather than
   *  hides — the diff filters, or a committed search when it is set to dim.
   *  Drawn at `dimOpacity` instead of being removed. */
  dimmedNodeIds: Set<string>;
  /** Opacity for `dimmedNodeIds`. Carried on the plan rather than read from
   *  a store by the View because the two producers have different defaults:
   *  the diff slider sits at 0, and a search that dimmed to 0 would just be
   *  hiding with extra steps. When both dim, the more visible value wins —
   *  the failure worth avoiding is "the node vanished". */
  dimOpacity: number;
  /** Non-null when the plan wanted more nodes on screen than the canvas
   *  draws, in which case every visibility set above is empty and the View
   *  shows the overflow card instead.
   *
   *  Carried on the plan rather than kept in a store because it is a
   *  property *of this plan* — the count it reports is the one `compute()`
   *  just produced under the current filters. The store it replaced was
   *  written from `applySelection` and read here before `compute()` ran,
   *  which is precisely why no filter could move it (UI-061). */
  overflow: DrawOverflow | null;
}

/** Link key that distinguishes duplicate edges between the same pair.
 * `order` differentiates multiple calls to the same target (e.g.,
 * `remove_worktree` called 3 times with orders #11, #19, #20). */
const linkKey = (src: string, tgt: string, kind: string, order?: number | null) =>
  order == null ? `${src}->${tgt}|${kind}` : `${src}->${tgt}|${kind}#${order}`;
const sourceIdOf = (l: D3Link): string => typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
const targetIdOf = (l: D3Link): string => typeof l.target === 'object' ? (l.target as D3Node).id : l.target;

function getMaxLevel(lo: Record<number, LevelOverrides>, depthCap: number): number {
  for (let i = depthCap; i >= 1; i--) {
    if (lo[i]?.enabled) return i;
  }
  return 0;
}

function resolveDirection(lo: Record<number, LevelOverrides>, level: number, dir: 'outgoing' | 'incoming', generalOut: boolean, generalIn: boolean): boolean {
  return resolveTriState(lo[level]?.[dir] || 'general', dir === 'outgoing' ? generalOut : generalIn);
}

function resolveEntityType(lo: Record<number, LevelOverrides>, level: number, type: string, generalKinds: Set<string>): boolean {
  const override = lo[level]?.entityTypes[type] || 'general';
  return resolveTriState(override as TriState, generalKinds.has(type));
}

function resolveRelType(lo: Record<number, LevelOverrides>, level: number, type: string, generalRels: Set<string>): boolean {
  const override = lo[level]?.relTypes[type] || 'general';
  return resolveTriState(override as TriState, generalRels.has(type));
}

interface ComputeArgs {
  graph: GraphData;
  selected: D3Node | null;
  viewMode: ViewMode;
  kinds: Set<string>;
  rels: Set<string>;
  langs: Set<string>;
  files: Set<string>;
  demoteHubs: boolean;
  hubCount: number;
  generalOut: boolean;
  generalIn: boolean;
  lo: Record<number, LevelOverrides>;
  showDirect: boolean;
  showCross: boolean;
  searchMatched: Set<string>;
  searchNeighbors: Set<string>;
  vpWidth: number;
  density: TreeDensity;
  maxDepth: number;
  // Diff filter: when active + changesOnly, hide unchanged entities.
  diffIsActive: boolean;
  /** Committed search hides non-matches instead of dimming them. */
  searchHides: boolean;
  searchDim: number;
  diffDim: number;
  diffChangesOnly: boolean;
  diffCoreOnly: boolean;
  diffStatuses: Map<string, ChangeStatus>;
  diffSourceChanged: Map<string, boolean>;
  /** Scope path → rolled-up change, for the collapsed levels where a node's
   *  `original_id` is a path rather than an entity id (UI-064). */
  diffScopes: Map<string, ScopeChange>;
  showGhosts: boolean;
  /** Independent toggle for `ghost_stdlib`-tagged ghosts. When false,
   *  those ghosts are hidden even if `showGhosts` is true. */
  showBuiltinGhosts: boolean;
  /** Toggle for the `template_var`-tagged ansible templating layer.
   *  When false (default), those nodes and their edges are hidden. */
  showTemplateVars: boolean;
  /** Split view: the Elevator layer draws in its own pane, so it comes out
   *  of this one (ADR 0011). */
  splitView: boolean;
  /** Code paths the focused spec entity claims, or null when no cross-filter
   *  is active. `[]` is a real value meaning "claims nothing" — see
   *  `stores/crossFilter.ts`. */
  crossFilterPaths: string[] | null;
}

function emptyPlan(): DisplayPlan {
  return {
    mode: 'force',
    visibleNodeIds: new Set(),
    visibleLinkKeys: new Set(),
    selectedId: null,
    treePositions: new Map(),
    nodeDistances: null,
    searchMatched: new Set(),
    searchNeighbors: new Set(),
    demotedHubIds: new Set(),
    dimmedNodeIds: new Set(),
    dimOpacity: 0,
    overflow: null,
  };
}

/** Check if a node fails only the diff filter (passes all other filters).
 *  Returns 'visible', 'dimmed' (diff-filtered only), or 'hidden'. */
type FilterResult = 'visible' | 'dimmed' | 'hidden';

/** Filter pass for nodes — kind/language/file plus search restriction.
 *  Selection-distance gating is applied separately because it depends on
 *  the BFS, which only runs when a selection is active. */
function nodePassesFilters(n: D3Node, args: ComputeArgs): FilterResult {
  // Ghost filter — two layers: master toggle hides every ghost; stdlib
  // sub-toggle hides builtin (`ghost_stdlib`) ghosts independently so a
  // user can see library ghosts without the builtin noise.
  if (n.tags?.includes('ghost')) {
    if (!args.showGhosts) return 'hidden';
    if (!args.showBuiltinGhosts && n.tags.includes('ghost_stdlib')) return 'hidden';
  }
  // Ansible templating layer — hidden by default so the high-volume
  // config-variable nodes don't bury the deploy topology.
  if (!args.showTemplateVars && n.tags?.includes('template_var')) return 'hidden';
  // Split view: the spec layer is drawn by its own pane, so this one stops
  // drawing it. Without this the `.elv` entities appear twice on screen and
  // the code pane's force layout spends its space on nodes that already have
  // a better home (ADR 0011).
  if (args.splitView && isSpecNode(n)) return 'hidden';
  // Cross-filter from the spec pane. Hard, not dimmed: the interaction asks
  // for "only what belongs to this", and an empty path list is a real answer
  // ("this entity declares no code") rather than a cleared filter — the
  // blank-canvas card names it.
  if (args.crossFilterPaths && !pathsClaim(args.crossFilterPaths, n.file_path)) {
    return 'hidden';
  }
  // Hard filters — these truly hide the node
  if (!args.kinds.has(n.kind_raw)) return 'hidden';
  if (!args.langs.has(n.language)) return 'hidden';
  if (n.file_path && !args.files.has(n.file_path)) return 'hidden';
  if (args.searchMatched.size > 0) {
    if (!args.searchMatched.has(n.id) && !args.searchNeighbors.has(n.id)) {
      // A search answers "where is X"; deleting everything around the answer
      // removes what makes it an answer. Hiding stays one toggle away.
      return args.searchHides ? 'hidden' : 'dimmed';
    }
  }
  // Diff filter: when diff mode is active, dim (not hide) filtered nodes.
  if (args.diffIsActive && (args.diffChangesOnly || args.diffCoreOnly)) {
    const change = changeOf(n, args);
    if (!change) return diffUnknown(n, args);
    if (args.diffChangesOnly && change.status === 'unchanged') return 'dimmed';
    if (args.diffCoreOnly && (change.status === 'unchanged' || !change.sourceChanged)) {
      return 'dimmed';
    }
  }
  return 'visible';
}

/**
 * What the diff says about a node, or null when it has nothing to say.
 *
 * Entity nodes are keyed by entity id. A collapsed File or Module node
 * carries its scope path in `original_id` instead, so the entity lookup
 * misses and the scope rollup answers (UI-064). Entity ids and scope paths
 * do not collide — an id carries `:line:name` — so trying them in this order
 * needs no discriminator on the node.
 */
function changeOf(n: D3Node, args: ComputeArgs): ScopeChange | null {
  const id = normalizeEntityId(n.original_id);
  const status = args.diffStatuses.get(id);
  if (status) {
    return { status, sourceChanged: args.diffSourceChanged.get(id) ?? true };
  }
  return args.diffScopes.get(n.original_id) ?? null;
}

/**
 * Verdict for a node the diff never mentioned.
 *
 * Unknown is not unchanged, and conflating the two is how the interesting
 * nodes disappear: a file created since the diff ran is absent from every
 * map, and dimming it to `diffDimOpacity: 0` hides exactly the thing the
 * user opened a diff to see.
 *
 * The distinction that survives maintenance is whether the diff *looked*.
 * It reports on unchanged entities too, so its scope keys are the whole file
 * tree as of the moment it ran:
 *
 * - the node's file is in that tree ⇒ the diff considered the file and said
 *   nothing about this node, which means it is one of the kinds the diff
 *   deliberately skips (`Parameter`) or a ghost. Dim it, as before.
 * - the file is absent ⇒ the diff never saw it. Show it.
 *
 * Ghosts carry an empty `file_path` and so take the first branch, leaving
 * their visibility to `showGhosts` where it belongs.
 */
function diffUnknown(n: D3Node, args: ComputeArgs): FilterResult {
  if (n.file_path && !args.diffScopes.has(n.file_path)) return 'visible';
  return 'dimmed';
}

/** BFS over dependency edges from `start`, limited by maxLevel and the
 *  per-level direction / kind / entity-type overrides. Returns distance
 *  per reachable node id. */
function bfsDistances(graph: GraphData, start: string, args: ComputeArgs): Map<string, number> {
  const maxLevel = getMaxLevel(args.lo, args.maxDepth);
  const distances = new Map<string, number>();
  distances.set(start, 0);
  if (maxLevel === 0) return distances;

  // Adjacency cache — building it once per recompute is far cheaper
  // than scanning data.links inside the inner loop on every BFS step.
  const out = new Map<string, D3Link[]>();
  const inn = new Map<string, D3Link[]>();
  for (const l of graph.links) {
    const s = sourceIdOf(l);
    const t = targetIdOf(l);
    if (!out.has(s)) out.set(s, []);
    out.get(s)!.push(l);
    if (!inn.has(t)) inn.set(t, []);
    inn.get(t)!.push(l);
  }
  const nodeById = new Map(graph.nodes.map((n) => [n.id, n]));

  let queue = [start];
  for (let lvl = 1; lvl <= maxLevel; lvl++) {
    if (!args.lo[lvl]?.enabled) continue;
    const dirOut = resolveDirection(args.lo, lvl, 'outgoing', args.generalOut, args.generalIn);
    const dirIn = resolveDirection(args.lo, lvl, 'incoming', args.generalOut, args.generalIn);
    const next: string[] = [];
    // Branch nodes are synthetic grouping nodes — traversing one doesn't
    // consume a level, so `consider` enqueues them into `sameLevel` and
    // we keep expanding at the same depth until no new branches surface.
    const visitFrom = (cur: string) => {
      const sameLevel: string[] = [];
      const consider = (link: D3Link, neighborId: string) => {
        if (distances.has(neighborId)) return;
        if (!resolveRelType(args.lo, lvl, link.kind_raw, args.rels)) return;
        const node = nodeById.get(neighborId);
        if (!node) return;
        if (!resolveEntityType(args.lo, lvl, node.kind_raw, args.kinds)) return;
        const isGhost = node.tags?.includes('ghost');
        if (!isGhost && !args.files.has(node.file_path)) return;
        if (isGhost && !args.showGhosts) return;
        if (isGhost && !args.showBuiltinGhosts && node.tags?.includes('ghost_stdlib')) return;
        distances.set(neighborId, lvl);
        if (node.kind_raw === 'Branch' || node.kind_raw === 'Loop') {
          // Transparent — expand through it at the same level.
          sameLevel.push(neighborId);
        } else {
          next.push(neighborId);
        }
      };
      if (dirOut) for (const l of out.get(cur) ?? []) consider(l, targetIdOf(l));
      if (dirIn) for (const l of inn.get(cur) ?? []) consider(l, sourceIdOf(l));
      // Drain any Branch hops discovered during this fan-out, keeping
      // their callees at the current level.
      while (sameLevel.length > 0) {
        const branchId = sameLevel.shift()!;
        if (dirOut) for (const l of out.get(branchId) ?? []) consider(l, targetIdOf(l));
        if (dirIn) for (const l of inn.get(branchId) ?? []) consider(l, sourceIdOf(l));
      }
    };
    for (const cur of queue) visitFrom(cur);
    queue = next;
  }
  return distances;
}

/** Tree layout via d3.tree on a BFS spanning tree. Returns positions
 *  per node id (selection at canvas centre, outgoing growing downward,
 *  incoming upward). Pure — uses estimated text widths instead of
 *  measuring DOM, so layout drift is at most a small horizontal
 *  imprecision the View can't help anyway. */
function computeTreePositions(graph: GraphData, args: ComputeArgs): {
  positions: Map<string, { x: number; y: number }>;
  levels: Map<string, number>;
  nodeIds: Set<string>;
} {
  const sel = args.selected!;
  const positions = new Map<string, { x: number; y: number }>();
  const levels = new Map<string, number>();
  const nodeIds = new Set<string>([sel.id]);
  levels.set(sel.id, 0);

  const maxLevel = getMaxLevel(args.lo, args.maxDepth);
  if (maxLevel === 0) {
    positions.set(sel.id, { x: 0, y: 0 });
    return { positions, levels, nodeIds };
  }

  const out = new Map<string, D3Link[]>();
  const inn = new Map<string, D3Link[]>();
  for (const l of graph.links) {
    const s = sourceIdOf(l);
    const t = targetIdOf(l);
    if (!out.has(s)) out.set(s, []);
    out.get(s)!.push(l);
    if (!inn.has(t)) inn.set(t, []);
    inn.get(t)!.push(l);
  }
  const nodeById = new Map(graph.nodes.map((n) => [n.id, n]));

  // Spanning trees: parent map per direction, capturing the first reach.
  const outParent = new Map<string, string>();
  const inParent = new Map<string, string>();
  // Call-site order on the spanning-tree edge for outgoing children.
  // Sort key for siblings within the outgoing tree so callees appear in
  // source-call order rather than by the callee's own definition line
  // (which is unrelated to where the parent calls them).
  const outOrder = new Map<string, number>();
  const visited = new Set<string>([sel.id]);

  // Outgoing first.
  let q = [sel.id];
  /** Branch nodes are synthetic grouping nodes. The BFS is "transparent"
   *  across them in two senses:
   *
   *   1. `levels` (used for filter classification and for the final Y
   *      override below) counts only non-Branch hops, so a callee
   *      nested inside any number of branches still registers at the
   *      same depth as a direct call from the caller.
   *   2. `outParent`/`inParent` keeps the natural AST-style hierarchy
   *      (a nested branch's parent stays its enclosing branch), so the
   *      tree renders nested branches under their actual container —
   *      the Y override after `d3.tree` flattens their visual row. */
  const isTransparent = (id: string): boolean => {
    const k = nodeById.get(id)?.kind_raw;
    return k === 'Branch' || k === 'Loop';
  };

  for (let lvl = 1; lvl <= maxLevel; lvl++) {
    if (!args.lo[lvl]?.enabled) break;
    const dirOut = resolveDirection(args.lo, lvl, 'outgoing', args.generalOut, args.generalIn);
    if (!dirOut) { q = []; continue; }
    const next: string[] = [];

    const visitOutFrom = (cur: string) => {
      for (const l of out.get(cur) ?? []) {
        const tgt = targetIdOf(l);
        if (visited.has(tgt)) {
          continue;
        }
        if (!resolveRelType(args.lo, lvl, l.kind_raw, args.rels)) {
          continue;
        }
        const node = nodeById.get(tgt);
        if (!node) {
          continue;
        }
        if (!resolveEntityType(args.lo, lvl, node.kind_raw, args.kinds)) {
          continue;
        }
        const isGhost = node.tags?.includes('ghost');
        if (!isGhost && !args.files.has(node.file_path)) {
          continue;
        }
        if (isGhost && !args.showGhosts) {
          continue;
        }
        if (isGhost && !args.showBuiltinGhosts && node.tags?.includes('ghost_stdlib')) {
          continue;
        }
        if (
          (node.tags?.includes('class_field') || node.kind_raw === 'Property') &&
          node.parent_id === sel.original_id
        ) {
          continue;
        }
        visited.add(tgt);
        // AST-style parent: the natural enclosing node, not a flattened
        // effective ancestor. Nested branches keep their container as
        // parent; the Y override below (using `levels`) is what
        // collapses the visual rows.
        outParent.set(tgt, cur);
        if (l.order != null) outOrder.set(tgt, l.order);
        levels.set(tgt, lvl);
        nodeIds.add(tgt);
        if (isTransparent(tgt)) {
          // Still at the same filter level — descend through the branch
          // without advancing `lvl`, so a call nested any number of
          // branches deep is classified as if it were at this level.
          visitOutFrom(tgt);
        } else {
          next.push(tgt);
        }
      }
    };
    for (const cur of q) visitOutFrom(cur);
    q = next;
  }

  // Incoming next, skipping anything already in the outgoing tree.
  q = [sel.id];
  for (let lvl = 1; lvl <= maxLevel; lvl++) {
    if (!args.lo[lvl]?.enabled) break;
    const dirIn = resolveDirection(args.lo, lvl, 'incoming', args.generalOut, args.generalIn);
    if (!dirIn) { q = []; continue; }
    const next: string[] = [];

    const visitInTo = (cur: string) => {
      for (const l of inn.get(cur) ?? []) {
        const src = sourceIdOf(l);
        if (visited.has(src)) continue;
        if (!resolveRelType(args.lo, lvl, l.kind_raw, args.rels)) continue;
        const node = nodeById.get(src);
        if (!node) continue;
        if (!resolveEntityType(args.lo, lvl, node.kind_raw, args.kinds)) continue;
        const isGhost = node.tags?.includes('ghost');
        if (!isGhost && !args.files.has(node.file_path)) continue;
        if (isGhost && !args.showGhosts) continue;
        if (isGhost && !args.showBuiltinGhosts && node.tags?.includes('ghost_stdlib')) continue;
        visited.add(src);
        inParent.set(src, cur);
        levels.set(src, lvl);
        nodeIds.add(src);
        if (isTransparent(src)) {
          visitInTo(src);
        } else {
          next.push(src);
        }
      }
    };
    for (const cur of q) visitInTo(cur);
    q = next;
  }

  // Build hierarchy from the parent map and run d3.tree. Estimated widths
  // give a deterministic (DOM-free) layout; tree layout uses these to size
  // the horizontal allocation per node. ~7px per character + minimum.
  // Density presets control spacing between levels (vertical), separation
  // between siblings (horizontal multiplier), and sub-row gap.
  const densityCfg = {
    compact:  { vertSpacing: 70,  sepMul: 0.7, subRowGap: 30 },
    normal:   { vertSpacing: 120, sepMul: 1.0, subRowGap: 50 },
    spacious: { vertSpacing: 180, sepMul: 1.5, subRowGap: 70 },
  }[args.density];

  const estimatedWidth = (id: string) => {
    const name = nodeById.get(id)?.name ?? '';
    return Math.max(name.length * 7 + 30, 80) * densityCfg.sepMul;
  };

  interface TreeNode { id: string; children?: TreeNode[] }
  const buildTree = (
    rootId: string,
    parent: Map<string, string>,
    orderMap: Map<string, number>,
  ): TreeNode => {
    const childMap = new Map<string, string[]>();
    parent.forEach((p, child) => {
      if (!childMap.has(p)) childMap.set(p, []);
      childMap.get(p)!.push(child);
    });
    // Sort children by source line number so the tree preserves
    // declaration order (e.g., methods in an impl block appear in the
    // same order as in the source file). Branch nodes all inherit
    // their caller's line, so break those ties by the numeric suffix
    // of the branch name (c1, c2, c3, …) — left-to-right = increasing
    // branch index.
    const branchIndex = (name: string | undefined): number => {
      if (!name) return Number.MAX_SAFE_INTEGER;
      // Branch names are dotted paths (`c1`, `c1.2`, `c1.1.3`). Siblings
      // always share the same prefix and differ in the last integer
      // segment, so extracting that is sufficient for left-to-right
      // ordering within a parent.
      const m = /(\d+)$/.exec(name);
      return m ? Number.parseInt(m[1], 10) : Number.MAX_SAFE_INTEGER;
    };
    childMap.forEach((ids) => {
      ids.sort((a, b) => {
        // Primary key: call-site order from the parent → child link.
        // The callee's own `line` is its definition line, which is
        // unrelated to where the parent calls it — using `order` keeps
        // sibling callees in execution order left-to-right. Falls
        // back to line for children whose incoming edge has no order
        // (Branches via Contains, structural relationships, etc.).
        const oa = orderMap.get(a);
        const ob = orderMap.get(b);
        if (oa != null && ob != null) return oa - ob;
        const na = nodeById.get(a);
        const nb = nodeById.get(b);
        const la = na?.line ?? 0;
        const lb = nb?.line ?? 0;
        if (la !== lb) return la - lb;
        const isBranchA = na?.kind_raw === 'Branch';
        const isBranchB = nb?.kind_raw === 'Branch';
        if (isBranchA && isBranchB) return branchIndex(na?.name) - branchIndex(nb?.name);
        // Keep non-branch siblings stable by name as a last-resort tiebreak.
        return (na?.name ?? '').localeCompare(nb?.name ?? '');
      });
    });
    const walk = (id: string): TreeNode => {
      const kids = childMap.get(id) ?? [];
      return kids.length ? { id, children: kids.map(walk) } : { id };
    };
    return walk(rootId);
  };

  const layout = d3
    .tree<TreeNode>()
    .nodeSize([1, densityCfg.vertSpacing])
    .separation((a, b) => {
      const wa = estimatedWidth(a.data.id);
      const wb = estimatedWidth(b.data.id);
      const base = (wa + wb) / 2;
      return a.parent === b.parent ? base : base * 1.4;
    });

  const outRoot = layout(d3.hierarchy(buildTree(sel.id, outParent, outOrder)));
  // Incoming siblings come from different callers, so each caller's
  // call order doesn't combine into a meaningful left-to-right
  // ordering for the incoming tree — pass an empty order map and keep
  // the line-based fallback.
  const inRoot = layout(d3.hierarchy(buildTree(sel.id, inParent, new Map())));

  // Collect raw positions from d3.tree (selection at origin, outgoing
  // downward +y, incoming upward -y). The AST-style parent chain built
  // in the BFS above is used as-is: Branches render on their own row,
  // nested branches under their container. `levels` keeps
  // non-Branch-hop semantics for the direct/peer/cross classification
  // further down, but positioning follows the natural hierarchy.
  const rawPositions = new Map<string, { x: number; y: number }>();
  rawPositions.set(sel.id, { x: 0, y: 0 });
  outRoot.each((n) => {
    if (n.data.id === sel.id) return;
    rawPositions.set(n.data.id, { x: n.x, y: n.y });
  });
  inRoot.each((n) => {
    if (n.data.id === sel.id) return;
    rawPositions.set(n.data.id, { x: n.x, y: -n.y });
  });

  // Reserve a vertical band below the method for the parameter row when
  // the selection is a callable with parameters. Without this, the
  // method → callee edges pass straight through the parameter row and
  // visually "overlay" the TakesParam edges. Shifting every outgoing
  // node down by `paramReserve` opens a clear zone for params and their
  // short horizontal edges. Incoming rows are untouched.
  if (sel.kind_raw === 'Function' || sel.kind_raw === 'Method') {
    const hasParams = graph.nodes.some(
      (n) => n.kind_raw === 'Parameter' && n.parent_id === sel.original_id,
    );
    if (hasParams) {
      const paramReserve = {
        compact:  50,
        normal:   70,
        spacious: 90,
      }[args.density];
      rawPositions.forEach((pos) => {
        if (pos.y > 0) pos.y += paramReserve;
      });
    }
  }

  // --- Auto-wrap wide levels into multiple sub-rows ---
  // When a tree level has many siblings, d3.tree spreads them
  // horizontally beyond any reasonable viewport. We detect levels wider
  // than `maxRowWidth` and split them into chunks, offsetting each chunk's
  // Y by `subRowGap` so they stack neatly without overlapping the parent
  // or child level.
  const maxRowWidth = Math.max(args.vpWidth - 80, 400);
  const subRowGap = densityCfg.subRowGap;

  // Group by Y (each unique Y = one tree depth level).
  const byY = new Map<number, string[]>();
  rawPositions.forEach((pos, id) => {
    const key = Math.round(pos.y); // round to avoid float grouping issues
    if (!byY.has(key)) byY.set(key, []);
    byY.get(key)!.push(id);
  });

  byY.forEach((ids, yKey) => {
    if (ids.length <= 1) return;
    // Sort by x so chunks are left-to-right.
    ids.sort((a, b) => rawPositions.get(a)!.x - rawPositions.get(b)!.x);
    const minX = rawPositions.get(ids[0])!.x;
    const maxX = rawPositions.get(ids[ids.length - 1])!.x;
    const rowWidth = maxX - minX;
    if (rowWidth <= maxRowWidth) return;

    // How many sub-rows do we need?
    const numSubRows = Math.ceil(rowWidth / maxRowWidth);
    const chunkSize = Math.ceil(ids.length / numSubRows);
    // Expand sub-rows *away* from the selection rather than centring
    // around the nominal y. Centring bled wrapped rows back toward the
    // method at y=0, where they collided with the parameter row and
    // with the selection itself. Outgoing rows now grow downward only,
    // incoming rows grow upward only.
    const totalSubRowHeight = (numSubRows - 1) * subRowGap;
    const baseY = yKey >= 0 ? yKey : yKey - totalSubRowHeight;

    for (let row = 0; row < numSubRows; row++) {
      const chunk = ids.slice(row * chunkSize, (row + 1) * chunkSize);
      if (chunk.length === 0) continue;
      // Re-centre this chunk around x=0.
      const chunkMinX = rawPositions.get(chunk[0])!.x;
      const chunkMaxX = rawPositions.get(chunk[chunk.length - 1])!.x;
      const chunkCenterX = (chunkMinX + chunkMaxX) / 2;
      const offsetY = baseY + row * subRowGap;
      for (const id of chunk) {
        const pos = rawPositions.get(id)!;
        pos.x = pos.x - chunkCenterX;
        pos.y = offsetY;
      }
    }
  });

  // Copy to final positions map.
  rawPositions.forEach((pos, id) => positions.set(id, pos));

  return { positions, levels, nodeIds };
}

function compute(args: ComputeArgs): DisplayPlan {
  const { graph, selected, viewMode } = args;
  if (graph.nodes.length === 0) {
    console.log('[displayPlan] compute: empty graph → empty plan');
    return emptyPlan();
  }

  // A spec entity selected while the split view is open is *not* a selection
  // as far as this pane is concerned. Clicking in the spec pane sets the
  // global selection — the Details panel is shared — and both of the things
  // `selectionInGraph` gates would then be wrong here: tree mode would re-root
  // the code canvas on a Feature and redraw the spec hierarchy it was just
  // told not to draw, and force mode would restrict the canvas to the BFS
  // reach of a node that has no code edges, emptying it. The cross-filter the
  // same click applied is the answer the user actually asked for.
  const specIsSelected = args.splitView && !!selected && isSpecNode(selected);
  const selectionInGraph =
    selected && !specIsSelected ? graph.nodes.some((n) => n.id === selected.id) : false;
  const wantsTree = viewMode === 'tree' && selectionInGraph;
  console.log(`[displayPlan] compute: nodes=${graph.nodes.length} links=${graph.links.length} viewMode=${viewMode} selected=${selected?.id ?? 'null'} selectionInGraph=${selectionInGraph} wantsTree=${wantsTree} kinds=${[...new Set(graph.nodes.map(n=>n.kind_raw))]} filterKinds=${[...args.kinds]}`);

  // --- Helper: inject Parameter nodes when a callable is selected ---
  // Parameters are children of the method, connected by TakesParam edges.
  // They're included in the graph data but normally hidden by the kind
  // filter. When the selection is a callable (Function/Method), we add
  // its parameters to the visible set so they appear around the method.
  const CALLABLE_KINDS = new Set(['Function', 'Method']);
  const injectParams = (
    visibleNodeIds: Set<string>,
    visibleLinkKeys: Set<string>,
    treePositions?: Map<string, { x: number; y: number }>,
    levels?: Map<string, number>,
  ) => {
    if (!selectionInGraph || !selected || !CALLABLE_KINDS.has(selected.kind_raw)) return;
    // Find parameter nodes whose parent_id matches the selection.
    const selId = selected.id;
    const params = graph.nodes.filter(
      (n) => n.kind_raw === 'Parameter' && n.parent_id === selected.original_id,
    );
    if (params.length === 0) return;

    // Put parameters in a vertical column to the right of the method.
    // Before this, params sat in a horizontal row directly below the
    // method — and when any callee happened to land near x=0 (a very
    // common case when the method has a single child), the downward
    // `Calls` edge passed exactly on top of the downward `TakesParam`
    // edge to the middle parameter. Putting params off-axis makes the
    // two edge families permanently distinct: TakesParam edges now run
    // horizontally from the method while Calls edges still run
    // vertically, so they cannot share a line.
    const paramLayout = {
      compact:  { colOffset: 110, rowGap: 22 },
      normal:   { colOffset: 150, rowGap: 28 },
      spacious: { colOffset: 200, rowGap: 36 },
    }[args.density];

    for (const p of params) {
      visibleNodeIds.add(p.id);
      if (levels) levels.set(p.id, 1);
      if (treePositions) {
        const selPos = treePositions.get(selId);
        const idx = params.indexOf(p);
        // Centre the column vertically on the method, then stack
        // params downward: first param slightly above the method line,
        // last slightly below, so the column reads top-to-bottom.
        const totalHeight = (params.length - 1) * paramLayout.rowGap;
        const x = (selPos?.x ?? 0) + paramLayout.colOffset;
        const y = (selPos?.y ?? 0) - totalHeight / 2 + idx * paramLayout.rowGap;
        treePositions.set(p.id, { x, y });
      }
    }
    // Add TakesParam edges
    for (const l of graph.links) {
      const s = sourceIdOf(l), t = targetIdOf(l);
      if (visibleNodeIds.has(s) && visibleNodeIds.has(t)) {
        if (l.kind_raw === 'TakesParam') {
          visibleLinkKeys.add(linkKey(s, t, l.kind_raw, l.order));
        }
      }
    }
  };

  // --- Helper: inject class-field Variable nodes when a class is
  // selected. Mirrors `injectParams` but for fields on a class, placed
  // as a vertical column to the LEFT of the class so the structural
  // declarations sit opposite the methods. Methods still render below
  // as normal tree children.
  const CONTAINER_KINDS = new Set([
    'Class', 'Dataclass', 'AbstractClass', 'Struct',
    'Interface', 'Trait', 'Enum',
  ]);
  const injectClassFields = (
    visibleNodeIds: Set<string>,
    visibleLinkKeys: Set<string>,
    treePositions?: Map<string, { x: number; y: number }>,
    levels?: Map<string, number>,
  ) => {
    if (!selectionInGraph || !selected || !CONTAINER_KINDS.has(selected.kind_raw)) return;
    const selId = selected.id;
    // Fields: the analyzer-synthesised `class_field`-tagged Variables
    // (Python, etc.) plus real Property entities emitted directly by
    // parsers that model class fields as Property (Java, Kotlin, TS).
    const fields = graph.nodes.filter(
      (n) =>
        n.parent_id === selected.original_id &&
        ((n.kind_raw === 'Variable' && n.tags?.includes('class_field')) ||
          n.kind_raw === 'Property'),
    );
    if (fields.length === 0) return;

    const fieldLayout = {
      compact:  { colOffset: 110, rowGap: 22 },
      normal:   { colOffset: 150, rowGap: 28 },
      spacious: { colOffset: 200, rowGap: 36 },
    }[args.density];

    for (const f of fields) {
      visibleNodeIds.add(f.id);
      if (levels) levels.set(f.id, 1);
      if (treePositions) {
        const selPos = treePositions.get(selId);
        const idx = fields.indexOf(f);
        const totalHeight = (fields.length - 1) * fieldLayout.rowGap;
        // Negative x puts the column to the LEFT of the class.
        const x = (selPos?.x ?? 0) - fieldLayout.colOffset;
        const y = (selPos?.y ?? 0) - totalHeight / 2 + idx * fieldLayout.rowGap;
        treePositions.set(f.id, { x, y });
      }
    }
    // Add the class → field Contains edges so the column is visually
    // connected to the class.
    for (const l of graph.links) {
      const s = sourceIdOf(l), t = targetIdOf(l);
      if (!visibleNodeIds.has(s) || !visibleNodeIds.has(t)) continue;
      if (l.kind_raw !== 'Contains') continue;
      // Only the class→field direction; other Contains (class→method)
      // are already included via the normal BFS.
      const tgt = graph.nodes.find((n) => n.id === t);
      if (!tgt) continue;
      const isField =
        tgt.tags?.includes('class_field') || tgt.kind_raw === 'Property';
      if (!isField) continue;
      visibleLinkKeys.add(linkKey(s, t, l.kind_raw, l.order));
    }
  };

  if (wantsTree) {
    const { positions, levels, nodeIds } = computeTreePositions(graph, args);
    // Edge classification mirrors the old applyTreeLayout logic so the
    // direct / same-level / cross-level visibility toggles still apply.
    const nodeKind = new Map(graph.nodes.map((n) => [n.id, n.kind_raw]));
    const visibleLinkKeys = new Set<string>();
    for (const l of graph.links) {
      const s = sourceIdOf(l), t = targetIdOf(l);
      if (!nodeIds.has(s) || !nodeIds.has(t)) {
        continue;
      }
      const sLevel = levels.get(s)!;
      const tLevel = levels.get(t)!;
      const linkLevel = Math.max(sLevel, tLevel);
      const involvesBranch =
        nodeKind.get(s) === 'Branch' || nodeKind.get(t) === 'Branch' ||
        nodeKind.get(s) === 'Loop'   || nodeKind.get(t) === 'Loop';
      const isDirect = sLevel === 0 || tLevel === 0;
      const isSameLevel = sLevel === tLevel && sLevel > 0;
      const isCrossLevel = !isDirect && !isSameLevel;
      if (isDirect && !args.showDirect) {
        continue;
      }
      if (!involvesBranch && isSameLevel && args.lo[linkLevel]?.peerEdges === false) {
        continue;
      }
      if (!involvesBranch && isCrossLevel && !args.showCross) {
        continue;
      }
      visibleLinkKeys.add(linkKey(s, t, l.kind_raw, l.order));
    }
    // Inject parameter nodes for the selected callable.
    injectParams(nodeIds, visibleLinkKeys, positions, levels);
    // Inject class-field Variables as a left-side column for a class.
    injectClassFields(nodeIds, visibleLinkKeys, positions, levels);
    return {
      mode: 'tree',
      visibleNodeIds: nodeIds,
      visibleLinkKeys,
      selectedId: selected!.id,
      treePositions: positions,
      nodeDistances: levels,
      searchMatched: args.searchMatched,
      searchNeighbors: args.searchNeighbors,
      demotedHubIds: new Set(),
    dimmedNodeIds: new Set(),
      dimOpacity: 0,
      overflow: null,
    };
  }

  // Force mode.
  const filterOk = new Set<string>();
  const dimmedIds = new Set<string>();
  for (const n of graph.nodes) {
    const result = nodePassesFilters(n, args);
    if (result === 'visible') filterOk.add(n.id);
    else if (result === 'dimmed') dimmedIds.add(n.id);
  }

  let nodeDistances: Map<string, number> | null = null;
  let visibleNodeIds = filterOk;
  if (selectionInGraph) {
    nodeDistances = bfsDistances(graph, selected!.id, args);
    // Restrict to nodes that pass filters AND fall within the BFS reach.
    const restricted = new Set<string>();
    for (const id of nodeDistances.keys()) {
      if (filterOk.has(id) || id === selected!.id) restricted.add(id);
    }
    visibleNodeIds = restricted;
  }

  // UI-056. Ranked over what is actually drawn, not over the repo: whether a
  // node is a hub is a property of the view the reader chose, and a rank
  // taken from the whole dataset would demote nothing on a narrow scope.
  const demotedHubIds = args.demoteHubs
    ? new Set(rankHubs(graph.nodes.filter((n) => visibleNodeIds.has(n.id)), args.hubCount))
    : new Set<string>();

  const visibleLinkKeys = new Set<string>();
  const maxLevel = getMaxLevel(args.lo, args.maxDepth);
  const searchActive = args.searchMatched.size > 0;
  const nodeKind = new Map(graph.nodes.map((n) => [n.id, n.kind_raw]));
  for (const l of graph.links) {
    const s = sourceIdOf(l), t = targetIdOf(l);
    if (!visibleNodeIds.has(s) || !visibleNodeIds.has(t)) continue;
    if (!args.rels.has(l.kind_raw)) continue;
    // Suppress edges *into* a demoted hub. Inbound is what makes a utility
    // module unreadable — everything points at it — while its own outgoing
    // edges are few and carry real information. The node stays drawn, so the
    // reader can still select it and read its true fan-in in the panel.
    if (demotedHubIds.has(t) && !demotedHubIds.has(s)) continue;
    if (searchActive && !args.searchMatched.has(s) && !args.searchMatched.has(t)) continue;

    if (selectionInGraph && nodeDistances) {
      const sd = nodeDistances.get(s);
      const td = nodeDistances.get(t);
      if (sd === undefined || td === undefined) continue;
      const linkLevel = Math.max(sd, td);
      if (linkLevel < 1) continue;
      if (linkLevel > maxLevel) continue;
      const cfg = args.lo[linkLevel];
      if (!cfg?.enabled) continue;
      if (!resolveRelType(args.lo, linkLevel, l.kind_raw, args.rels)) continue;
      // Edges that touch a synthetic scope node (Branch or Loop) are
      // structural scaffolding, not peer/cross-level program
      // relationships — exempt them from those filters so toggling
      // peer/cross-edges off doesn't sever the caller's link to its
      // contained scopes.
      const involvesBranch =
        nodeKind.get(s) === 'Branch' || nodeKind.get(t) === 'Branch' ||
        nodeKind.get(s) === 'Loop'   || nodeKind.get(t) === 'Loop';
      const isDirect = sd === 0 || td === 0;
      const isSameLevel = sd === td && sd > 0;
      const isCrossLevel = !isDirect && !isSameLevel;
      if (isDirect && !args.showDirect) continue;
      if (!involvesBranch && isSameLevel && cfg.peerEdges === false) continue;
      if (!involvesBranch && isCrossLevel && !args.showCross) continue;
    }

    visibleLinkKeys.add(linkKey(s, t, l.kind_raw, l.order));
  }

  // Inject parameter nodes for the selected callable.
  injectParams(visibleNodeIds, visibleLinkKeys);
  // Inject class-field Variables (force mode: only add to visible set;
  // positions come from d3 simulation).
  injectClassFields(visibleNodeIds, visibleLinkKeys);

  return {
    mode: 'force',
    visibleNodeIds,
    visibleLinkKeys,
    demotedHubIds,
    selectedId: selectionInGraph ? selected!.id : null,
    treePositions: new Map(),
    nodeDistances,
    searchMatched: args.searchMatched,
    searchNeighbors: args.searchNeighbors,
    dimmedNodeIds: dimmedIds,
    // Search dimming and diff dimming have different defaults; when both are
    // in play the more visible wins so neither can black out the other's set.
    dimOpacity: args.searchMatched.size > 0 && !args.searchHides
      ? Math.max(args.searchDim, args.diffDim)
      : args.diffDim,
    overflow: null,
    };
}

export const displayPlan: Readable<DisplayPlan> = derived(
  [
    graphData, selectedNode, viewMode,
    generalEntityTypes, generalRelTypes, generalLanguages, visibleFiles,
    generalOutgoing, generalIncoming, levelOverrides,
    showDirectEdges, showCrossLevelEdges,
    searchMatchIds, searchNeighborIds,
    viewportWidth, treeDensity, treeMaxDepth,
    diffActive, diffChangesOnly, diffCoreOnly, diffFiltersEnabled, diffStatusMap, diffSourceChangedMap, diffScopeChanges,
    showGhostNodes, showBuiltinGhosts, showTemplateVars,
    searchHidesNonMatches, searchDimOpacity, diffDimOpacity,
    demoteHubs, hubCount,
    splitViewOpen, crossFilterPaths,
  ],
  ([$g, $sel, $vm, $kinds, $rels, $langs, $files, $genOut, $genIn, $lo, $sde, $scle, $sm, $sn, $vpW, $den, $dep, $diffAct, $diffCO, $diffCore, $diffFilt, $diffStat, $diffSrc, $diffScopes, $ghosts, $builtinGhosts, $templateVars, $searchHides, $searchDim, $diffDim, $demoteHubs, $hubCount, $splitView, $crossPaths]) => {
    // The plan is always computed. The render gate is applied to its result
    // rather than in front of it (UI-061): every filter above narrows
    // `visibleNodeIds`, so gating on that count is what makes filtering a
    // way out of the overflow card instead of a no-op behind it.
    return gateByDrawCeiling(compute({
      graph: $g,
      selected: $sel,
      viewMode: $vm,
      kinds: $kinds,
      rels: $rels,
      langs: $langs,
      files: $files,
      demoteHubs: $demoteHubs,
      hubCount: $hubCount,
      generalOut: $genOut,
      generalIn: $genIn,
      lo: $lo,
      showDirect: $sde,
      showCross: $scle,
      searchMatched: $sm,
      searchNeighbors: $sn,
      vpWidth: $vpW,
      density: $den,
      maxDepth: $dep,
      // Diff filters are only effective when the master toggle is ON.
      diffIsActive: $diffAct && $diffFilt,
      diffChangesOnly: $diffCO,
      diffCoreOnly: $diffCore,
      diffStatuses: $diffStat,
      diffSourceChanged: $diffSrc,
      diffScopes: $diffScopes,
      showGhosts: $ghosts,
      showBuiltinGhosts: $builtinGhosts,
      showTemplateVars: $templateVars,
      searchHides: $searchHides,
      searchDim: $searchDim,
      diffDim: $diffDim,
      splitView: $splitView,
      crossFilterPaths: $crossPaths,
    }));
  },
);

/** Helper for the View — given a link, compute the same key the plan uses. */
export function linkKeyFor(l: D3Link): string {
  const s = typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
  const t = typeof l.target === 'object' ? (l.target as D3Node).id : l.target;
  return linkKey(s, t, l.kind_raw, l.order);
}

/** Ids of nodes currently on screen. Derived from displayPlan so no code
 *  path writes it manually. Exposed here (not in filterViewModel) to keep
 *  the import dependency acyclic: anything that needs "what's visible
 *  right now" imports it from this module. */
export const visibleNodeIds: Readable<Set<string>> = derived(
  displayPlan,
  ($plan) => $plan.visibleNodeIds,
);

// --- Display search (Ctrl+F within the current view) ---
//
// Lives here (not in filterViewModel) because it depends on
// `visibleNodeIds` — which is itself derived from displayPlan. Putting it
// in filterViewModel and re-importing displayPlan would reintroduce the
// startup-time module cycle that caused TDZ ReferenceErrors.
import { writable } from 'svelte/store';

export const displaySearchTerm = writable<string>('');

export const displaySearchMatches: Readable<D3Node[]> = derived(
  [graphData, visibleNodeIds, displaySearchTerm],
  ([$data, $visible, $term]) => {
    const s = $term.toLowerCase().trim();
    if (!s || $visible.size === 0) return [] as D3Node[];
    return $data.nodes.filter(
      (n) =>
        $visible.has(n.id) && (
          n.name.toLowerCase().includes(s) ||
          n.qualified_name.toLowerCase().includes(s) ||
          n.file_path.toLowerCase().includes(s)
        ),
    );
  },
);

export const displaySearchMatchIds: Readable<Set<string>> = derived(
  displaySearchMatches,
  ($matches) => new Set($matches.map((n) => n.id)),
);
