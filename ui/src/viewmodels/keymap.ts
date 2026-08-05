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
 * Digits, not letters, and in layout order left-to-right: the number is a
 * position on screen, which is the one mapping nobody has to memorise.
 */
export const PANES: readonly PaneDef[] = [
  { id: 'sidebar', digit: '0', label: 'Sidebar', hint: 'Filters, Quality and Settings' },
  { id: 'graph', digit: '1', label: 'Graph', hint: 'The canvas itself' },
  { id: 'view', digit: '2', label: 'View', hint: 'The view controls above the canvas' },
  { id: 'details', digit: '3', label: 'Details', hint: 'The hovered or pinned entity' },
  { id: 'description', digit: '4', label: 'Description', hint: 'The graph as prose' },
  // Out of layout order, breaking this list's own rule, and the alternative is
  // worse: the spec pane sits between the sidebar and the canvas, so placing
  // it by position would renumber four panes that people already have in
  // their hands. It is also the only optional pane — a project with no `.elv`
  // files never shows it — so a digit that moves with its presence would be
  // the least memorable number of all.
  { id: 'spec', digit: '5', label: 'Spec', hint: 'The Elevator spec, and the code filter it drives' },
];

export const PANE_LABEL: Record<PaneId, string> = {
  sidebar: 'Sidebar', graph: 'Graph', view: 'View', details: 'Details',
  description: 'Description', spec: 'Spec',
};

/**
 * Every command the layer can issue. A closed union rather than free strings
 * so `keymapActions.ts` fails to compile the moment a binding names something
 * nothing runs.
 */
export type Command =
  // global
  | 'pane.focus.sidebar' | 'pane.focus.graph' | 'pane.focus.view'
  | 'pane.focus.details' | 'pane.focus.description' | 'pane.focus.spec'
  | 'help.toggle' | 'help.close' | 'hover.lock' | 'search.focus'
  // any pane that can be collapsed
  | 'pane.collapse'
  // sidebar
  | 'sidebar.tab.filters' | 'sidebar.tab.quality' | 'sidebar.tab.settings'
  // graph
  | 'graph.pin' | 'graph.clear' | 'graph.fit' | 'graph.fitWidth'
  | 'graph.zoomIn' | 'graph.zoomOut' | 'graph.resetZoom'
  | 'graph.toggleView' | 'graph.toggleLabels'
  | 'graph.mark' | 'graph.markDrill'
  // view controls
  | 'view.toggleMode' | 'view.level.entity' | 'view.level.file' | 'view.level.module'
  | 'view.autoFit' | 'view.spacing' | 'view.highlightDepth' | 'view.hoverMode'
  | 'view.labels.node' | 'view.labels.kind' | 'view.labels.link'
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
  { keys: 'Escape', scope: 'global', command: 'help.close', label: 'Close help' },
  { keys: 'l', scope: 'global', command: 'hover.lock', label: 'Freeze hover' },
  { keys: 'mod+k', scope: 'global', command: 'search.focus', label: 'Search' },

  // --- sidebar ---
  { keys: 'f', scope: 'sidebar', command: 'sidebar.tab.filters', label: 'Filters' },
  { keys: 'q', scope: 'sidebar', command: 'sidebar.tab.quality', label: 'Quality' },
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

  // --- view controls ---
  { keys: 't', scope: 'view', command: 'view.toggleMode', label: 'Tree/Graph' },
  { keys: 'e', scope: 'view', command: 'view.level.entity', label: 'Entity level' },
  { keys: 'f', scope: 'view', command: 'view.level.file', label: 'File level' },
  { keys: 'm', scope: 'view', command: 'view.level.module', label: 'Module level' },
  { keys: 'a', scope: 'view', command: 'view.autoFit', label: 'Auto-fit' },
  { keys: 's', scope: 'view', command: 'view.spacing', label: 'Tree spacing' },
  { keys: 'd', scope: 'view', command: 'view.highlightDepth', label: 'Highlight depth' },
  { keys: 'o', scope: 'view', command: 'view.hoverMode', label: 'Hover mode' },
  { keys: 'n', scope: 'view', command: 'view.labels.node', label: 'Node labels' },
  { keys: 'k', scope: 'view', command: 'view.labels.kind', label: 'Kind labels' },
  { keys: 'b', scope: 'view', command: 'view.labels.link', label: 'Link labels' },
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
