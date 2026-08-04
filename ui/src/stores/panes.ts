/**
 * Which columns flank the canvas in the standalone UI, and how wide they are.
 *
 * Three of them: Filters/Quality on the left, entity Details and Description
 * on the right. None of this has a VS Code counterpart — showing and hiding a
 * side view is VS Code's job — so it all persists without anything to fall
 * out of step with.
 *
 * Widths live here rather than as component state because the app has to
 * answer "does the canvas still have room for this pane" before deciding
 * whether to render it, and that sum needs every pane's width in one place.
 */
import { writable, type Writable } from 'svelte/store';

function persisted<T>(key: string, fallback: T, parse: (raw: string) => T): Writable<T> {
  let initial = fallback;
  try {
    const raw = localStorage.getItem(key);
    if (raw !== null) initial = parse(raw);
  } catch { /* SSR / blocked storage */ }
  const store = writable<T>(initial);
  store.subscribe((v) => {
    try { localStorage.setItem(key, String(v)); } catch { /* ignore */ }
  });
  return store;
}

/** Whether the Details column is expanded. Default open: it is where a click
 *  on a node lands, and one click hides it. */
export const detailsPaneOpen = persisted('nao-details-pane-open', true, (v) => v === 'true');

/** Whether the Description column is expanded. Same key as when this lived in
 *  `description.ts`, so an existing preference survives the move. */
export const describePaneOpen = persisted('nao-describe-pane-open', true, (v) => v === 'true');

/** Narrow, deliberately: it is what the pane shrinks to at 1280×800 rather
 *  than vanishing, and a cramped Details column beats no Details column when
 *  promoting it out of the sidebar was the whole point. */
export const DETAILS_MIN_WIDTH = 220;
export const DETAILS_MAX_WIDTH = 560;

export const detailsWidth = persisted('nao-details-width', 340, (v) => {
  const n = Number(v);
  if (!Number.isFinite(n)) return 340;
  return Math.min(DETAILS_MAX_WIDTH, Math.max(DETAILS_MIN_WIDTH, n));
});

/** The Description column is a fixed-width reading pane — its content is one
 *  chain of prose, so there is nothing for extra width to buy. */
export const DESCRIPTION_WIDTH = 300;

/** Below this the canvas stops being the thing you are looking at, so the
 *  right-hand panes give way rather than squeezing it further.
 *
 *  Set so the canvas stays wider than every side column put together at
 *  1280×800, the narrowest window the layout is checked at (UI-020): that
 *  window less the sidebar's 360 and the three 20px toggle strips leaves
 *  Details its 220px minimum, and 640 > 360 + 220.
 *
 *  With the sidebar open, all three columns fit from ~1700px, Details alone
 *  from 1280, and below that the right-hand side is empty. Collapsing the
 *  sidebar moves each of those thresholds down by its width. */
export const MIN_CANVAS_WIDTH = 640;
