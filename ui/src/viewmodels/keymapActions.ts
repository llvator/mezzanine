/**
 * What each command in the keymap actually does.
 *
 * The split from `keymap.ts` is deliberate: that file is a list a reader (and
 * a test) can inspect, this one is the only place a keystroke reaches a store.
 * Every action here is something a control on screen already does — the layer
 * gives those controls a key, it does not invent behaviour that exists nowhere
 * else. When the two disagree the button is right and this is the bug.
 *
 * One function per scope rather than one 30-arm switch: the arms have nothing
 * in common beyond being reachable from a keyboard, and a single switch would
 * be the largest function in `ui/` for no gain in cohesion.
 */
import { get } from 'svelte/store';
import type { Command, PaneId } from './keymap';
import {
  selectedNode, hoveredNode, hoverLocked, graphLevel, hoverDepth, hoverMode,
  showLabels, showKindLabels, showLinkLabels, cycleTreeDensity,
} from '../stores/graph';
import { autoLevel, drillIntoMarks } from '../stores/scope';
import { clearMarks, toggleMark } from '../stores/marks';
import { autoFitView } from '../stores/settings';
import { describeOnHover } from '../stores/description';
import {
  sidebarPaneOpen, sidebarTab, toolbarCollapsed, detailsPaneOpen, describePaneOpen,
  splitViewOpen,
} from '../stores/panes';
import { clearSpecFocus } from '../stores/crossFilter';
import { focusedPane, focusPane, shortcutHelpOpen, requestSearchFocus } from '../stores/keymap';
import { HOVER_MODES } from './hoverHighlight';
import type { GraphLevel } from '../types/graph';

/**
 * The bits of `GraphView` the keyboard drives. Structural rather than the
 * component type: these actions run from a plain `.ts` module, and the
 * viewport is the one thing they cannot reach through a store.
 */
export interface GraphViewApi {
  zoomIn(): void;
  zoomOut(): void;
  resetZoom(): void;
  fitView(): void;
  fitWidth(): void;
  toggleViewMode(): void;
}

export interface KeymapContext {
  /** Undefined until App's `bind:this` lands — a key pressed in that window
   *  does nothing rather than throwing. */
  graphView?: GraphViewApi;
}

/** Pin the level the way the toolbar does: an explicit choice turns auto-level
 *  off, or the next scope change would silently overrule it. */
function setLevel(level: GraphLevel): void {
  autoLevel.set(false);
  graphLevel.set(level);
}

function runGlobal(command: Command): boolean {
  switch (command) {
    case 'pane.focus.sidebar': focusPane('sidebar'); return true;
    case 'pane.focus.graph': focusPane('graph'); return true;
    case 'pane.focus.view': focusPane('view'); return true;
    case 'pane.focus.details': focusPane('details'); return true;
    case 'pane.focus.description': focusPane('description'); return true;
    case 'pane.focus.spec': focusPane('spec'); return true;
    case 'help.toggle': shortcutHelpOpen.update((v) => !v); return true;
    case 'help.close': shortcutHelpOpen.set(false); return true;
    case 'hover.lock': hoverLocked.update((v) => !v); return true;
    case 'search.focus':
      // The box lives in the sidebar's Filters tab, so getting there is part
      // of the command — `/` from a collapsed sidebar has to end with a caret
      // in a visible field, not a focus call into a hidden one.
      focusPane('sidebar');
      sidebarTab.set('filters');
      requestSearchFocus();
      return true;
    case 'pane.collapse': collapseFocusedPane(); return true;
    default: return false;
  }
}

/** `c` in whichever pane has focus. The canvas has no collapsed state — it is
 *  what the panes flank — so there it does nothing. */
function collapseFocusedPane(): void {
  const pane: PaneId = get(focusedPane);
  switch (pane) {
    case 'sidebar': sidebarPaneOpen.set(false); break;
    case 'view': toolbarCollapsed.set(true); break;
    case 'details': detailsPaneOpen.set(false); break;
    case 'description': describePaneOpen.set(false); break;
    case 'spec': splitViewOpen.set(false); break;
    case 'graph': break;
  }
  // Focus follows the reader, not the pane that just went away.
  if (pane !== 'graph') focusedPane.set('graph');
}

function runSidebar(command: Command): boolean {
  switch (command) {
    case 'sidebar.tab.filters': sidebarTab.set('filters'); return true;
    case 'sidebar.tab.quality': sidebarTab.set('quality'); return true;
    case 'sidebar.tab.settings': sidebarTab.set('settings'); return true;
    default: return false;
  }
}

function runGraph(command: Command, ctx: KeymapContext): boolean {
  const view = ctx.graphView;
  switch (command) {
    case 'graph.pin': {
      // The hovered node, which under `l` is the frozen one — so the pair
      // reads as "hold this, then keep it".
      const hovered = get(hoveredNode);
      if (hovered) selectedNode.set(hovered);
      return true;
    }
    // `x` means "drop what is selected" in the spec and details panes too, and
    // the marked set is a selection — leaving it behind would make the one key
    // that promises a clean canvas the one that quietly does not.
    case 'graph.clear': selectedNode.set(null); clearMarks(); return true;
    case 'graph.mark': {
      // The hovered node, exactly as `graph.pin` above: the same reach, and
      // the difference between the two keys is what you meant by it.
      const hovered = get(hoveredNode);
      if (hovered) toggleMark(hovered);
      return true;
    }
    case 'graph.markDrill': void drillIntoMarks(); return true;
    case 'graph.fit': view?.fitView(); return true;
    case 'graph.fitWidth': view?.fitWidth(); return true;
    case 'graph.zoomIn': view?.zoomIn(); return true;
    case 'graph.zoomOut': view?.zoomOut(); return true;
    case 'graph.resetZoom': view?.resetZoom(); return true;
    case 'graph.toggleView': view?.toggleViewMode(); return true;
    case 'graph.toggleLabels': showLabels.update((v) => !v); return true;
    default: return false;
  }
}

function runView(command: Command, ctx: KeymapContext): boolean {
  switch (command) {
    case 'view.toggleMode': ctx.graphView?.toggleViewMode(); return true;
    case 'view.level.entity': setLevel('entity'); return true;
    case 'view.level.file': setLevel('file'); return true;
    case 'view.level.module': setLevel('module'); return true;
    case 'view.autoFit': autoFitView.update((v) => !v); return true;
    case 'view.spacing': cycleTreeDensity(); return true;
    case 'view.highlightDepth': hoverDepth.update((d) => (d % 3) + 1); return true;
    case 'view.hoverMode':
      hoverMode.update((m) => HOVER_MODES[(HOVER_MODES.indexOf(m) + 1) % HOVER_MODES.length]);
      return true;
    case 'view.labels.node': showLabels.update((v) => !v); return true;
    case 'view.labels.kind': showKindLabels.update((v) => !v); return true;
    case 'view.labels.link': showLinkLabels.update((v) => !v); return true;
    default: return false;
  }
}

/** The cross-filter is the spec pane's selection, so `x` drops it — the same
 *  word `x` has in the graph and details panes. */
function runSpec(command: Command): boolean {
  if (command === 'spec.clear') {
    clearSpecFocus();
    return true;
  }
  return false;
}

function runDetails(command: Command): boolean {
  switch (command) {
    case 'details.pin': {
      // Same toggle as the pin button, and deliberately so: two ways to reach
      // one behaviour, not two behaviours.
      const pinned = get(selectedNode);
      if (pinned) selectedNode.set(null);
      else {
        const hovered = get(hoveredNode);
        if (hovered) selectedNode.set(hovered);
      }
      return true;
    }
    case 'details.clear': selectedNode.set(null); return true;
    default: return false;
  }
}

function runDescription(command: Command): boolean {
  if (command === 'description.onHover') {
    describeOnHover.update((v) => !v);
    return true;
  }
  return false;
}

/**
 * Run a command. Returns whether anything handled it, which is what the
 * dispatcher uses to decide about `preventDefault` — a key nothing consumed
 * must stay the browser's.
 */
export function runCommand(command: Command, ctx: KeymapContext = {}): boolean {
  return runGlobal(command)
    || runSidebar(command)
    || runGraph(command, ctx)
    || runView(command, ctx)
    || runSpec(command)
    || runDetails(command)
    || runDescription(command);
}
