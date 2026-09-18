/**
 * The keymap: which key does what, and where.
 *
 * Modelled on lazygit. Every binding belongs to a *scope* — one of the five
 * panes, or `global` — and only the focused pane's bindings plus the global
 * ones are live at any moment. That is what makes a one-letter key affordable:
 * `f` can mean "fit the view" on the canvas and "Filters tab" in the sidebar
 * without either one being a compromise, because the reader can always see
 * which set is active (`ShortcutBar`) and ask for the whole map (`?`).
 *
 * This module is data and matching only — no stores, no DOM. Two reasons: the
 * shortcut bar and the help overlay both render the same list, so it has to be
 * inspectable rather than hidden in a handler; and the invariants that keep the
 * layer honest (no two bindings on one key in one scope, no pane key shadowing
 * a navigation digit) are then unit-testable without a browser. The commands
 * are *names* here; `keymapActions.ts` is the only place that runs anything.
 */

/** The five panes, in the order their digits run. */
export type PaneId = 'sidebar' | 'spec' | 'graph' | 'view' | 'details' | 'description';

/** A binding is live in exactly one pane, or everywhere. */
export type Scope = PaneId | 'global';

export interface PaneDef {
  id: PaneId;
  /** The digit that focuses it. Also its position in the shortcut bar. */
  digit: string;
  label: string;
  /** One line, for the help overlay. */
  hint: string;
}

/**
 * Digits, not letters, and near enough to layout order to be guessable: the
 * number is a position on screen, which is the one mapping nobody has to
 * memorise.
 *
 * The one pane not at its screen position is the canvas. `1` is the graph
 * because it is what the app opens onto and what every other pane is read
 * *against* — geometry would hand that digit to the spec pane, which a project
 * without `.elv` files never draws at all. So the canvas keeps `1`, the spec
 * pane takes `2` as the column beside it, and the rest run left to right from
 * there. Appending Spec at `5` (its first home, when the pane was new) put the
 * two canvases at opposite ends of the row for no reason a reader could see.
 */
export const PANES: readonly PaneDef[] = [
  { id: 'sidebar', digit: '0', label: 'Sidebar', hint: 'Filters, Quality and Settings' },
  { id: 'graph', digit: '1', label: 'Graph', hint: 'The canvas itself' },
  { id: 'spec', digit: '2', label: 'Spec', hint: 'The Elevator spec, and the code filter it drives' },
  { id: 'view', digit: '3', label: 'View', hint: 'The view controls above the canvas' },
  { id: 'details', digit: '4', label: 'Details', hint: 'The hovered or pinned entity' },
  { id: 'description', digit: '5', label: 'Description', hint: 'The graph as prose' },
];

export const PANE_LABEL: Record<PaneId, string> = {
  sidebar: 'Sidebar', graph: 'Graph', view: 'View', details: 'Details',
  description: 'Description', spec: 'Spec',
};

/**
 * The key that focuses each pane, by id.
 *
 * Derived rather than written out, because the collapsed strips print it: a
 * second hand-kept copy of the digits is exactly the kind of thing that ends
 * up telling the reader to press a key that moves them somewhere else.
 */
export const PANE_DIGIT: Record<PaneId, string> =
  Object.fromEntries(PANES.map((p) => [p.id, p.digit])) as Record<PaneId, string>;

/**
 * Every command the layer can issue. A closed union rather than free strings
 * so `keymapActions.ts` fails to compile the moment a binding names something
 * nothing runs.
 */
export type Command =
  // global
  | 'pane.focus.sidebar' | 'pane.focus.graph' | 'pane.focus.view'
  | 'pane.focus.details' | 'pane.focus.description' | 'pane.focus.spec'
  | 'help.toggle' | 'ui.dismiss' | 'hover.lock' | 'search.focus'
  | 'history.back' | 'history.forward' | 'pane.expand'
  // any pane that can be collapsed
  | 'pane.collapse'
  // sidebar
  | 'sidebar.tab.filters' | 'sidebar.tab.quality' | 'sidebar.tab.changes'
  | 'sidebar.tab.settings'
  // graph
  | 'graph.pin' | 'graph.clear' | 'graph.fit' | 'graph.fitWidth'
  | 'graph.zoomIn' | 'graph.zoomOut' | 'graph.resetZoom'
  | 'graph.toggleView' | 'graph.toggleLabels'
  | 'graph.mark' | 'graph.markDrill' | 'graph.markRelate'
  // view controls
  | 'view.toggleMode' | 'view.level.entity' | 'view.level.file' | 'view.level.folder'
  | 'view.autoFit' | 'view.spacing' | 'view.highlightDepth' | 'view.hoverMode'
  | 'view.labels.node' | 'view.labels.kind' | 'view.labels.link'
  | 'view.structureOnly'
  // spec
  | 'spec.clear'
  // details
  | 'details.pin' | 'details.clear'
  // description
  | 'description.onHover';

export interface Binding {
  /** `p`, `?`, `mod+k`, `shift+p`. `mod` is Cmd on macOS, Ctrl elsewhere. */
  keys: string;
  scope: Scope;
  command: Command;
  /** Imperative and short — it has to fit in a bar chip. */
  label: string;
}

/**
 * The map.
 *
 * Grouped by scope so the file reads the way the help overlay does. Keys are
 * reused across panes on purpose (`c` collapses whichever pane you are in,
 * `p` pins from both the canvas and the Details pane); what is never reused is
 * a key across a pane *and* the global set, which would make the meaning of a
 * keystroke depend on focus in a way the bar cannot show.
 */
export const BINDINGS: readonly Binding[] = [
  // --- global ---
  ...PANES.map((p): Binding => ({
    keys: p.digit,
    scope: 'global',
    command: `pane.focus.${p.id}` as Command,
    label: p.label,
  })),
  { keys: '?', scope: 'global', command: 'help.toggle', label: 'Keys' },
  // Escape backs out of whatever is in front of the reader: the help overlay
  // while it is up, otherwise the pane holding focus. Global rather than one
  // binding per pane, and not because it means the same thing everywhere — it
  // does not — but because the overlay covers every pane, so a pane-scoped
  // Escape would win the resolution order and leave `?` with no way out.
  // The per-pane `c` stays: it is the one that reads as "collapse" rather than
  // as "get me out of here", and the bar can only advertise a pane's own keys.
  { keys: 'Escape', scope: 'global', command: 'ui.dismiss', label: 'Close / collapse' },
  { keys: 'l', scope: 'global', command: 'hover.lock', label: 'Freeze hover' },
  { keys: 'mod+k', scope: 'global', command: 'search.focus', label: 'Search' },
  // Global rather than canvas-scoped, and the reason is where the gestures
  // are: a scope is committed from the sidebar, a region focused from the
  // canvas, a view restored from the sidebar again. A key that only worked
  // in the pane the last navigation happened to come from would be a key
  // nobody could rely on. `[` and `]` are the two brackets no other binding
  // wants, and they read as directions on every keyboard layout that has
  // them together.
  { keys: '[', scope: 'global', command: 'history.back', label: 'Back' },
  { keys: ']', scope: 'global', command: 'history.forward', label: 'Forward' },
  // Global for the same reason the digits are: it changes what focus *means*
  // for every pane at once, so binding it inside one of them would make the
  // mode reachable only from wherever you happened to be standing (UI-094).
  { keys: 'z', scope: 'global', command: 'pane.expand', label: 'Expand focused' },

  // --- sidebar ---
  { keys: 'f', scope: 'sidebar', command: 'sidebar.tab.filters', label: 'Filters' },
  { keys: 'q', scope: 'sidebar', command: 'sidebar.tab.quality', label: 'Quality' },
  // `g` for git: `c` is the sidebar's collapse key, and the tab is a reading
  // of what git reports rather than of the graph (UI-134).
  { keys: 'g', scope: 'sidebar', command: 'sidebar.tab.changes', label: 'Changes' },
  { keys: 's', scope: 'sidebar', command: 'sidebar.tab.settings', label: 'Settings' },
  { keys: '/', scope: 'sidebar', command: 'search.focus', label: 'Search' },
  { keys: 'c', scope: 'sidebar', command: 'pane.collapse', label: 'Collapse' },

  // --- graph ---
  { keys: 'p', scope: 'graph', command: 'graph.pin', label: 'Pin hovered' },
  { keys: 'x', scope: 'graph', command: 'graph.clear', label: 'Clear selection' },
  { keys: 'f', scope: 'graph', command: 'graph.fit', label: 'Fit view' },
  { keys: 'w', scope: 'graph', command: 'graph.fitWidth', label: 'Fit width' },
  { keys: '+', scope: 'graph', command: 'graph.zoomIn', label: 'Zoom in' },
  { keys: '=', scope: 'graph', command: 'graph.zoomIn', label: 'Zoom in' },
  { keys: '-', scope: 'graph', command: 'graph.zoomOut', label: 'Zoom out' },
  { keys: 'r', scope: 'graph', command: 'graph.resetZoom', label: 'Reset zoom' },
  { keys: 't', scope: 'graph', command: 'graph.toggleView', label: 'Tree/Graph' },
  { keys: 'n', scope: 'graph', command: 'graph.toggleLabels', label: 'Node labels' },
  // The keyboard half of ⌘-click. `m` marks what `p` would pin, so the two
  // read as the same reach with two different intents; `d` spends the set.
  // `m` is the module level in the *view* pane and free here, which is the
  // whole point of scoped bindings.
  { keys: 'm', scope: 'graph', command: 'graph.mark', label: 'Mark hovered' },
  { keys: 'd', scope: 'graph', command: 'graph.markDrill', label: 'Drill into marks' },
  // The set's other use (UI-147): `d` narrows to it, `v` asks about it and
  // leaves the canvas alone. `v` for versus — the reading is about the space
  // between two scopes, and the two verbs sit next to each other so a reader
  // who knows one finds the other. `r` would have read better and is the
  // canvas's zoom reset.
  { keys: 'v', scope: 'graph', command: 'graph.markRelate', label: 'Relate marks' },

  // --- view controls ---
  { keys: 't', scope: 'view', command: 'view.toggleMode', label: 'Tree/Graph' },
  { keys: 'e', scope: 'view', command: 'view.level.entity', label: 'Entity level' },
  { keys: 'f', scope: 'view', command: 'view.level.file', label: 'File level' },
  { keys: 'm', scope: 'view', command: 'view.level.folder', label: 'Folder level' },
  { keys: 'a', scope: 'view', command: 'view.autoFit', label: 'Auto-fit' },
  { keys: 's', scope: 'view', command: 'view.spacing', label: 'Tree spacing' },
  { keys: 'd', scope: 'view', command: 'view.highlightDepth', label: 'Highlight depth' },
  { keys: 'o', scope: 'view', command: 'view.hoverMode', label: 'Hover mode' },
  { keys: 'n', scope: 'view', command: 'view.labels.node', label: 'Node labels' },
  { keys: 'k', scope: 'view', command: 'view.labels.kind', label: 'Kind labels' },
  { keys: 'b', scope: 'view', command: 'view.labels.link', label: 'Link labels' },
  // `i` for internals — the one control that changes what a scope *is* when
  // you open it, so it earns a key rather than a scroll into the Filters pane
  // (UI-113).
  { keys: 'i', scope: 'view', command: 'view.structureOnly', label: 'Internals' },
  { keys: 'c', scope: 'view', command: 'pane.collapse', label: 'Collapse' },

  // --- spec ---
  // `x` for "drop what is selected", the same word it has in the graph and
  // details panes. The cross-filter *is* the spec pane's selection.
  { keys: 'x', scope: 'spec', command: 'spec.clear', label: 'Clear filter' },
  { keys: 'c', scope: 'spec', command: 'pane.collapse', label: 'Collapse' },

  // --- details ---
  { keys: 'p', scope: 'details', command: 'details.pin', label: 'Pin/unpin' },
  { keys: 'x', scope: 'details', command: 'details.clear', label: 'Unpin' },
  { keys: 'c', scope: 'details', command: 'pane.collapse', label: 'Collapse' },

  // --- description ---
  { keys: 'h', scope: 'description', command: 'description.onHover', label: 'Follow hover' },
  { keys: 'c', scope: 'description', command: 'pane.collapse', label: 'Collapse' },
];

/** Enough of a KeyboardEvent to match on, so tests need no DOM. */
export interface KeyEventLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
}

interface KeySpec {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
}

/**
 * `mod+k` → `{key: 'k', mod: true}`. The last `+`-separated segment is the
 * key, which leaves `+` itself as an empty final segment — the zoom-in binding
 * depends on that, so it is handled rather than left to produce a spec that
 * matches nothing.
 */
export function parseKeys(spec: string): KeySpec {
  const parts = spec.split('+');
  let key = parts.pop() ?? '';
  if (key === '') key = '+';
  const mods = new Set(parts.filter((p) => p !== ''));
  return { key, mod: mods.has('mod'), shift: mods.has('shift'), alt: mods.has('alt') };
}

/**
 * Shift is not compared unless a binding asks for it: on most layouts `?` and
 * `+` *are* shifted keys, and `event.key` has already resolved them. Cmd and
 * Ctrl are one modifier (`mod`) because this UI has no binding that means one
 * thing on macOS and another elsewhere.
 */
function specMatches(spec: KeySpec, ev: KeyEventLike): boolean {
  if (spec.key.toLowerCase() !== ev.key.toLowerCase()) return false;
  if (spec.mod !== !!(ev.metaKey || ev.ctrlKey)) return false;
  if (spec.alt !== !!ev.altKey) return false;
  if (spec.shift && !ev.shiftKey) return false;
  return true;
}

/** True if the binding carries Cmd/Ctrl, which is what lets it fire while a
 *  text field has focus. */
export function needsModifier(binding: Binding): boolean {
  return parseKeys(binding.keys).mod;
}

/** Bindings live in a scope, in declaration order — which is the order the bar
 *  and the overlay print them in. */
export function bindingsForScope(scope: Scope): Binding[] {
  return BINDINGS.filter((b) => b.scope === scope);
}

/**
 * Resolve a keystroke against the focused pane.
 *
 * Pane first, then global. Nothing currently collides — `noCollisions` in the
 * test suite holds that line — but the order is the contract a future binding
 * is added under: a pane may take a key back, and never the reverse.
 *
 * `typing` is the caller's answer to "is a text field focused". While typing,
 * only modifier bindings resolve, so `p` reaches the search box and `Cmd-K`
 * still reaches the app.
 */
export function matchBinding(ev: KeyEventLike, pane: PaneId, typing: boolean): Binding | null {
  const candidates = [...bindingsForScope(pane), ...bindingsForScope('global')];
  for (const b of candidates) {
    if (typing && !needsModifier(b)) continue;
    if (specMatches(parseKeys(b.keys), ev)) return b;
  }
  return null;
}

/** Element-shaped enough to classify without a DOM. */
export interface TargetLike {
  tagName?: string;
  isContentEditable?: boolean;
}

/** Is the keystroke going into a text field? Selects and checkboxes are not
 *  text fields — arrow keys matter there, single letters do not. */
export function isTypingTarget(el: TargetLike | null | undefined): boolean {
  if (!el) return false;
  if (el.isContentEditable) return true;
  const tag = (el.tagName ?? '').toUpperCase();
  return tag === 'INPUT' || tag === 'TEXTAREA';
}

/** How a binding prints: `mod+k` becomes `⌘K` on macOS, `Ctrl-K` elsewhere. */
export function displayKeys(spec: string, mac: boolean): string {
  const { key, mod, shift, alt } = parseKeys(spec);
  const label = key.length === 1 ? key.toUpperCase() : key;
  let out = label;
  if (shift) out = (mac ? '⇧' : 'Shift-') + out;
  if (alt) out = (mac ? '⌥' : 'Alt-') + out;
  if (mod) out = (mac ? '⌘' : 'Ctrl-') + out;
  return out;
}
