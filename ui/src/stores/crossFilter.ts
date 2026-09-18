/**
 * The wire between the two panes of the split view (ADR 0011).
 *
 * The spec pane and the code pane draw two subgraphs of one `AnalysisResult`,
 * and everything they say to each other goes through here — in both
 * directions, and asymmetrically on purpose:
 *
 *  - **spec → code is a filter.** Focusing a Category hides every code entity
 *    outside the `cr:` paths its subtree declares. Asking "show me only what
 *    belongs to this" and getting the surrounding code back anyway is not an
 *    answer.
 *  - **spec → code is *also* a highlight**, on a separate channel, because
 *    "show me only this" and "show me where this is" are two questions and
 *    the click could only answer one of them. See `viewmodels/specHighlight.ts`.
 *  - **code → spec is a highlight.** The spec pane is small and its whole
 *    value is the shape of the hierarchy; filtering it to the two entities
 *    that claim the selected file would delete the map to show a pin on it.
 *
 * The filter is a **layer over the scope, not a write to it**. ADR 0005
 * rejected the split pane partly because it seemed to need a `selectedScopes`
 * holding two independent selections; it does not, because the code pane's own
 * scope is untouched and this narrows what is drawn from it. One `Escape`
 * clears the focus and the user's scope is exactly as they left it — which is
 * also why focusing a spec entity is not `setScopes`, the destructive door
 * `showImplementingCode` goes through for the in-place pairing.
 *
 * **Selection and revelation are separate state, and the filter outlives the
 * pane.** Both were true the other way round once. `specPath` was a single
 * chain that doubled as the selection, and `crossFilterPaths` went null when
 * the pane closed — on the reasoning that a filter with no visible control is
 * worse than no filter. The reasoning was sound and the conclusion was wrong:
 * collapsing the pane is a request for canvas room, not for the filter to
 * stop, and the fix for "no visible control" is a second control, not a
 * self-cancelling filter. The Filters pane's Spec section is that control, so
 * the filter can now safely survive anything the pane does.
 */

import { derived, get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import type { D3Node } from '../types/graph';
import { fullGraphDataStore } from './scope';
import { focusNode, graphData, rawEntityGraph, selectedNode } from './graph';
import { filterOnSpecClick, followAnalysisScope, splitViewOpen } from './panes';
import { buildPathUniverse } from '../utils/refPaths';
import {
  claimedNodeIds,
  highlightSources,
  togglePin,
} from '../viewmodels/specHighlight';
import {
  buildSpecGraph,
  claimedPaths,
  claimedPathsForAll,
  filterSpecGraph,
  isSpecNode,
  revealedIds,
  specNodesClaiming,
  specScopeStates,
  trailTo,
  type SpecGraph,
  type SpecScopeState,
} from '../viewmodels/specGraph';

/**
 * The Elevator layer, cut from the *unscoped* graph.
 *
 * `fullGraphDataStore` rather than `graphData`: the spec pane is the control
 * surface for the code pane's scope, so deriving it from the scoped graph
 * would let a narrow scope erase the map the user narrows with — and would
 * make the pane empty exactly when it is most needed.
 */
export const specGraph: Readable<SpecGraph> = derived(fullGraphDataStore, ($full) =>
  buildSpecGraph($full),
);

/**
 * What each entity's `cr:` claims can currently reach.
 *
 * Against `rawEntityGraph` — the entities the **Analysis Scope** tree loaded,
 * before collapse and before any display filter. Two other stores were
 * candidates and both are wrong here:
 *
 * - `analysisGraphData` is what the *store* named "analysis scope" slices, but
 *   its rules are only writable from the VS Code extension and default to the
 *   whole repo, so in the browser this marker would never fire. The sidebar
 *   heading a reader sees as "Analysis Scope" is `ScopeTree`, which writes
 *   `scopeRules` — UI-051 renamed the label and not the store.
 * - `graphData` is the same file set collapsed to the current level. The
 *   universe is built from `file_path`, which survives collapse, so it would
 *   agree — but it would also rebuild on every level toggle for no change in
 *   the answer.
 *
 * Two universes, and the second is what keeps the marker honest. Against the
 * scope alone, an entity whose refs point at deleted code and one whose refs
 * point at code you simply have not loaded look identical — both claim nothing
 * reachable. They are not: one is drift and one is a scope setting, and a
 * reader told "widen the scope" about a stale `cr:` will widen it to the whole
 * repo and still see nothing.
 *
 * No scope picked yet means no marks. Before the first selection the canvas
 * shows its own "pick a scope" card, and dimming the entire spec behind it
 * would be a second answer to a question the reader has not asked.
 */
export const specScopeState: Readable<Map<string, SpecScopeState>> = derived(
  [specGraph, rawEntityGraph, fullGraphDataStore],
  ([$graph, $loaded, $full]) => {
    if ($graph.empty || $loaded.nodes.length === 0) return new Map<string, SpecScopeState>();
    return specScopeStates(
      $graph,
      buildPathUniverse($loaded.nodes.map((n) => n.file_path)),
      buildPathUniverse(($full?.nodes ?? []).map((n) => n.file_path)),
    );
  },
);

/**
 * The graph the pane actually draws.
 *
 * Only `in-scope` survives when following. The other three states are all
 * "clicking this shows nothing", which is precisely what the toggle is for —
 * and that includes `unanchored`, so turning it on is also the fastest way to
 * see how much of a spec is anchored at all.
 */
export const visibleSpecGraph: Readable<SpecGraph> = derived(
  [specGraph, specScopeState, followAnalysisScope],
  ([$graph, $states, $follow]) => {
    if (!$follow || $graph.empty) return $graph;
    const keep = new Set<string>();
    for (const node of $graph.nodes) {
      if ($states.get(node.id) === 'in-scope') keep.add(node.id);
    }
    return filterSpecGraph($graph, keep);
  },
);

/**
 * What the code pane is filtered by: a set, not a path.
 *
 * A set because the Filters pane offers Categories, Concepts, Features and
 * Functionalities as independent checkboxes, and because two Categories
 * checked together is a real question ("show me everything these two own").
 * The union is taken in `crossFilterPaths`.
 *
 * Ids rather than nodes so it survives a re-analysis: `transformAnalysisJson`
 * mints fresh objects on every fetch, and held references would pin the filter
 * to stale copies of entities that may no longer exist.
 */
export const specSelection = writable<Set<string>>(new Set());

/**
 * What the spec pane has *opened* — pure view state, root first.
 *
 * Deliberately not the selection any more. The two were one store while the
 * pane was the only surface, and merging them was defensible then: expanding
 * and filtering were one gesture. They are not one gesture once a checkbox in
 * the Filters pane can select without opening anything, and once closing the
 * pane must leave the filter running.
 *
 * The pane still keeps them in step for its own clicks — a click there selects
 * *and* drills — but nothing else has to.
 */
export const specPath = writable<string[]>([]);

/** The deepest step of the drill path, for the pane's breadcrumb emphasis. */
export const specFocusId: Readable<string | null> = derived(
  specPath,
  ($path) => $path.at(-1) ?? null,
);

/**
 * Spec entities the code selection lights up, and the trail down to them.
 *
 * Computed before the drill filter — a claimant three levels down must be
 * reachable even when the reader has not opened its branch, so this is fed
 * into `revealedIds` as `forced` rather than intersected with what is already
 * drawn.
 *
 * Empty while the selection *is* a spec entity: there the drill path already
 * owns the pane, and pointing the reverse channel at the same node would
 * double-mark it.
 */
export const specHighlightIds: Readable<Set<string>> = derived(
  [visibleSpecGraph, selectedNode, splitViewOpen],
  ([$graph, $selected, $split]) => {
    if (!$split || !$selected || $graph.empty) return new Set<string>();
    if (isSpecNode($selected)) return new Set<string>();
    return new Set(specNodesClaiming($graph, $selected.file_path));
  },
);

// ---------------------------------------------------------------------------
// The highlight channel
// ---------------------------------------------------------------------------

/**
 * The spec entity under the pointer in the spec pane, or `null`.
 *
 * Pointer state in a store rather than in the component because the thing it
 * moves is on the *other* canvas — the same reason `hoveredNode` lives in
 * `graph.ts`. It is not persisted and not mirrored: a hover is a question
 * being asked right now by the hand that is moving.
 */
export const specHoverId = writable<string | null>(null);

/**
 * Spec entities whose claimed code stays lit after the pointer leaves.
 *
 * The hover answers "where is this" for as long as you hold still, which is
 * exactly as long as you cannot also be clicking, scrolling or reading the
 * Details pane. Pinning is what makes the answer survive the next gesture,
 * and it is what lets the two channels be used together: filter to a Category
 * with a click, then pin a Functionality to see which of the remaining code
 * is its.
 */
export const specPinnedHighlight = writable<Set<string>>(new Set());

/** Pinned plus hovered — see `highlightSources` for why it is a union. */
export const specHighlightSourceIds: Readable<string[]> = derived(
  [specPinnedHighlight, specHoverId],
  ([$pinned, $hover]) => highlightSources($pinned, $hover),
);

/**
 * Code paths the highlight stands for.
 *
 * Reads `visibleSpecGraph` for the same reason `crossFilterPaths` does: it
 * carries the *unfiltered* claims map, so a Category still stands for its
 * whole subtree even when the pane has revealed only one level of it. Pointing
 * at something must not mean less than selecting it would.
 */
export const specHighlightPaths: Readable<string[] | null> = derived(
  [visibleSpecGraph, specHighlightSourceIds],
  ([$graph, $sources]) => {
    if ($graph.empty || $sources.length === 0) return null;
    const live = $sources.filter((id) => $graph.claims.has(id));
    if (live.length === 0) return null;
    return claimedPathsForAll($graph, live);
  },
);

/**
 * What the code canvas rings: ids, resolved against `graphData`.
 *
 * `graphData` and not `rawEntityGraph`, because this is the only store here
 * whose answer has to be in the *drawn* vocabulary — the canvas puts a class
 * on a node it has, and at File or Module level the node it has is a rollup
 * whose id was minted by `collapseGraph`. Matching by `file_path` inside
 * `claimedNodeIds` is what makes the rollup answer for the entities under it.
 *
 * Resolving to ids here rather than handing the canvas the paths keeps the
 * per-node path arithmetic out of the render loop, where it would rerun on
 * every plan apply for an answer that only changes when the pointer does.
 */
export const specClaimHighlightIds: Readable<Set<string>> = derived(
  [specHighlightPaths, graphData],
  ([$paths, $data]) => claimedNodeIds($paths, $data.nodes),
);

/** The pane's hover. Takes the node so the caller cannot pass an id from a
 *  graph this store does not hold. */
export function hoverSpecEntity(node: D3Node | null): void {
  specHoverId.set(node?.id ?? null);
}

/** Pin or unpin what the pointer is on — the pane's shift-click. */
export function togglePinnedHighlight(id: string): void {
  specPinnedHighlight.update((current) => togglePin(current, id));
}

/** Drop every pin. The hover is left alone: it is about to be answered by
 *  wherever the pointer is now. */
export function clearPinnedHighlight(): void {
  specPinnedHighlight.set(new Set());
}

/**
 * The graph the pane actually draws: roots, the open path, each step's
 * children, and whatever the code selection forced open.
 *
 * This is the answer to the flat view's real problem, which was not that it
 * drew too many nodes but that it drew them all at equal weight — 41 Features
 * scattered across four wrapped sub-rows, with the one you cared about
 * indistinguishable from the forty you did not. Progressive disclosure means
 * the pane never holds more than one branch's worth of choices.
 */
export const drawnSpecGraph: Readable<SpecGraph> = derived(
  [visibleSpecGraph, specPath, specSelection, specHighlightIds, specPinnedHighlight],
  ([$graph, $path, $selection, $highlight, $pinned]) => {
    if ($graph.empty) return $graph;
    // Anything selected is forced visible along with its trail, whether it was
    // picked here or ticked in the Filters pane. A filter running from a row
    // the pane refuses to draw is the state this whole rework exists to avoid,
    // and a *pin* is the same state one channel over: rings on the canvas whose
    // source the pane has since navigated away from and will not redraw.
    //
    // The hover is deliberately not in here. It is forced open by nothing
    // because it cannot need to be — you can only hover what is already drawn —
    // and putting it in would relay the pane out on every pointer move.
    const forced = new Set([...$selection, ...$highlight, ...$pinned]);
    return filterSpecGraph($graph, revealedIds($graph, $path, forced));
  },
);

/** The focused entity itself, re-resolved against the graph the pane draws. */
export const specFocus: Readable<D3Node | null> = derived(
  [drawnSpecGraph, specFocusId],
  ([$graph, $id]) => (!$id ? null : $graph.nodes.find((n) => n.id === $id) ?? null),
);

/** The open path as entities, for the breadcrumb. Drops steps the current
 *  graph no longer holds, so a scope change cannot leave a crumb pointing at
 *  something that is not there. */
export const specTrail: Readable<D3Node[]> = derived(
  [visibleSpecGraph, specPath],
  ([$graph, $path]) =>
    $path
      .map((id) => $graph.nodes.find((n) => n.id === id))
      .filter((n): n is D3Node => n !== undefined),
);

/**
 * Code paths the selection stands for, or `null` when nothing is selected.
 *
 * The two falsy-looking states are deliberately different and must not be
 * collapsed: `null` means *no filter is active* (draw everything in scope),
 * and `[]` means *the selected entities declare no code* (draw nothing, and
 * say why). Conflating them would make an unanchored Feature silently behave
 * like a cleared filter, which is the failure that hides missing `cr:`
 * coverage.
 *
 * **Not gated on `splitViewOpen`.** Collapsing the pane is a request for
 * canvas room; it is not a request to stop filtering, and treating it as one
 * meant the reader lost their filter every time they wanted a wider graph. The
 * Filters pane's Spec section keeps the filter reachable and clearable with
 * the pane shut, which is what makes this safe.
 *
 * `visibleSpecGraph` carries the *unfiltered* `claims` map (see
 * `filterSpecGraph`), so a Category still stands for its whole subtree even
 * though the pane has only revealed one level of it. Drilling changes what you
 * can see, never what a selection means.
 */
export const crossFilterPaths: Readable<string[] | null> = derived(
  [visibleSpecGraph, specSelection],
  ([$graph, $selection]) => {
    if ($graph.empty || $selection.size === 0) return null;
    // Only entities the current scope still holds. A selection surviving a
    // re-analysis that dropped its target must not filter on a ghost id.
    const live = [...$selection].filter((id) => $graph.claims.has(id));
    if (live.length === 0) return null;
    return claimedPathsForAll($graph, live);
  },
);

/** The selection as entities, for the pane's rings and the panel's summary. */
export const specSelectedNodes: Readable<D3Node[]> = derived(
  [visibleSpecGraph, specSelection],
  ([$graph, $selection]) =>
    $graph.nodes.filter((n) => $selection.has(n.id)),
);

/**
 * The pane's click: open an entity and make it the selection.
 *
 * Replaces rather than accumulates, and the asymmetry with the Filters pane is
 * deliberate. Here a click also *moves* the pane — it drills — so accumulating
 * would leave earlier picks filtering the canvas from branches the pane has
 * since navigated away from. Multi-select is an explicit act and belongs on
 * the explicit surface, which is the checkbox list.
 *
 * Re-picking the open entity closes one level rather than clearing outright:
 * the gesture that opened a level should close it, and dropping the whole path
 * on a mis-click is the expensive mistake in a drill-down.
 *
 * **Whether it also filters is `filterOnSpecClick`.** The two halves were one
 * gesture because they arrived as one, not because they are one question: a
 * reader who clicks to read a Feature's description in the Details pane has
 * asked nothing about what the canvas should draw, and got the canvas emptied
 * down to that Feature's files anyway. Off, the click still opens, still
 * drills and still moves Details — everything the reader wanted — and leaves
 * whatever filter the Filters pane's checkboxes are running untouched, which
 * is the separation `specSelection` and `specPath` already had and only this
 * function was collapsing.
 */
export function focusSpecEntity(node: D3Node): void {
  const graph = get(visibleSpecGraph);
  const wasOpen = get(specPath).at(-1) === node.id;
  specPath.update((current) => {
    if (current.at(-1) === node.id) return current.slice(0, -1);
    const at = current.indexOf(node.id);
    if (at >= 0) return current.slice(0, at + 1);
    return trailTo(graph, node.id);
  });
  // Closing a level clears the filter with it; the entity you just closed is
  // no longer the thing you are asking about.
  if (get(filterOnSpecClick)) specSelection.set(wasOpen ? new Set() : new Set([node.id]));
  // `focusNode`, not `selectedNode.set`: this is a click on a *panel*, and the
  // pane the reader clicked is not the canvas, so whatever the graph still
  // thinks is hovered is a leftover. Details prefers the selection and the
  // Description pane prefers the hover, so setting only the selection moved one
  // column and left the other narrating the code entity you navigated away
  // from. Every other panel-driven selection already goes through here.
  focusNode(node);
}

/** The Filters pane's checkbox: select without moving the pane. */
export function toggleSpecSelection(id: string): void {
  specSelection.update((current) => {
    const next = new Set(current);
    if (!next.delete(id)) next.add(id);
    return next;
  });
}

/** Select exactly these — the pick list's "only these" bulk action. */
export function setSpecSelection(ids: Iterable<string>): void {
  specSelection.set(new Set(ids));
}

/** Drop the filter, the pins and the pane's path. The one control that undoes
 *  everything this feature did, reachable from both surfaces — and it has to
 *  reach the pins too, or closing the pane strands rings on the canvas with
 *  nothing left on screen that explains them. */
export function clearSpecFocus(): void {
  specSelection.set(new Set());
  specPath.set([]);
  specPinnedHighlight.set(new Set());
  specHoverId.set(null);
}

/** Truncate the drill path at `id` — the breadcrumb's click. Leaves the
 *  selection alone: climbing back up to look around is navigation, and it
 *  should not silently change what the canvas is filtered by. */
export function drillTo(id: string): void {
  specPath.update((current) => {
    const at = current.indexOf(id);
    return at >= 0 ? current.slice(0, at + 1) : current;
  });
}

/** True when a cross-filter is narrowing the code pane right now. */
export function crossFilterActive(): boolean {
  return get(crossFilterPaths) !== null;
}
