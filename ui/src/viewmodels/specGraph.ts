/**
 * The Elevator layer as a graph of its own.
 *
 * Until the split view (ADR 0011) the `.elv` entities were nodes in the code
 * graph like any other, and everything that read them read them through the
 * code graph's machinery — collapse levels, metric encoding, file scoping.
 * That machinery answers questions the spec layer does not have: a Feature has
 * no cyclomatic complexity, does not collapse to a file circle usefully, and
 * is not interesting at "folder" granularity.
 *
 * So this module cuts the spec out as a standalone subgraph and answers the
 * three questions the split view asks of it:
 *
 *  - **What is the shape?** — nodes plus the `Contains` hierarchy, with a
 *    tier per kind so a layout can be tidy without a strict tree (a Feature
 *    can sit under two Categories; `d3.tree` cannot represent that, tiers can).
 *  - **What code does this entity claim?** — the union of `cr:` paths over its
 *    whole `Contains`-subtree, so clicking a Category means "everything the
 *    Features under it declare", which is the only reading that makes a
 *    behaviourless container clickable at all.
 *  - **Who claims this file?** — the reverse leg, for lighting the spec up
 *    from a code selection.
 *
 * Pure: no stores, no d3, no DOM. The interesting properties here are subtree
 * arithmetic and cycle tolerance, and neither can be demonstrated by clicking
 * around in a browser — see `scripts/spec-graph.test.ts`.
 */

import type { D3Node, D3Link, GraphData } from '../types/graph';
// Explicit `.ts` on the value imports so `scripts/spec-graph.test.ts` can load
// this module under `node --test` type stripping, which resolves no extensions.
import { isSpecNode } from '../types/graph.ts';
import { normalizeRefPath, pathClaims, bySpecificity } from '../utils/refPaths.ts';

/**
 * Re-exported because this module is the spec layer's public face — its
 * consumers should not have to know that the predicate lives with the node
 * model and the path arithmetic with the path helpers.
 */
export { isSpecNode } from '../types/graph.ts';
export { pathsClaim } from '../utils/refPaths.ts';

/**
 * Vertical tier per Elevator kind, driving the layered layout.
 *
 * Taken from the kind rather than from graph depth on purpose. The Elevator
 * hierarchy is fixed by the language — a Functionality is always a leaf on a
 * Feature — so a Feature that happens to be reachable at two different depths
 * (because two Categories claim it) must still draw on the Feature row.
 * Depth-derived tiers would put the same kind on two rows and make the picture
 * lie about the language.
 *
 * Concept and UI Page sit below the hierarchy rather than inside it: neither
 * is contained by anything, they are referenced across it.
 */
export const SPEC_TIERS: Record<string, number> = {
  Extension: 0,
  Category: 1,
  Feature: 2,
  Functionality: 3,
  Concept: 4,
  UiPage: 5,
};

/** Fallback row for a kind the spec language grows later — below everything
 *  known, so a new kind is visibly unplaced rather than silently drawn as a
 *  Feature. */
const UNKNOWN_TIER = 6;

export function tierOf(node: D3Node): number {
  return SPEC_TIERS[node.kind_raw] ?? UNKNOWN_TIER;
}

/** Tier → the kind that names it, for row captions. */
const TIER_KIND = new Map(Object.entries(SPEC_TIERS).map(([kind, tier]) => [tier, kind]));

export interface SpecGraph {
  /** True when the loaded project has no `.elv` layer, so the split view can
   *  say so rather than rendering an empty pane. */
  readonly empty: boolean;
  nodes: D3Node[];
  /** Every link with both ends in the spec layer — `Contains` for the
   *  hierarchy, `References` / `Uses` for the cross-links the hierarchy
   *  cannot express. */
  links: D3Link[];
  /** `Contains` children, in graph order. */
  children: Map<string, string[]>;
  /** `Contains` parents. Plural: a Feature may be listed by more than one
   *  Category, and collapsing that to one parent would silently drop the
   *  second Category's claim on its code. */
  parents: Map<string, string[]>;
  /** Union of `cr:` paths over each node's `Contains`-subtree, normalized and
   *  most-specific-first. Empty for an entity that neither declares a ref nor
   *  contains anything that does. */
  claims: Map<string, string[]>;
}

const EMPTY_GRAPH: SpecGraph = {
  empty: true,
  nodes: [],
  links: [],
  children: new Map(),
  parents: new Map(),
  claims: new Map(),
};

function endIds(link: D3Link): [string, string] {
  const source = typeof link.source === 'object' ? (link.source as D3Node).id : link.source;
  const target = typeof link.target === 'object' ? (link.target as D3Node).id : link.target;
  return [source, target];
}

/**
 * Cut the spec layer out of a full analysis result.
 *
 * Fed the *unscoped* graph by its caller, and that is load-bearing: the spec
 * pane is the navigator: it is what the reader steers the code pane with, so
 * narrowing the code scope must not shrink the map that does the narrowing.
 */
export function buildSpecGraph(full: GraphData | null): SpecGraph {
  if (!full || full.nodes.length === 0) return EMPTY_GRAPH;

  const nodes = full.nodes.filter(isSpecNode);
  if (nodes.length === 0) return EMPTY_GRAPH;

  const ids = new Set(nodes.map((n) => n.id));
  const links = full.links.filter((l) => {
    const [source, target] = endIds(l);
    return ids.has(source) && ids.has(target);
  });

  const children = new Map<string, string[]>();
  const parents = new Map<string, string[]>();
  for (const link of links) {
    if (link.kind_raw !== 'Contains') continue;
    const [source, target] = endIds(link);
    if (source === target) continue;
    push(children, source, target);
    push(parents, target, source);
  }

  return {
    empty: false,
    nodes,
    links,
    children,
    parents,
    claims: buildClaims(nodes, children),
  };
}

function push(map: Map<string, string[]>, key: string, value: string): void {
  const existing = map.get(key);
  if (!existing) {
    map.set(key, [value]);
    return;
  }
  if (!existing.includes(value)) existing.push(value);
}

/** A node's own declared refs, normalized, empties dropped. */
function ownPaths(node: D3Node): string[] {
  const out: string[] = [];
  for (const ref of node.codeRefs ?? []) {
    const path = normalizeRefPath(ref.path);
    if (path) out.push(path);
  }
  return out;
}

/**
 * Roll `cr:` paths up the containment tree.
 *
 * Memoized post-order walk. The `visiting` set is not defensive
 * programming — Elevator explicitly allows import cycles ("no execution
 * semantics; the analyzer deduplicates"), and a spec that declares a
 * containment cycle by mistake must render as a cycle rather than hang the
 * browser. A node reached while its own subtree is still being computed
 * contributes what it has so far and does not recurse again.
 */
function buildClaims(nodes: D3Node[], children: Map<string, string[]>): Map<string, string[]> {
  const own = new Map(nodes.map((n) => [n.id, ownPaths(n)]));
  const claims = new Map<string, string[]>();
  const visiting = new Set<string>();

  const walk = (id: string): string[] => {
    const done = claims.get(id);
    if (done) return done;
    if (visiting.has(id)) return own.get(id) ?? [];
    visiting.add(id);
    const collected = new Set(own.get(id) ?? []);
    for (const child of children.get(id) ?? []) {
      for (const path of walk(child)) collected.add(path);
    }
    visiting.delete(id);
    const result = bySpecificity(collected);
    claims.set(id, result);
    return result;
  };

  for (const node of nodes) walk(node.id);
  return claims;
}

/**
 * The code paths a spec entity stands for — its own refs plus everything its
 * subtree declares.
 *
 * This is what makes a Category clickable. A Category is behaviourless by
 * definition (CONTEXT.md: "name + description + a list of child Features"), so
 * it never carries a `cr:` of its own; reading only its own refs would make
 * every top-level row in the spec pane a no-op.
 */
export function claimedPaths(graph: SpecGraph, id: string | null): string[] {
  if (!id) return [];
  return graph.claims.get(id) ?? [];
}

/**
 * The union of what several entities claim.
 *
 * Union, not intersection: checking two Categories asks for the code belonging
 * to either, the way the entity-kind and language filters already read. An
 * intersection would be near-always empty — two Categories sharing code is the
 * exception the `cr:` duplicate marker exists to flag, not the common case.
 */
export function claimedPathsForAll(graph: SpecGraph, ids: Iterable<string>): string[] {
  const all = new Set<string>();
  for (const id of ids) {
    for (const path of graph.claims.get(id) ?? []) all.add(path);
  }
  return bySpecificity(all);
}

/**
 * The pick lists behind the Filters pane's Spec section — one per tier, in
 * language order, empty tiers dropped.
 *
 * `drillDown` narrows each tier to the children of what is already selected:
 * the same progressive disclosure the pane does spatially, applied to a list.
 * Off, every entity is offered at every tier, which is what you want when you
 * know the name of the Functionality you are after and not which Category it
 * hangs from.
 *
 * A selected entity is always offered, even when the rule would hide it.
 * Otherwise deselecting a Category strands its Features checked, filtering the
 * canvas from rows that are no longer on screen to uncheck.
 */
export function specOptions(
  graph: SpecGraph,
  selection: ReadonlySet<string>,
  drillDown: boolean,
): { kind: string; tier: number; nodes: D3Node[] }[] {
  const offered = graph.nodes.filter((node) => {
    if (!drillDown || selection.has(node.id)) return true;
    const parents = graph.parents.get(node.id) ?? [];
    // Nothing above it — a root, or a Concept, which by definition is
    // contained by nothing. Always offered; there is no parent to gate on.
    if (parents.length === 0) return true;
    return parents.some((parent) => selection.has(parent));
  });

  const byTier = new Map<number, D3Node[]>();
  for (const node of offered) {
    const tier = tierOf(node);
    const bucket = byTier.get(tier);
    if (bucket) bucket.push(node);
    else byTier.set(tier, [node]);
  }

  return [...byTier.keys()]
    .sort((a, b) => a - b)
    .map((tier) => {
      const nodes = byTier.get(tier)!.slice().sort((a, b) => a.name.localeCompare(b.name));
      return { kind: TIER_KIND.get(tier) ?? nodes[0].kind_raw, tier, nodes };
    });
}

/**
 * Spec entities whose *own* `cr:` claims this file, most-specific first.
 *
 * Own refs, not rolled-up ones: rolled up, every Category in the project would
 * claim most files and the reverse highlight would light the whole pane. The
 * ancestors are added back deliberately by [`withAncestors`], which is a
 * different statement — "here is the path through the hierarchy to the entity
 * that actually declared this" — and reads as a trail rather than a match.
 */
export function specNodesClaiming(graph: SpecGraph, filePath: string): string[] {
  const subject = normalizeRefPath(filePath);
  if (!subject || graph.empty) return [];
  const hits: { id: string; path: string }[] = [];
  for (const node of graph.nodes) {
    for (const path of ownPaths(node)) {
      if (pathClaims(path, subject)) hits.push({ id: node.id, path });
    }
  }
  hits.sort((a, b) => b.path.length - a.path.length || a.id.localeCompare(b.id));
  const seen = new Set<string>();
  const out: string[] = [];
  for (const hit of hits) {
    if (seen.has(hit.id)) continue;
    seen.add(hit.id);
    out.push(hit.id);
  }
  return out;
}

/**
 * Whether an entity answers a pick-list filter box.
 *
 * Name *and* qualified name, so `grouping.cohere` finds a Functionality the
 * same way its bare verb does — the qualified form is what the spec calls it
 * and what a reader arriving from `elevator --list` will type. Blank matches
 * everything, so an empty box is not a filter.
 */
export function matchesSpecQuery(node: D3Node, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return node.name.toLowerCase().includes(q)
    || node.qualified_name.toLowerCase().includes(q);
}

// ---------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------

/**
 * Whether clicking a spec entity can show the reader anything, and if not,
 * why not.
 *
 * Four states because they have four different fixes, and collapsing any two
 * of them sends the reader at the wrong one. `codeRefs.ts` already makes this
 * argument for the first two; the analysis scope adds the third:
 *
 * - `in-scope` — at least one claimed path is loaded. Clicking works.
 * - `out-of-scope` — the claims name real code that the current **analysis
 *   scope** excludes. Fix: widen the scope. Nothing is wrong with the spec,
 *   and badging this as drift would send someone editing a correct `cr:`.
 * - `unanalyzed` — the claims name nothing in the *analysed graph* at all.
 *   Two very different causes, and this cannot tell them apart: the path died
 *   (real drift), or its file type is outside the parsed language set —
 *   `.mezz/settings.json` restricts this repo to rust/typescript/svelte/
 *   elevator, so a `cr:` at a `.py` script resolves on disk and appears
 *   nowhere here. It was called `stale` and asserted drift, which put a
 *   warning badge on correct refs and sent readers to edit a healthy spec.
 *   `elevator --drift` is the check that CAN separate them, reading the
 *   filesystem rather than the graph.
 * - `unanchored` — no `cr:` anywhere in the subtree. Fix: write one. This is
 *   the state the whole feature is meant to surface, so it must not hide
 *   inside a generic "shows nothing".
 */
export type SpecScopeState = 'in-scope' | 'out-of-scope' | 'unanalyzed' | 'unanchored';

/**
 * Classify every entity against two path universes.
 *
 * `loaded` is built from the analysis scope's files, `repo` from the whole
 * parsed graph — both via `buildPathUniverse`, so each carries every file
 * *and its ancestor directories*. That is what makes the folder cases work in
 * both directions without a second matcher: a `cr: ui/` is in scope because
 * `ui` is an ancestor of a loaded file, and a `cr: src/` survives a narrowing
 * to `src/parser` for the same reason — which is right, because clicking it
 * would indeed still show something.
 *
 * Claims are the **rolled-up** ones, matching what the cross-filter would
 * actually apply. Classifying on own-refs would mark every Category
 * unanchored, since a Category never carries a `cr:`.
 */
export function specScopeStates(
  graph: SpecGraph,
  loaded: Set<string>,
  repo: Set<string>,
): Map<string, SpecScopeState> {
  const states = new Map<string, SpecScopeState>();
  for (const node of graph.nodes) {
    const claims = graph.claims.get(node.id) ?? [];
    if (claims.length === 0) {
      states.set(node.id, 'unanchored');
      continue;
    }
    if (claims.some((path) => loaded.has(path))) {
      states.set(node.id, 'in-scope');
      continue;
    }
    states.set(node.id, claims.some((path) => repo.has(path)) ? 'out-of-scope' : 'unanalyzed');
  }
  return states;
}

/**
 * Keep only the entities in `keep`, for the pane's "follow the analysis
 * scope" mode.
 *
 * **`claims` is carried over untouched, not recomputed.** What a Category
 * stands for is a fact about the spec, not about what happens to be drawn —
 * recomputing the roll-up over the surviving nodes would silently shrink a
 * partially-in-scope Category's filter to the subset still visible, so
 * clicking it would show less code than the same click shows with the toggle
 * off. The filter has to mean the same thing in both modes.
 *
 * `children` and `parents` *are* rebuilt, because they describe the drawn
 * picture: an edge to a hidden node is not an edge the reader can follow.
 */
export function filterSpecGraph(graph: SpecGraph, keep: ReadonlySet<string>): SpecGraph {
  if (graph.empty) return graph;
  const nodes = graph.nodes.filter((n) => keep.has(n.id));
  if (nodes.length === graph.nodes.length) return graph;
  if (nodes.length === 0) return EMPTY_GRAPH;

  const links = graph.links.filter(
    (l) => keep.has(endIds(l)[0]) && keep.has(endIds(l)[1]),
  );
  const children = new Map<string, string[]>();
  const parents = new Map<string, string[]>();
  for (const link of links) {
    if (link.kind_raw !== 'Contains') continue;
    const [source, target] = endIds(link);
    if (source === target) continue;
    push(children, source, target);
    push(parents, target, source);
  }
  return { empty: false, nodes, links, children, parents, claims: graph.claims };
}

// ---------------------------------------------------------------------
// Drill-down
// ---------------------------------------------------------------------

/**
 * The entities the pane opens on — everything nothing else `Contains`.
 *
 * Derived from the containment edges rather than from a kind list, which
 * matters in three ways a `kind === 'Category'` test would get wrong: a spec
 * using Extensions opens on those instead, Concepts appear because nothing
 * contains them (they are cross-cutting by definition), and a Feature no
 * Category declares shows up rather than becoming unreachable — an orphan is
 * exactly the thing a reader should see.
 */
export function specRoots(graph: SpecGraph): string[] {
  return graph.nodes.filter((n) => (graph.parents.get(n.id) ?? []).length === 0).map((n) => n.id);
}

/**
 * The containment trail from a root down to `id`, root first.
 *
 * First parent at each step. A Feature under two Categories has two honest
 * trails and this picks one; the alternative — drawing both — is what the flat
 * view did, and having a node appear under every parent at once is a large
 * part of why the flat view was unreadable. The other parent is one click
 * away, since selecting it re-roots the trail through itself.
 */
export function trailTo(graph: SpecGraph, id: string): string[] {
  const trail: string[] = [];
  const seen = new Set<string>();
  let current: string | undefined = id;
  while (current && !seen.has(current)) {
    seen.add(current);
    trail.push(current);
    current = (graph.parents.get(current) ?? [])[0];
  }
  return trail.reverse();
}

/**
 * What the pane draws for a given drill path: the roots, everything on the
 * path, and the children of each step along it.
 *
 * Children of *every* step rather than only the last, so the picture keeps the
 * shape of the descent — you see the Feature you picked among its siblings,
 * with its own Functionalities below, instead of a single column that forgets
 * where it came from.
 *
 * `forced` is folded in with its ancestors so the code→spec highlight can
 * still reach an entity the path has not opened. That is a real case: clicking
 * a file whose claimant lives three levels down must ring *something*, and
 * silently revealing its trail is better than either lighting nothing or
 * yanking the user's drill path somewhere they did not ask to go.
 */
export function revealedIds(
  graph: SpecGraph,
  path: readonly string[],
  forced: ReadonlySet<string> = new Set(),
): Set<string> {
  const out = new Set<string>(specRoots(graph));
  // Membership from the node list, not from `claims`: after `filterSpecGraph`
  // the claims map still holds every entity in the spec, so testing it would
  // re-reveal exactly the nodes the scope filter just removed.
  const present = new Set(graph.nodes.map((n) => n.id));
  for (const step of path) {
    // Stop at the first step the graph has lost, rather than skipping it. A
    // re-analysis or a scope change can strip an entity out from under an open
    // path, and carrying on past the gap would draw its grandchildren with
    // nothing above them — a level that appears to hang off the wrong parent.
    if (!present.has(step)) break;
    out.add(step);
    for (const child of graph.children.get(step) ?? []) out.add(child);
  }
  for (const id of forced) {
    for (const step of trailTo(graph, id)) out.add(step);
  }
  return out;
}

// ---------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------

/**
 * Column pitch and row pitch, in px. `COL_PITCH` is set by the widest name a
 * 10px label can carry without colliding with its neighbour; `ROW_PITCH` by a
 * circle plus the name drawn under it.
 */
export const COL_PITCH = 92;
export const ROW_PITCH = 38;
/** Left inset, leaving the tier captions a gutter of their own. */
export const GUTTER = 14;
/** Vertical space above each tier's first sub-row, for its caption. */
export const BAND_HEADER = 26;

export interface SpecLayout {
  /** Where each entity draws. */
  positions: Map<string, { x: number; y: number }>;
  /** Caption anchor per tier, in tier order, skipping tiers with no nodes —
   *  an empty row's caption is a promise of content that is not there. */
  bands: { kind: string; tier: number; y: number; count: number }[];
  /** Total extent, so the caller can size its viewport and fit to it. */
  width: number;
  height: number;
}

/**
 * Lay the spec out as wrapped tiers.
 *
 * A force simulation was the first attempt and does not survive contact with a
 * real spec in a side pane: this repo's own has 76 Features, and one row of 76
 * circles is ~2300px wide. In a 400px column `forceCollide` pushes them off
 * their row until the tiers stop meaning anything, and fitting the row to the
 * pane shrinks it past the point where a label can be read. Neither is a
 * layout; both are the same information refusing to fit.
 *
 * So each tier wraps into as many sub-rows as the pane's width allows, and the
 * whole thing is deterministic — same graph and same width, same picture, with
 * nothing to settle and no frame where the reader watches nodes drift. Panning
 * and zooming are how a tall spec is read, which is what a side pane is shaped
 * for anyway.
 *
 * Within a tier, nodes are ordered by their first parent's position in the tier
 * above, so children sit under the parent that declares them and the
 * containment edges stay short. `bySpecificity`-style stability matters here
 * too: ties break on qualified name so the picture does not reshuffle between
 * two runs over the same spec.
 */
export function layoutSpecGraph(graph: SpecGraph, width: number): SpecLayout {
  const positions = new Map<string, { x: number; y: number }>();
  const bands: SpecLayout['bands'] = [];
  if (graph.empty) return { positions, bands, width: 0, height: 0 };

  const columns = Math.max(1, Math.floor((width - GUTTER * 2) / COL_PITCH));

  const byTier = new Map<number, D3Node[]>();
  for (const node of graph.nodes) {
    const tier = tierOf(node);
    const bucket = byTier.get(tier);
    if (bucket) bucket.push(node);
    else byTier.set(tier, [node]);
  }

  let y = BAND_HEADER;
  let widest = 0;
  /** Rank of each node in the tier above, for the child ordering. */
  let parentRank = new Map<string, number>();

  for (const tier of [...byTier.keys()].sort((a, b) => a - b)) {
    const nodes = byTier.get(tier)!;
    const rankOf = (node: D3Node): number => {
      let best = Number.MAX_SAFE_INTEGER;
      for (const parent of graph.parents.get(node.id) ?? []) {
        const rank = parentRank.get(parent);
        if (rank !== undefined && rank < best) best = rank;
      }
      return best;
    };
    nodes.sort((a, b) => rankOf(a) - rankOf(b) || a.qualified_name.localeCompare(b.qualified_name));

    // The tier's canonical name, not the first node's kind: the fallback tier
    // collects every kind the language grows later, and captioning that row
    // with whichever one sorted first would misname the rest.
    bands.push({
      kind: TIER_KIND.get(tier) ?? nodes[0].kind_raw,
      tier,
      y: y - BAND_HEADER + 12,
      count: nodes.length,
    });

    const nextRank = new Map<string, number>();
    nodes.forEach((node, index) => {
      const column = index % columns;
      const row = Math.floor(index / columns);
      const x = GUTTER + column * COL_PITCH + COL_PITCH / 2;
      positions.set(node.id, { x, y: y + row * ROW_PITCH });
      nextRank.set(node.id, index);
      if (x > widest) widest = x;
    });

    const rows = Math.ceil(nodes.length / columns);
    y += rows * ROW_PITCH + BAND_HEADER;
    parentRank = nextRank;
  }

  return { positions, bands, width: widest + COL_PITCH / 2, height: y };
}

/**
 * An entity plus everything it `Contains`, transitively.
 *
 * The visual counterpart of [`claimedPaths`]: that answers "what code does
 * this stand for", this answers "what part of the spec is that", and the two
 * must walk the same edges or the pane emphasises a different set from the one
 * it filtered by.
 */
export function subtreeOf(graph: SpecGraph, id: string | null): Set<string> {
  const out = new Set<string>();
  if (!id) return out;
  const queue = [id];
  while (queue.length > 0) {
    const next = queue.pop()!;
    if (out.has(next)) continue;
    out.add(next);
    for (const child of graph.children.get(next) ?? []) {
      if (!out.has(child)) queue.push(child);
    }
  }
  return out;
}

/**
 * `ids` plus every `Contains`-ancestor, so a highlighted Functionality shows
 * the Feature and Category it hangs from instead of floating alone on a row
 * four tiers down.
 *
 * Cycle-tolerant for the same reason [`buildClaims`] is.
 */
export function withAncestors(graph: SpecGraph, ids: Iterable<string>): Set<string> {
  const out = new Set<string>();
  const queue = [...ids];
  while (queue.length > 0) {
    const id = queue.pop()!;
    if (out.has(id)) continue;
    out.add(id);
    for (const parent of graph.parents.get(id) ?? []) {
      if (!out.has(parent)) queue.push(parent);
    }
  }
  return out;
}
