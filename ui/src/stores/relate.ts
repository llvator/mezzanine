/**
 * Reading the marked set as a relationship — the state, and nothing about the
 * reading itself (UI-147).
 *
 * `stores/marks.ts` holds *what* is marked and says explicitly that it means
 * nothing on its own; `drillIntoMarks` in `stores/scope.ts` was the one thing
 * that spent the set, by narrowing. This is the second thing: spend it as a
 * *question* instead, and leave the canvas exactly as it was.
 *
 * That is the whole reason this is a mode rather than a navigation. Drilling
 * is irreversible-ish and clears the marks; asking is neither, and a reader
 * who asks "how do these relate" almost always wants to keep looking at the
 * picture that made them ask.
 *
 * ## Why the whole repo, and what happens before it arrives
 *
 * `markRelation` wants the unscoped graph — "who depends on both of these"
 * has one true answer and it is not "whoever is drawn right now". So opening
 * the mode kicks `ensureFullData()`. Until that resolves (and if it fails)
 * the reading falls back to `rawEntityGraph`, which is the scoped graph, and
 * carries `wholeRepo: false` so the panel can say the neighbour counts are a
 * floor rather than a total. Showing the narrow answer immediately and
 * widening it a moment later beats an empty pane with a spinner in it: the
 * direct flow between two marked scopes — the part a reader came for — is
 * already correct in the scoped graph, because both ends are in scope by
 * construction.
 */

import { derived, get, writable } from 'svelte/store';
import { rawEntityGraph } from './graph';
import { focusPane } from './keymap';
import { markedPaths } from './marks';
import { ensureFullData, fullGraphDataStore } from './scope';
import { compactSides, markRelation, type MarkRelation } from '../viewmodels/markRelation';

/** Whether the Details column is answering about the marked set. */
export const relateOpen = writable(false);

/** True while the whole-repo graph is on its way. The reading is already on
 *  screen by then — this only qualifies it. */
export const relateLoading = writable(false);

/** The reading, plus whether it was taken over the whole repo. */
export type RelationReading = MarkRelation & { wholeRepo: boolean };

/**
 * Open the mode and make sure the graph behind it is the repo's.
 *
 * The fetch is fired and not awaited by the caller: `relateOpen` is set first
 * so the panel appears on the click rather than after the round trip. A failed
 * fetch is swallowed on purpose — `ensureFullData` already surfaces load
 * errors through `graphLoadError`, and the fallback reading is a real answer
 * about a smaller graph, not an error state.
 */
export async function openRelate(): Promise<void> {
  // One scope has nothing to be related to, and a panel that opens to explain
  // the gesture would be the disabled control `CanvasToolbar` deliberately does
  // not draw. `drillIntoMarks` declines the same way on an empty set.
  if (compactSides([...get(markedPaths)]).length < 2) return;
  relateOpen.set(true);
  // The reading is drawn by the Details column, and `App.svelte` mounts that
  // column only while it has a width — so with it collapsed, setting the flag
  // above put the panel somewhere nobody can see and the button read as dead.
  // `focusPane` is the move `search.focus` already makes for the same reason:
  // a command whose output lands in a pane has to open that pane, or it has
  // not run. It also focuses it, which is right — the reader's attention just
  // moved there, and the shortcut bar should be showing that pane's keys.
  focusPane('details');
  if (get(fullGraphDataStore)) return;
  relateLoading.set(true);
  try {
    await ensureFullData();
  } catch {
    /* fall back to the scoped graph; the panel says which it used */
  } finally {
    relateLoading.set(false);
  }
}

export function closeRelate(): void {
  relateOpen.set(false);
}

/**
 * A set that can no longer answer closes the mode.
 *
 * Two things reach this: `clearMarks`, and `drillIntoMarks` — which clears the
 * set once it has spent it. Leaving the panel open past either would leave the
 * reader staring at a relationship between scopes nothing is marking any more,
 * with no ring on the canvas to say which they were.
 */
markedPaths.subscribe((marks) => {
  if (get(relateOpen) && compactSides([...marks]).length < 2) relateOpen.set(false);
});

/**
 * What the panel renders, or `null` when the mode is closed.
 *
 * Gated on `relateOpen` rather than computed always: this walks every edge in
 * the repo, and `rawEntityGraph` fires on every scope change, level change and
 * live reload. Nobody should pay for that while the panel is shut.
 */
export const relation = derived(
  [relateOpen, markedPaths, fullGraphDataStore, rawEntityGraph],
  ([$open, $marks, $full, $raw]): RelationReading | null => {
    if (!$open) return null;
    const graph = $full ?? $raw;
    return { ...markRelation(graph.nodes, graph.links, $marks), wholeRepo: $full !== null };
  },
);
