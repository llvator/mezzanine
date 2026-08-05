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
import { rawEntityGraph, selectedNode } from './graph';
import { followAnalysisScope, splitViewOpen } from './panes';
import { buildPathUniverse } from '../utils/refPaths';
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
  [visibleSpecGraph, specPath, specSelection, specHighlightIds],
  ([$graph, $path, $selection, $highlight]) => {
    if ($graph.empty) return $graph;
    // Anything selected is forced visible along with its trail, whether it was
    // picked here or ticked in the Filters pane. A filter running from a row
    // the pane refuses to draw is the state this whole rework exists to avoid.
    const forced = new Set([...$selection, ...$highlight]);
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
  specSelection.set(wasOpen ? new Set() : new Set([node.id]));
  selectedNode.set(node);
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

/** Drop the filter and close the pane's path. The one control that undoes
 *  everything this feature did, reachable from both surfaces. */
export function clearSpecFocus(): void {
  specSelection.set(new Set());
  specPath.set([]);
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
