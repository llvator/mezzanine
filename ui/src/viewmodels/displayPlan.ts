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
import type { D3Node, D3Link, FolderPicture, GraphData, ViewMode, LevelOverrides, TriState } from '../types/graph';
import { isSpecNode } from '../types/graph';
import { pathsClaim } from '../utils/refPaths';
import { pickedHighlight } from '../utils/rowPicks';
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
  structureOnly,
  shapePicture,
  type TreeDensity,
} from '../stores/graph';
import { bodyHidden, linkDrawable, linkTraversable } from './bodyScope';
import { shapeEdgeVerdicts, shapePlacement, verdictFor } from './shapeView';
import { searchMatchIds, searchNeighborIds } from './filterViewModel';
import { diffActive, diffLevel, diffSeedFacet, diffFiltersEnabled, diffStatusMap, diffSourceChangedMap, diffScopeChanges, diffChangedEdges, diffAddedEntityIds, diffDimOpacity, diffContextOpacity, diffHeadIsWorking, normalizeEntityId, type ChangeStatus } from '../stores/diff';
import type { ScopeChange } from './diffRollup';
import { editKind, type DiffFacts } from './diffVerdict';
import { linkTier, planDiffLevel, splitEdits, type DiffLevel, type DiffSeedFacet, type EditKind, type LevelEdge } from './diffLevels';
import { drawnNodeSet, gateByDrawCeiling, type DrawOverflow } from './drawCeiling';
import { rankHubs } from './hubs';
import { flowFields, type FlowRung } from './flowPlacement';
import { carriesFlux } from './scopeFlow';
import type { FlowEdge } from './flowLayers';
import { demoteHubs, hubCount } from '../stores/settings';
import { crossFilterPaths } from '../stores/crossFilter';
import { splitViewOpen } from '../stores/panes';

export interface DisplayPlan {
  /** 'tree' when viewMode === 'tree' AND a selection exists in the
   *  current graph, 'shape' when viewMode === 'shape' AND a folder picture
   *  is in hand, otherwise 'force'. The View uses this to decide between
   *  pinning nodes to computed positions vs. running the simulation.
   *
   *  'shape' and 'tree' share the whole pinning path — both are laid out
   *  rather than simulated, and the only difference is where the positions
   *  come from. What makes 'shape' its own mode rather than a second source
   *  of tree positions is what it means: a tree is a reach around a
   *  selection, a shape is a claim about one folder's structure, and the
   *  edges carry a verdict in the second case and not the first.
   *
   *  'flow' is the fourth, and the only one that is a pure re-layout: it draws
   *  exactly what force mode drew, in dependency layers. It shares the pinning
   *  path with the other two — positions in `treePositions`, nothing
   *  simulated — and adds `flowAxis`, which is the part the reader needs and
   *  no pixel can carry: which end is upstream. */
  mode: 'force' | 'tree' | 'shape' | 'flow';
  /** Ids of nodes that should be visible in the current view. Includes
   *  filter, search, and selection-distance gating. */
  visibleNodeIds: Set<string>;
  /** Stable per-link identifiers for visible links. Format: `${src}->${tgt}|${kind}` */
  visibleLinkKeys: Set<string>;
  /**
   * The Rest tier's wiring — lines that run between drawn nodes but that the
   * view did not choose (UI-144). Drawn at `dimOpacity`, like the nodes.
   *
   * The counterpart to `dimmedNodeIds`, and it exists for the same reason: the
   * Rest slider is how a reader asks "what else is there", and an answer made
   * of unconnected circles is a census, not a shape. Two kinds of line land
   * here — one with an end in `dimmedNodeIds`, and one between two visible
   * nodes that a rung below `neighbourhood` declined to draw because it had
   * not changed. Both are untouched wiring, which is exactly what this tier is.
   *
   * Empty when `dimOpacity` is 0, because then the Rest tier is not on screen
   * and naming its lines would have the View build DOM for nothing.
   */
  dimmedLinkKeys: Set<string>;
  /** Selected node id, or null. Plain pass-through for the .selected class. */
  selectedId: string | null;
  /** Tree-mode positions, keyed by node id. Empty in force mode. Flow mode
   *  fills the same map — the View's pinning path is shared. */
  treePositions: Map<string, { x: number; y: number }>;
  /**
   * One entry per dependency layer the Flow view drew, left to right (UI-146).
   * Empty in every other mode.
   *
   * Carried on the plan rather than recomputed in the View for the reason the
   * positions are: the columns and their captions have to come from the same
   * layering, or the canvas would label a column that the nodes are not in.
   */
  flowAxis: FlowRung[];
  /**
   * Nodes the Flow view found in a dependency cycle. Empty in every other
   * mode, and empty in Flow mode when the drawn graph is acyclic — which is
   * the reading worth having, so it is said by an absence of marks rather
   * than by a badge on every node.
   */
  flowCycleIds: Set<string>;
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
  /**
   * The subset of `visibleNodeIds` a diff rung recruited rather than the seed
   * (UI-112) — drawn, but drawn at `contextOpacity`.
   *
   * A *subset*, deliberately: every other consumer of the plan — the draw
   * ceiling, the empty-canvas verdict, the layout — should keep treating
   * these as the drawn nodes they are. The only thing this changes is how
   * loudly they are drawn, which is why nothing but the View reads it.
   */
  contextNodeIds: Set<string>;
  /** Opacity for `contextNodeIds`. 1 when no diff rung is recruiting, which
   *  is the picture as it was before this existed. */
  contextOpacity: number;
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

/**
 * The lines the Flow layout is derived from (UI-146).
 *
 * Read back off the key sets rather than collected in the edge loop, and that
 * is a deliberate trade: one extra pass over `graph.links`, paid only when the
 * Flow view is on, against three branches inside `compute` — a function that
 * is already the largest in the codebase and should not grow by three for a
 * mode it spends most of its life not being in.
 *
 * Both tiers feed it. The Rest tier's wiring is drawn, so a layout that
 * ignored it would route lines across columns the reader can see are wrong.
 * Structural edges are left out — see `carriesFlux`. A `Contains` line does
 * not make a class stand on its own methods.
 */
function fluxEdgesOf(
  links: readonly D3Link[],
  visible: ReadonlySet<string>,
  dimmed: ReadonlySet<string>,
): FlowEdge[] {
  const out: FlowEdge[] = [];
  for (const l of links) {
    if (!carriesFlux(l.kind_raw)) continue;
    const source = sourceIdOf(l);
    const target = targetIdOf(l);
    const key = linkKey(source, target, l.kind_raw, l.order);
    if (!visible.has(key) && !dimmed.has(key)) continue;
    out.push({ source, target });
  }
  return out;
}

/** Is a committed search contributing to the dim tier? Only when one is
 *  running and it was set to fade non-matches rather than remove them. */
function searchDims(args: { searchMatched: Set<string>; searchHides: boolean }): boolean {
  return args.searchMatched.size > 0 && !args.searchHides;
}

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
  /** How strongly the nodes a rung recruited are drawn against the edits
   *  (UI-112). Only reaches the plan when the ladder is actually filtering. */
  diffContext: number;
  /** How wide the diff draws. Only consulted when `diffFiltersOn`. */
  diffLevel: DiffLevel;
  /** Which half of the change the ladder starts from (UI-109). `all` is the
   *  whole seed, and the picture the ladder drew before this existed. */
  diffSeedFacet: DiffSeedFacet;
  /** Master toggle — off means a diff can be loaded and coloured while the
   *  ladder has no effect on what is drawn. */
  diffFiltersOn: boolean;
  diffStatuses: Map<string, ChangeStatus>;
  diffSourceChanged: Map<string, boolean>;
  /** `${src}->${tgt}|${kind}` for every edge the diff reported as appeared. */
  diffAddedEdges: Set<string>;
  /** Normalized ids of added entities, whose edges carry no delta of their
   *  own but are new all the same. */
  diffAddedEntities: Set<string>;
  /** Scope path → rolled-up change, for the collapsed levels where a node's
   *  `original_id` is a path rather than an entity id (UI-064). */
  diffScopes: Map<string, ScopeChange>;
  /** Whether the diff's head is the tree being drawn. Decides what the diff's
   *  *silence* about a file means — see `diffVerdict` (UI-104). */
  diffHeadIsWorking: boolean;
  showGhosts: boolean;
  /** Independent toggle for `ghost_stdlib`-tagged ghosts. When false,
   *  those ghosts are hidden even if `showGhosts` is true. */
  showBuiltinGhosts: boolean;
  /** Toggle for the `template_var`-tagged ansible templating layer.
   *  When false (default), those nodes and their edges are hidden. */
  showTemplateVars: boolean;
  /** Draw only what a file declares, not what its functions do (UI-113).
   *  On by default. Reads the `body_of` stamp `liftBodies` put on every
   *  entity enclosed by a callable. */
  structureOnly: boolean;
  /** `original_id` of the callable whose body is exempt from the filter —
   *  the current selection, when there is one.
   *
   *  This is the gesture the whole filter is built around: a canvas opens
   *  with declarations, and picking one thing to read opens that one thing.
   *  It is a single equality because `body_of` names the *outermost*
   *  enclosure, so a call five branch arms deep in the selected function
   *  carries the same value its parameters do — one selection, one whole
   *  body, no walk. */
  exemptBody: string | null;
  /** Split view: the Elevator layer draws in its own pane, so it comes out
   *  of this one (ADR 0011). */
  splitView: boolean;
  /** Code paths the focused spec entity claims, or null when no cross-filter
   *  is active. `[]` is a real value meaning "claims nothing" — see
   *  `stores/crossFilter.ts`. */
  crossFilterPaths: string[] | null;
  /** The folder picture, when the shape view is on AND one has arrived.
   *  Null in every other case, including while a fetch is in flight — the
   *  canvas keeps drawing what it had rather than blanking (UI-108). */
  picture: FolderPicture | null;
}

function emptyPlan(): DisplayPlan {
  return {
    mode: 'force',
    visibleNodeIds: new Set(),
    visibleLinkKeys: new Set(),
    dimmedLinkKeys: new Set(),
    selectedId: null,
    treePositions: new Map(),
    flowAxis: [],
    flowCycleIds: new Set(),
    nodeDistances: null,
    searchMatched: new Set(),
    searchNeighbors: new Set(),
    demotedHubIds: new Set(),
    dimmedNodeIds: new Set(),
    dimOpacity: 0,
    contextNodeIds: new Set(),
    contextOpacity: 1,
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
  // A function's internals are not what the file declares. Hard, and above
  // the kind filter, because the two are answering different questions: the
  // kind checkboxes cannot tell a module's entry point from a closure three
  // callbacks down, and this cannot tell a Branch from a Loop.
  if (bodyHidden(n, args)) return 'hidden';
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
  // The diff is deliberately absent here. It used to be one more predicate on
  // a single node, which is why it could only ever filter nodes; the rungs
  // need the whole graph at once (a far end, a neighbour), so they run as
  // their own pass over the survivors — see `applyDiffLevel`.
  return 'visible';
}

/** The diff lookups `diffVerdict` needs, gathered off `ComputeArgs`. Cheap —
 *  four field reads, no copying — so it is built per call rather than
 *  threaded through as a fifth shape. */
function diffFactsOf(args: ComputeArgs): DiffFacts {
  return {
    statuses: args.diffStatuses,
    sourceChanged: args.diffSourceChanged,
    scopes: args.diffScopes,
    headIsWorking: args.diffHeadIsWorking,
  };
}

/**
 * Did the diff report this link as new?
 *
 * Two sources, because the engine deliberately reports only one of them.
 * `rel_deltas` covers edges on entities that exist on both sides. An *added*
 * entity carries none — every edge it has is new by construction, and UI-086
 * leaves them off the list so they don't bury the deltas that carry
 * information. That reasoning holds for a list and fails for a canvas, so any
 * head edge touching an added entity counts as added here.
 *
 * Two key shapes, because the graph changes under aggregation. At entity
 * level an edge is `(src, tgt, kind)`. Above it, `collapseGraph` merges every
 * edge between two scopes into one line relabelled `DependsOn`, so the kind
 * is gone and the honest question becomes "did anything between these two
 * scopes move".
 *
 * Known under-report above entity level: a collapsed line is *not* marked
 * changed when the only new edges behind it belong to an added entity. Those
 * edges have no delta to roll up and the collapsed graph does not carry its
 * constituent edges, so there is nothing to read. It errs toward drawing
 * fewer lines than changed, never more.
 */
function isChangedLink(
  l: D3Link,
  src: string,
  tgt: string,
  orig: Map<string, string>,
  args: ComputeArgs,
): boolean {
  const s = orig.get(src);
  const t = orig.get(tgt);
  if (s === undefined || t === undefined) return false;
  if (args.diffAddedEntities.has(s) || args.diffAddedEntities.has(t)) return true;
  return args.diffAddedEdges.has(`${s}->${t}|${l.kind_raw}`)
    || args.diffAddedEdges.has(`${s}->${t}`);
}

/** What one rung of the ladder draws, over the nodes that already passed
 *  every other filter. */
interface DiffLevelResult {
  visible: Set<string>;
  /** Of `visible`, what the rung recruited rather than the seed (UI-112). */
  context: Set<string>;
  dimmed: Set<string>;
  /** Link keys the diff reported as new — the whole edge budget at `edits`
   *  and `rewiring`, and merely decorative at `neighbourhood`. */
  changedLinkKeys: Set<string>;
  changedEdgesOnly: boolean;
}

/**
 * Run the diff ladder over `candidates` (UI-088).
 *
 * The rungs need the graph, not one node at a time — `rewiring` asks for the
 * far end of a changed edge and `neighbourhood` for anything one hop out —
 * which is why this is a pass of its own rather than another clause in
 * `nodePassesFilters`. The membership decision itself lives in `diffLevels`,
 * where it can be tested without a graph; what happens here is the
 * translation from nodes and links into the ids that module speaks.
 */
function applyDiffLevel(
  graph: GraphData,
  candidates: Set<string>,
  args: ComputeArgs,
): DiffLevelResult {
  const orig = new Map(graph.nodes.map((n) => [n.id, normalizeEntityId(n.original_id)]));
  const changedLinkKeys = new Set<string>();
  const levelEdges: LevelEdge[] = [];
  for (const l of graph.links) {
    const src = sourceIdOf(l);
    const tgt = targetIdOf(l);
    const changed = isChangedLink(l, src, tgt, orig, args);
    if (changed) changedLinkKeys.add(linkKey(src, tgt, l.kind_raw, l.order));
    levelEdges.push({ src, tgt, changed });
  }

  // The seed, split by the facet on the way in (UI-109). Narrowing it here
  // rather than after the ladder is what makes `new` + `neighbourhood` mean
  // "what the new code plugs into": every rung grows the seed, so a seed of
  // new entities recruits their context and keeps it. Filtering the drawn set
  // instead would delete half of that context back out, and the far end of a
  // changed edge has no facet of its own to be judged by.
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const facts = diffFactsOf(args);
  const edits = new Map<string, EditKind>();
  for (const id of candidates) {
    const n = byId.get(id);
    if (!n) continue;
    const kind = editKind(n, facts);
    if (kind) edits.set(id, kind);
  }

  const { seed, excluded } = splitEdits(args.diffSeedFacet, edits);
  const plan = planDiffLevel(args.diffLevel, candidates, seed, levelEdges, excluded);
  return { ...plan, changedLinkKeys };
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
        if (!linkTraversable(link, args)) return;
        if (!resolveRelType(args.lo, lvl, link.kind_raw, args.rels)) return;
        const node = nodeById.get(neighborId);
        if (!node) return;
        if (bodyHidden(node, args)) return;
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
        if (!linkTraversable(l, args)) {
          continue;
        }
        if (!resolveRelType(args.lo, lvl, l.kind_raw, args.rels)) {
          continue;
        }
        const node = nodeById.get(tgt);
        if (!node) {
          continue;
        }
        if (bodyHidden(node, args)) {
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
        if (!linkTraversable(l, args)) continue;
        if (!resolveRelType(args.lo, lvl, l.kind_raw, args.rels)) continue;
        const node = nodeById.get(src);
        if (!node) continue;
        if (bodyHidden(node, args)) continue;
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
      if (l.lifted_from) continue;
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
      if (l.lifted_from) continue;
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

  // The shape view, before every filter below it.
  //
  // Nothing here narrows: `graphData` was already aggregated through the
  // picture's own resolvers, so everything on the canvas is either a child
  // of the folder or one of its one-hop neighbours, and the kind, language
  // and file filters have nothing left to say about a set of nine circles
  // the reader asked for by name. Running them anyway is how a reader who
  // unticked `File` in some earlier scope opens a folder and sees nothing.
  if (args.picture) {
    const { positions } = shapePlacement(args.picture);
    const byPath = new Map(graph.nodes.map((n) => [n.original_id, n]));
    const shapePositions = new Map<string, { x: number; y: number }>();
    for (const [path, pos] of positions) {
      const node = byPath.get(path);
      if (node) shapePositions.set(node.id, pos);
    }
    // A node the placement does not name would sit at the origin, under
    // whatever is drawn there — so it is dropped rather than piled up.
    const nodeIds = new Set(shapePositions.keys());
    // Only the edges the picture has a reading for. Two outsiders that
    // depend on each other are both on screen and the line between them is
    // real, but it is not about this folder — the picture describes the
    // child graph and the boundary traffic, and nothing else. Measured on
    // `src/mcp`: 43 such lines against 31 that carry a verdict, so left in
    // they would be most of the drawing, in a colour that means nothing,
    // crossing the part that does. That is the entanglement this view
    // exists to get out of.
    const verdicts = shapeEdgeVerdicts(args.picture);
    const pathOf = new Map(graph.nodes.map((n) => [n.id, n.original_id]));
    const visibleLinkKeys = new Set<string>();
    for (const l of graph.links) {
      const s = sourceIdOf(l), t = targetIdOf(l);
      if (!nodeIds.has(s) || !nodeIds.has(t)) continue;
      if (!verdictFor(verdicts, pathOf.get(s) ?? '', pathOf.get(t) ?? '')) continue;
      visibleLinkKeys.add(linkKey(s, t, l.kind_raw, l.order));
    }
    return {
      ...emptyPlan(),
      mode: 'shape',
      visibleNodeIds: nodeIds,
      visibleLinkKeys,
      selectedId: selectionInGraph ? selected!.id : null,
      treePositions: shapePositions,
      searchMatched: args.searchMatched,
      searchNeighbors: args.searchNeighbors,
    };
  }

  if (wantsTree) {
    const { positions, levels, nodeIds } = computeTreePositions(graph, args);
    // The diff ladder, over the reach the tree just computed.
    //
    // It used to run in force mode alone, and `wantsTree` is
    // `viewMode === 'tree' && selectionInGraph` — so in tree view the rungs
    // worked until the reader selected something and silently stopped the
    // moment they did. Same control, same click, opposite result, with the
    // strip above the canvas still claiming the diff was filtering.
    //
    // The reach is the candidate set, exactly as `filterOk` is in force mode,
    // so the ladder can only ever narrow what the tree already drew — a node
    // outside the selection's hops cannot reappear because a changed edge
    // points at it. The root is exempt: a tree is rooted at its selection, and
    // dropping that node because it happens not to be an edit leaves a layout
    // with nothing at the origin.
    let treeVisible = nodeIds;
    const treeDimmed = new Set<string>();
    const treeContext = new Set<string>();
    let treeChangedLinks: Set<string> | null = null;
    if (args.diffIsActive && args.diffFiltersOn) {
      const level = applyDiffLevel(graph, nodeIds, args);
      treeVisible = level.visible;
      treeVisible.add(selected!.id);
      for (const id of level.dimmed) {
        if (id !== selected!.id) treeDimmed.add(id);
      }
      // The root is exempt from the context weighting for the same reason it
      // is exempt from the rung: a tree is rooted at its selection, and the
      // one node the reader pointed at is not background whatever the diff
      // says about it.
      for (const id of level.context) {
        if (id !== selected!.id) treeContext.add(id);
      }
      if (level.changedEdgesOnly) treeChangedLinks = level.changedLinkKeys;
    }
    // Edge classification mirrors the old applyTreeLayout logic so the
    // direct / same-level / cross-level visibility toggles still apply.
    const nodeKind = new Map(graph.nodes.map((n) => [n.id, n.kind_raw]));
    const visibleLinkKeys = new Set<string>();
    const dimmedLinkKeys = new Set<string>();
    // Every node this branch will put on screen — the rung's own set plus the
    // Rest tier, when the slider is off 0 and that tier is actually drawn.
    // Same definition `drawnIdsOf` uses, because a line whose end was never
    // built renders as an arrow into empty space.
    const treeDrawn = drawnNodeSet(treeVisible, treeDimmed, args.diffDim);
    for (const l of graph.links) {
      const s = sourceIdOf(l), t = targetIdOf(l);
      if (!treeDrawn.has(s) || !treeDrawn.has(t)) {
        continue;
      }
      if (!linkDrawable(l, treeVisible, args)) {
        continue;
      }
      // Two tiers rather than a gate (UI-144). Below `neighbourhood` an edge
      // has to have moved to earn a *line* — the same rule force mode applies,
      // and without it the tree drew every untouched call between two changed
      // nodes and called that a diff. What changed is where the rejects go: an
      // edge the rung declined, or one with an end in the Rest tier, is now
      // that tier's wiring instead of nothing at all.
      const key = linkKey(s, t, l.kind_raw, l.order);
      const tier = linkTier(s, t, key, treeVisible, treeChangedLinks);
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
      // Same rule as force mode: a hidden Rest tier earns no lines, so its
      // wiring cannot move the layout at the diff's default setting.
      if (tier === 'visible') visibleLinkKeys.add(key);
      else if (args.diffDim > 0) dimmedLinkKeys.add(key);
    }
    // Inject parameter nodes for the selected callable.
    injectParams(treeVisible, visibleLinkKeys, positions, levels);
    // Inject class-field Variables as a left-side column for a class.
    injectClassFields(treeVisible, visibleLinkKeys, positions, levels);
    return {
      mode: 'tree',
      visibleNodeIds: treeVisible,
      visibleLinkKeys,
      dimmedLinkKeys,
      selectedId: selected!.id,
      treePositions: positions,
      flowAxis: [],
      flowCycleIds: new Set(),
      nodeDistances: levels,
      searchMatched: args.searchMatched,
      searchNeighbors: args.searchNeighbors,
      demotedHubIds: new Set(),
      dimmedNodeIds: treeDimmed,
      // The Rest slider has to reach here too, or the ladder's only setting in
      // tree view is "gone" and there is no way to keep the rest as context.
      dimOpacity: args.diffDim,
      contextNodeIds: treeContext,
      contextOpacity: treeContext.size > 0 ? args.diffContext : 1,
      overflow: null,
    };
  }

  // Force mode.
  let filterOk = new Set<string>();
  const dimmedIds = new Set<string>();
  for (const n of graph.nodes) {
    const result = nodePassesFilters(n, args);
    if (result === 'visible') filterOk.add(n.id);
    else if (result === 'dimmed') dimmedIds.add(n.id);
  }

  // The diff ladder narrows what survived the other filters, and says whether
  // untouched wiring may be drawn between what is left (UI-088).
  let changedLinkKeys: Set<string> | null = null;
  let contextIds = new Set<string>();
  if (args.diffIsActive && args.diffFiltersOn) {
    const level = applyDiffLevel(graph, filterOk, args);
    for (const id of level.dimmed) dimmedIds.add(id);
    filterOk = level.visible;
    contextIds = level.context;
    if (level.changedEdgesOnly) changedLinkKeys = level.changedLinkKeys;
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
    // The dimmed set is subject to the selection too.
    //
    // It is built above from the *whole* graph, before the BFS exists, while
    // `visibleNodeIds` is intersected with the reach here — so the two were
    // answering the selection differently. The View draws anything dimmed once
    // `dimOpacity > 0`, which meant raising the Rest slider on a focused canvas
    // faded in the entire scope, most of it nowhere near the selection: a
    // filter the reader had applied, reappearing through a control that only
    // claims to change how visible the *rest of this view* is.
    for (const id of [...dimmedIds]) {
      if (!nodeDistances.has(id)) dimmedIds.delete(id);
    }
    // Same intersection for the context tier, for the plainer reason that a
    // node the selection removed is not drawn at all: leaving it here would
    // have the View weighting nodes that aren't on screen.
    for (const id of [...contextIds]) {
      if (!visibleNodeIds.has(id)) contextIds.delete(id);
    }
  }

  // UI-056. Ranked over what is actually drawn, not over the repo: whether a
  // node is a hub is a property of the view the reader chose, and a rank
  // taken from the whole dataset would demote nothing on a narrow scope.
  const demotedHubIds = args.demoteHubs
    ? new Set(rankHubs(graph.nodes.filter((n) => visibleNodeIds.has(n.id)), args.hubCount))
    : new Set<string>();

  // Search dimming and diff dimming have different defaults; when both are in
  // play the more visible wins so neither can black out the other's set.
  //
  // Read here rather than only at the return, because the edge loop below has
  // to know whether the Rest tier is on screen before it can decide which
  // lines belong to it.
  const dimOpacity = searchDims(args)
    ? Math.max(args.searchDim, args.diffDim)
    : args.diffDim;

  const visibleLinkKeys = new Set<string>();
  const dimmedLinkKeys = new Set<string>();
  const maxLevel = getMaxLevel(args.lo, args.maxDepth);
  const searchActive = args.searchMatched.size > 0;
  const nodeKind = new Map(graph.nodes.map((n) => [n.id, n.kind_raw]));
  // Every node the View will build — `drawnIdsOf` is the same definition, and
  // the two have to agree or the plan names a line with an end that was never
  // made. At `dimOpacity === 0` the Rest tier is hidden outright, so it
  // contributes no nodes here and earns no lines below.
  const drawnNodeIds = drawnNodeSet(visibleNodeIds, dimmedIds, dimOpacity);
  for (const l of graph.links) {
    const s = sourceIdOf(l), t = targetIdOf(l);
    if (!drawnNodeIds.has(s) || !drawnNodeIds.has(t)) continue;
    if (!linkDrawable(l, visibleNodeIds, args)) continue;
    if (!args.rels.has(l.kind_raw)) continue;
    // Two tiers rather than a gate (UI-144). Below `neighbourhood`, an edge has
    // to have moved to earn a *line*: that is the whole point of the ladder —
    // three quarters of what the old view drew was untouched wiring that merely
    // happened to run between two changed entities.
    //
    // But "not part of the diff" is what the Rest tier is *for*, and a reader
    // who raises that slider is asking for the code around the change. Getting
    // back a field of unconnected circles answers half the question. So an edge
    // the ladder declined — because it did not move, or because one end is in
    // the Rest tier — is demoted to that tier rather than dropped, and fades in
    // and out with the nodes it joins.
    const key = linkKey(s, t, l.kind_raw, l.order);
    const tier = linkTier(s, t, key, visibleNodeIds, changedLinkKeys);
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

    // The Rest tier earns keys only while it is on screen. At 0 the View sets
    // `display: none` on it, and a key it cannot draw would still cost the
    // build a DOM element and the simulation a link force — which would move
    // the layout at the diff's *default* setting, for lines nobody sees.
    if (tier === 'visible') visibleLinkKeys.add(key);
    else if (dimOpacity > 0) dimmedLinkKeys.add(key);
  }

  // Inject parameter nodes for the selected callable, and class-field
  // Variables for a selected class (force mode: only add to the visible set;
  // positions come from d3's simulation).
  //
  // Not in Flow mode. Parameters are reached by `TakesParam` and fields by
  // `Contains`, neither of which carries flux — so every injected node would
  // arrive with no edge the layout can read and pile into column 0 as an
  // isolated node, putting a method's five parameters at the upstream end of
  // the repo. The selection still opens them in Graph and Tree view, where a
  // position that is merely *near the method* is all they need.
  //
  // This `if` is the whole of what a fourth canvas mode costs this function.
  // It reads as one branch either way — moved inside the two closures it
  // guards it costs two, since they are declared here and their complexity is
  // this function's. Getting it to nothing means indexing a per-mode kind
  // table instead of asking the question, which trades a branch the ceiling
  // counts for a sentence the next reader has to decode. Not worth it.
  if (viewMode !== 'flow') {
    injectParams(visibleNodeIds, visibleLinkKeys);
    injectClassFields(visibleNodeIds, visibleLinkKeys);
  }

  return {
    // The Flow view, last, over exactly what force mode just decided to draw.
    //
    // Deliberately at the tail rather than as its own branch beside
    // `wantsTree`: Flow is not a different population, it is the same one laid
    // out by dependency depth instead of by force. Every filter, the diff
    // ladder, hub demotion and the selection reach have already run, so
    // switching modes moves the circles and changes nothing about which
    // circles there are — which is what makes it safe to flip back and forth
    // while reading.
    //
    // One call rather than four conditionals, because this function is the
    // largest in the codebase and a fourth mode should cost it as close to
    // nothing as a fourth mode can.
    ...flowFields(
      args.viewMode === 'flow',
      drawnNodeIds,
      () => fluxEdgesOf(graph.links, visibleLinkKeys, dimmedLinkKeys),
      args.density,
    ),
    visibleNodeIds,
    visibleLinkKeys,
    dimmedLinkKeys,
    demotedHubIds,
    selectedId: selectionInGraph ? selected!.id : null,
    nodeDistances,
    searchMatched: args.searchMatched,
    searchNeighbors: args.searchNeighbors,
    dimmedNodeIds: dimmedIds,
    dimOpacity,
    contextNodeIds: contextIds,
    // Nothing recruited, nothing to weight — and `1` keeps every other reason
    // a node is drawn out of this tier's reach.
    contextOpacity: contextIds.size > 0 ? args.diffContext : 1,
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
    diffActive, diffLevel, diffSeedFacet, diffFiltersEnabled, diffStatusMap, diffSourceChangedMap, diffScopeChanges,
    diffChangedEdges, diffAddedEntityIds, diffHeadIsWorking,
    showGhostNodes, showBuiltinGhosts, showTemplateVars,
    searchHidesNonMatches, searchDimOpacity, diffDimOpacity, diffContextOpacity,
    structureOnly, demoteHubs, hubCount,
    splitViewOpen, crossFilterPaths, shapePicture,
  ],
  ([$g, $sel, $vm, $kinds, $rels, $langs, $files, $genOut, $genIn, $lo, $sde, $scle, $sm, $sn, $vpW, $den, $dep, $diffAct, $diffLvl, $diffFacet, $diffFilt, $diffStat, $diffSrc, $diffScopes, $diffEdges, $diffAdded, $diffHeadWorking, $ghosts, $builtinGhosts, $templateVars, $searchHides, $searchDim, $diffDim, $diffContext, $structureOnly, $demoteHubs, $hubCount, $splitView, $crossPaths, $picture]) => {
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
      diffIsActive: $diffAct,
      // The ladder is only effective when the master toggle is ON — a diff
      // can be loaded and coloured with no filtering at all.
      diffFiltersOn: $diffFilt,
      diffLevel: $diffLvl,
      diffSeedFacet: $diffFacet,
      diffStatuses: $diffStat,
      diffSourceChanged: $diffSrc,
      diffScopes: $diffScopes,
      diffAddedEdges: $diffEdges.added,
      diffAddedEntities: $diffAdded,
      diffHeadIsWorking: $diffHeadWorking,
      showGhosts: $ghosts,
      showBuiltinGhosts: $builtinGhosts,
      showTemplateVars: $templateVars,
      structureOnly: $structureOnly,
      // The selection's own body is exempt. Read off the *store* rather than
      // off the plan's `selectionInGraph`, because the two disagree in the one
      // case that matters: a callable selected while the canvas is collapsed
      // to files is not "in the graph", and its body is not on screen to be
      // exempted either — so the value is harmless there and correct here.
      exemptBody: $sel?.original_id ?? null,
      searchHides: $searchHides,
      searchDim: $searchDim,
      diffDim: $diffDim,
      diffContext: $diffContext,
      splitView: $splitView,
      crossFilterPaths: $crossPaths,
      picture: $vm === 'shape' ? $picture : null,
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

// --- Picking within the display search ---
//
// The result list used to offer exactly one gesture: click a row, and the
// node becomes `selectedNode`. That is a graph selection — it narrows the
// canvas to the node's BFS reach, which changes `visibleNodeIds`, which is
// what this search matches against. So choosing a result rewrote the list
// it was chosen from, and a second choice replaced the first. There was no
// way to say "these three, and leave the view alone".
//
// Picking is that way. It is a view-only overlay, like the highlight it
// drives: it writes no filter, moves no camera and touches no selection, so
// the list a reader is picking from holds still while they pick.
export const displaySearchPicked = writable<Set<string>>(new Set());

/** Tick or untick a run of rows in one write, so the canvas re-marks once
 *  rather than once per row. */
export function setDisplaySearchPicks(ids: string[], picked: boolean): void {
  displaySearchPicked.update((s) => {
    const next = new Set(s);
    for (const id of ids) {
      if (picked) next.add(id);
      else next.delete(id);
    }
    return next;
  });
}

export function clearDisplaySearchPicks(): void {
  displaySearchPicked.set(new Set());
}

/** What the canvas marks: every match until the reader picks, then the
 *  picks. See `pickedHighlight` for why an empty intersection stays empty. */
export const displaySearchHighlightIds: Readable<Set<string>> = derived(
  [displaySearchMatches, displaySearchPicked],
  ([$matches, $picked]) => pickedHighlight($matches.map((n) => n.id), $picked),
);

// Emptying the box ends the search, and picks against a search nobody is
// running would silently narrow the next one. Picks survive an *edit*,
// though — the ids stay meaningful, and the intersection above drops the
// ones the new term no longer matches.
displaySearchTerm.subscribe((term) => {
  if (term.trim() === '') clearDisplaySearchPicks();
});
