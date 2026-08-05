/**
 * Where the keyboard is pointing, and what the shortcut layer is showing.
 *
 * Separate from `panes.ts`, which owns whether a column is open and how wide:
 * that is layout, this is focus. They meet in `focusPane`, because a pane you
 * cannot see cannot usefully hold the keyboard — asking for Details with `3`
 * opens Details.
 */
import { writable } from 'svelte/store';
import type { PaneId } from '../viewmodels/keymap';
import {
  detailsPaneOpen, describePaneOpen, sidebarPaneOpen, toolbarCollapsed, splitViewOpen,
} from './panes';

/**
 * The pane the keyboard is aimed at. Starts on the canvas: it is what the app
 * opens onto, and every pane is one digit away.
 *
 * Not persisted. Focus is a property of a session at the keyboard, and
 * restoring it into a window whose panes have been resized or closed since
 * would put the caret somewhere the reader did not leave it.
 */
export const focusedPane = writable<PaneId>('graph');

/** The full cheat sheet, on `?`. The bar shows the focused pane; this shows
 *  every pane at once, which is the question `?` is actually asking. */
export const shortcutHelpOpen = writable(false);

/** A bump-counter, not a boolean: the search box has to be focusable twice in
 *  a row, and a flag that is already `true` produces no change to react to. */
export const searchFocusRequest = writable(0);

export function requestSearchFocus(): void {
  searchFocusRequest.update((n) => n + 1);
}

/** Focus a pane, opening it if it is closed. */
export function focusPane(pane: PaneId): void {
  switch (pane) {
    case 'sidebar': sidebarPaneOpen.set(true); break;
    case 'view': toolbarCollapsed.set(false); break;
    case 'details': detailsPaneOpen.set(true); break;
    case 'description': describePaneOpen.set(true); break;
    case 'spec': splitViewOpen.set(true); break;
    case 'graph': break;
  }
  focusedPane.set(pane);
}
