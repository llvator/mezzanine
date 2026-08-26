/**
 * State behind the overview panel — the whole drawn graph in a corner of the
 * canvas, with a box around the part the viewport is showing.
 *
 * Two live stores and one preference, all in one file because they are one
 * feature. `GraphView` is the only writer of the live pair: it owns the zoom
 * behaviour and the force simulation, and both facts here are readings off
 * those. `OverviewPanel` is the only reader.
 *
 * A store rather than a prop chain because the panel renders inside
 * `GraphView`'s slot, which puts `App` between the two — and `App` has no
 * business relaying a frame it never looks at. It goes the other way as a
 * method call (`graphView.panTo`), following `CanvasToolbar`, since a command
 * has one recipient and doesn't need broadcasting.
 */
import { writable } from 'svelte/store';
import type { OverviewDot, ViewTransform } from '../viewmodels/overviewFrame';

/** The dots the panel draws, in world coordinates.
 *
 *  Empty rather than null when the canvas is empty: the panel folds itself
 *  away on an empty frame, and "nothing drawn" and "not yet published" call
 *  for the same picture, so there is no second state to carry. */
export const overviewDots = writable<OverviewDot[]>([]);

/**
 * Where the viewport is, as the transform plus the size it applies to.
 *
 * The size travels with the transform rather than being read off the DOM by
 * the panel. A transform means nothing without the viewport it maps into, and
 * the panel does not know the canvas's size — it knows its own. Publishing
 * them together also means one store write per change instead of two that can
 * be observed half-applied, which on a resize is a visibly wrong box.
 *
 * Null until the first zoom event, which `GraphView` triggers on mount by
 * applying the identity transform.
 */
export interface CanvasViewport extends ViewTransform {
  /** Canvas size in pixels at the moment the transform was read. */
  w: number;
  h: number;
}

export const canvasViewport = writable<CanvasViewport | null>(null);

/**
 * Whether the panel is showing.
 *
 * Default on: it costs the canvas a corner and it is the answer to a question
 * ("where am I?") a reader only thinks to ask once they are already lost —
 * off by default means it is never there when it is wanted. The corner is
 * cheap to give back and the choice sticks.
 */
const OPEN_KEY = 'mezz-overview-open';

function initialOpen(): boolean {
  try {
    const raw = localStorage.getItem(OPEN_KEY);
    if (raw !== null) return raw === 'true';
  } catch { /* SSR / blocked storage */ }
  return true;
}

export const overviewOpen = writable<boolean>(initialOpen());
overviewOpen.subscribe((v) => {
  try { localStorage.setItem(OPEN_KEY, String(v)); } catch { /* ignore */ }
});
