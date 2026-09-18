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
 *
 * The *arithmetic* of that question moved to `viewmodels/paneLayout.ts` in
 * UI-093, along with the floors it is expressed in. What is left here is
 * persistence: what the reader last asked for, so the next window opens on it.
 */
import { writable, type Writable } from 'svelte/store';
import {
  DETAILS_MIN_WIDTH, SPEC_MIN_WIDTH, DESCRIPTION_MIN_WIDTH, SIDEBAR_MIN_WIDTH,
} from '../viewmodels/paneLayout';

/**
 * Which browser store a preference lives in, and therefore how far it reaches.
 *
 * `local` is per-browser: every window of it shares one value, so the last
 * window to change something decides what the next one opens on. `session` is
 * per-tab and survives a reload of that tab, which is the scope a *window's*
 * layout actually wants now that two windows are a supported way to work
 * (UI-095).
 *
 * The existing flags stay on `local`. Changing them would silently drop
 * everyone's remembered layout, and being handed the other window's pane
 * widths is a small wrong compared with what it buys.
 */
type Area = 'local' | 'session';

function areaOf(area: Area): Storage | null {
  try {
    return area === 'local' ? localStorage : sessionStorage;
  } catch {
    return null; // SSR / blocked storage
  }
}

function persistedIn<T>(area: Area, key: string, fallback: T, parse: (raw: string) => T): Writable<T> {
  let initial = fallback;
  const raw = areaOf(area)?.getItem(key);
  if (raw !== null && raw !== undefined) initial = parse(raw);
  const store = writable<T>(initial);
  store.subscribe((v) => {
    try { areaOf(area)?.setItem(key, String(v)); } catch { /* quota, private mode */ }
  });
  return store;
}

function persisted<T>(key: string, fallback: T, parse: (raw: string) => T): Writable<T> {
  return persistedIn('local', key, fallback, parse);
}

/**
 * A remembered on/off switch.
 *
 * Exported because `stores/mirror.ts` needs one too, and every store in this
 * folder that wanted a persisted boolean has so far written its own
 * `try { localStorage… } catch` pair. Naming the one in this file rather than
 * adding a fourth copy elsewhere; a shared `stores/persisted.ts` would be the
 * tidier home if a fifth consumer ever turns up.
 */
export function persistedFlag(key: string, fallback: boolean): Writable<boolean> {
  return persisted(key, fallback, (v) => v === 'true');
}

/**
 * Whether this window draws the canvas at all (UI-098).
 *
 * The canvas used to be the one thing that could not be closed — it was what
 * the panes flanked, and closing it left a window with nothing in the middle.
 * Two windows changed that: one can hold the graph while the other holds the
 * panes that read it, and the second one has no use for a 640px floor it
 * cannot fill. Closing it is also what makes all five panes reachable at once
 * across the pair, which no single window under 1640px could manage.
 *
 * **Per-tab, unlike every other flag here.** `localStorage` is shared by every
 * window of the browser, so a pane window closing its canvas would leave the
 * canvas window opening without one after a reload — losing the graph, in the
 * window whose whole job is the graph. `sessionStorage` is scoped to the tab
 * and survives its reloads, which is exactly the reach this switch wants.
 */
export const canvasPaneOpen = persistedIn('session', 'mezz-canvas-pane-open', true, (v) => v === 'true');

/** Whether the Details column is expanded. Default open: it is where a click
 *  on a node lands, and one click hides it. */
export const detailsPaneOpen = persisted('mezz-details-pane-open', true, (v) => v === 'true');

/** Whether the left column is expanded. It was a plain `let` in App until the
 *  shortcut layer needed to open it from a keystroke (`0`), which is a
 *  decision taken outside the component that renders it. */
export const sidebarPaneOpen = persisted('mezz-sidebar-pane-open', true, (v) => v === 'true');

/** Which of the sidebar's three tabs is showing. Same move, same reason: `f`,
 *  `q` and `s` switch tabs from outside `Sidebar.svelte`. */
export type SidebarTab = 'filters' | 'quality' | 'changes' | 'settings';
const SIDEBAR_TABS: SidebarTab[] = ['filters', 'quality', 'changes', 'settings'];
export const sidebarTab = persisted<SidebarTab>('mezz-sidebar-tab', 'filters', (v) =>
  SIDEBAR_TABS.includes(v as SidebarTab) ? (v as SidebarTab) : 'filters');

/** Whether the view controls above the canvas are folded away. The key is the
 *  one `CanvasToolbar` used when it owned this as component state, so an
 *  existing preference survives the move. */
export const toolbarCollapsed = persisted('mezz-toolbar-collapsed', false, (v) => v === 'true');

/** Whether the Description column is expanded. Same key as when this lived in
 *  `description.ts`, so an existing preference survives the move. */
export const describePaneOpen = persisted('mezz-describe-pane-open', true, (v) => v === 'true');

/**
 * Whether the changes row under the columns is expanded (UI-150).
 *
 * Default open, because it is what it replaced: the branch chip and the
 * comparison buttons were always on screen when they floated over the canvas,
 * and a reader who never finds the chevron should lose nothing. Unlike the
 * columns it costs height rather than width, so folding it is a trade against
 * the canvas rather than against another pane — which is why it gets its own
 * flag instead of a slot in the width budget.
 */
export const changesBarOpen = persisted('mezz-changes-bar-open', true, (v) => v === 'true');

/** Whether the Elevator spec renders as its own pane beside the code canvas
 *  (ADR 0011). Off by default: a project with no `.elv` layer gains nothing
 *  from it, and the single canvas stays what the tool opens as. */
export const splitViewOpen = persisted('mezz-split-view-open', false, (v) => v === 'true');

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
export const followAnalysisScope = persisted('mezz-spec-follow-scope', false, (v) => v === 'true');

/**
 * Whether a click in the spec pane also filters the code canvas.
 *
 * On by default, because that is what the click has always done and because
 * narrowing to what an entity owns is the reason the pane is beside the graph
 * at all. The switch exists because the click carries a second job it cannot
 * decline: it is also the only way to read an entity's description, so a
 * reader who wanted the Details pane paid for it by having the canvas emptied
 * to that entity's files. Off, the click opens, drills and moves Details, and
 * the canvas is left where it was — the highlight channel (hover, and pins)
 * being what answers "where does this live" without deleting the context the
 * question is about.
 *
 * Beside `followAnalysisScope` for the same reason it is: a spec-pane
 * behaviour switch with exactly one reader, remembered between sessions.
 */
export const filterOnSpecClick = persisted('mezz-spec-filter-on-click', true, (v) => v !== 'false');

/**
 * Whether clicking a row in the Changes pane also seeds the code canvas.
 *
 * The same switch as `filterOnSpecClick`, one pane over, for the same reason:
 * the click carries two jobs and only one of them was ever asked for every
 * time. Opening a file's diff is why a reader clicks the row; making its node
 * the graph's selection re-roots the tree on it, and the rest of the change —
 * which is the list the reader is working down — leaves the picture.
 *
 * On by default, that being what the click has always done. Off, the click
 * still opens the diff, still marks the row and still lights the file's
 * circles on the canvas (`changeHighlight`), and the seed, the ladder and the
 * scope stay exactly where the reader left them.
 */
export const filterOnChangeClick = persisted('mezz-changes-filter-on-click', true, (v) => v !== 'false');

/**
 * Whether the Changes list is drawn as a folder tree rather than flat (UI-154).
 *
 * Two readings of one comparison, and which one is wanted depends on the
 * question: a flat list is the shorter walk when the reader is working down
 * every file, and a tree is the only one of the two that says *where* the
 * change happened — three edits in one folder and three spread across the repo
 * are the same list and very different changes.
 *
 * Off by default, that being what the pane has always drawn, and remembered
 * per browser because it is a reading habit rather than a property of any one
 * comparison.
 */
export const changesAsTree = persisted('mezz-changes-as-tree', false, (v) => v === 'true');

/**
 * What each column remembers being dragged to.
 *
 * Only a floor is enforced on the way in. There is no stored maximum any more
 * (UI-093): a width is a preference, and the window it is being read on is
 * what decides how much of it can be honoured — `layoutPanes` clamps what is
 * *shown* without touching what is *stored*, so a pane dragged wide on a
 * monitor survives a trip through a laptop.
 */
function width(key: string, fallback: number, min: number): Writable<number> {
  return persisted(key, fallback, (v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) return fallback;
    return Math.max(min, n);
  });
}

export const specWidth = width('mezz-spec-width', 400, SPEC_MIN_WIDTH);
export const detailsWidth = width('mezz-details-width', 340, DETAILS_MIN_WIDTH);

/** The Description column was a fixed 300px with no handle at all — the pane
 *  holding the most prose was the one you could not widen (UI-093). The key is
 *  new because there was never a stored value to inherit. */
export const describeWidth = width('mezz-describe-width', 300, DESCRIPTION_MIN_WIDTH);

/** The sidebar was a plain `let` in `App.svelte`, so it forgot its width on
 *  every reload while the two panes beside it remembered theirs. */
export const sidebarWidth = width('mezz-sidebar-width', 360, SIDEBAR_MIN_WIDTH);

/**
 * Whether the focused pane grows into the slack (UI-094).
 *
 * Off by default: it moves columns in response to a keystroke that used to
 * move only a focus ring, and a layout that rearranges itself is something to
 * opt into rather than to discover. What it does to the arithmetic is in
 * `viewmodels/paneLayout.ts`; all that lives here is whether it is on.
 */
export const focusExpand = persisted('mezz-focus-expand', false, (v) => v === 'true');
