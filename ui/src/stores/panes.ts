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

/** Whether the left column is expanded. It was a plain `let` in App until the
 *  shortcut layer needed to open it from a keystroke (`0`), which is a
 *  decision taken outside the component that renders it. */
export const sidebarPaneOpen = persisted('nao-sidebar-pane-open', true, (v) => v === 'true');

/** Which of the sidebar's three tabs is showing. Same move, same reason: `f`,
 *  `q` and `s` switch tabs from outside `Sidebar.svelte`. */
export type SidebarTab = 'filters' | 'quality' | 'settings';
const SIDEBAR_TABS: SidebarTab[] = ['filters', 'quality', 'settings'];
export const sidebarTab = persisted<SidebarTab>('nao-sidebar-tab', 'filters', (v) =>
  SIDEBAR_TABS.includes(v as SidebarTab) ? (v as SidebarTab) : 'filters');

/** Whether the view controls above the canvas are folded away. The key is the
 *  one `CanvasToolbar` used when it owned this as component state, so an
 *  existing preference survives the move. */
export const toolbarCollapsed = persisted('nao-toolbar-collapsed', false, (v) => v === 'true');

/** Whether the Description column is expanded. Same key as when this lived in
 *  `description.ts`, so an existing preference survives the move. */
export const describePaneOpen = persisted('nao-describe-pane-open', true, (v) => v === 'true');

/** Whether the Elevator spec renders as its own pane beside the code canvas
 *  (ADR 0011). Off by default: a project with no `.elv` layer gains nothing
 *  from it, and the single canvas stays what the tool opens as. */
export const splitViewOpen = persisted('nao-split-view-open', false, (v) => v === 'true');

/**
 * Whether the spec pane draws only entities whose `cr:` claims reach code the
 * **analysis scope** has loaded.
 *
 * Off by default, because the pane is a map and a map that loses rows as you
 * re-scope stops being one — you can no longer see that a Category exists
 * outside your current slice. Off does not mean silent: an entity that can
 * show nothing is marked either way, and this only decides whether it is
 * dimmed in place or removed. On is for when you want the two panes to agree
 * exactly.
 *
 * A preference rather than layout, but it lives here because it belongs with
 * `splitViewOpen` — the pane and its one behavioural switch are read together
 * and there is no second consumer of either.
 */
export const followAnalysisScope = persisted('nao-spec-follow-scope', false, (v) => v === 'true');

/** Spec-pane width. Wider floor than Details: the pane draws a graph rather
 *  than reading prose, and below ~260px the tier labels collide with the
 *  nodes. */
export const SPEC_MIN_WIDTH = 260;
export const SPEC_MAX_WIDTH = 720;

export const specWidth = persisted('nao-spec-width', 400, (v) => {
  const n = Number(v);
  if (!Number.isFinite(n)) return 400;
  return Math.min(SPEC_MAX_WIDTH, Math.max(SPEC_MIN_WIDTH, n));
});

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
