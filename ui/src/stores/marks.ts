/**
 * What the reader has marked on the canvas, and nothing about what it means.
 *
 * A mark is *not* a filter and changes nothing on screen except its own ring:
 * the point of marking two files is to compare them where they are, with the
 * neighbourhood that made them interesting still drawn around them. The
 * narrowing happens once, on `drillIntoMarks` in `stores/scope.ts`, which is
 * also why the action lives there and the state lives here — scope imports
 * marks and never the other way round.
 *
 * Deliberately separate from `selectedNode`, which is the *subject*: the one
 * entity the Details, Description and Context panes are answering about, and
 * the root the tree layout draws from. Those are questions with exactly one
 * answer, so folding a set into that store would mean rewriting every reader
 * of it to pick a winner. The spec pane made the same split for the same
 * reason — `specSelection` is a set beside `selectedNode`, not instead of it.
 */

import { derived, get, writable } from 'svelte/store';
import type { D3Node } from '../types/graph';
import { markPathOf, prunedMarks, toggleMarked } from '../viewmodels/markSet';
import { compactSides } from '../viewmodels/markRelation';

/** The marked scopes, as paths. See `viewmodels/markSet.ts` for why paths. */
export const markedPaths = writable<ReadonlySet<string>>(new Set<string>());

/** How many scopes are marked — the count the toolbar and the collapsed
 *  summary both render, so neither has to subscribe to the set itself. */
export const markCount = derived(markedPaths, ($marks) => $marks.size);

/**
 * How many *distinct* scopes are marked — `markCount` after dropping marks
 * that live inside another one (UI-147).
 *
 * The two counts differ for one gesture and it is a common one: mark a folder,
 * then mark a file inside it. For the drill that is harmless, because
 * `setScopes` compacts and the narrowing is the same either way. For a
 * *relationship* it is not — those two marks name one scope, and offering to
 * relate them would offer to compare a thing with itself.
 */
export const relatableCount = derived(
  markedPaths,
  ($marks) => compactSides([...$marks]).length,
);

/** Can this node be marked at all? The canvas asks before offering the
 *  gesture, so a ⌘-click on a ghost reads as "not that" rather than as a
 *  click that did nothing. */
export function isMarkable(node: D3Node): boolean {
  return markPathOf(node) !== null;
}

/** True when this node's scope is marked — what draws the ring. Matching on
 *  the path rather than the id is what keeps a ring on the same file whether
 *  the canvas is drawing it as a circle of its own or as one entity inside
 *  it. */
export function isMarked(node: D3Node, marks: ReadonlySet<string>): boolean {
  const path = markPathOf(node);
  return path !== null && marks.has(path);
}

/** Flip one node's scope in or out of the set. Returns false when the node
 *  has no scope to mark, so the caller can say so. */
export function toggleMark(node: D3Node): boolean {
  const path = markPathOf(node);
  if (path === null) return false;
  markedPaths.update((marks) => toggleMarked(marks, path));
  return true;
}

/** Drop every mark. Reachable from the toolbar and from `x` on the canvas,
 *  which already means "drop what is selected" in three other panes. */
export function clearMarks(): void {
  markedPaths.set(new Set<string>());
}

/** Drop marks the loaded graph no longer holds — see `prunedMarks`. */
export function pruneMarks(live: ReadonlySet<string>): void {
  markedPaths.update((marks) => prunedMarks(marks, live));
}

/** The marked paths as an array, for the callers that need to pass them on. */
export function markedList(): string[] {
  return [...get(markedPaths)];
}
