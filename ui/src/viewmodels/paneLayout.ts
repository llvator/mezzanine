/**
 * How wide each column ends up, given a window and what the reader asked for
 * (UI-093).
 *
 * This used to be seven reactive statements in `App.svelte`, which meant the
 * one piece of arithmetic that decides whether a pane is on screen at all had
 * no test but a browser probe. It is pure and store-free now for the same
 * reason `viewHistory.ts` is: the rules are worth checking without dragging a
 * graph fetch in behind them (`npm run test:panes`).
 *
 * `stores/panes.ts` owns *persistence* — which columns are open, and the width
 * each one remembers. This module owns the *numbers*: the floors, and the
 * budget that turns four remembered widths into four widths that fit.
 *
 * **There is no per-pane maximum.** There used to be (`DETAILS_MAX_WIDTH`,
 * `SPEC_MAX_WIDTH`), and they were arbitrary — a 2560px monitor still refused
 * to draw Details past 560. The only ceiling now is the canvas floor: a pane
 * may grow until the graph would stop being the thing you are looking at.
 * That is the one limit with a reason behind it, so it is the one that stayed.
 *
 * **And the canvas can be closed** (UI-098), which is the one case where even
 * that limit does not apply. It exists because a mirrored second window is a
 * real place to put panes and a pointless place to put a 640px graph; with the
 * floor gone the four columns need about 1000px between them rather than the
 * 1640 no laptop was going to give them.
 */

import type { PaneId } from './keymap.ts';

// ─────────────────────────────────────────────────────────────────────────────
// Floors
// ─────────────────────────────────────────────────────────────────────────────

/** Narrow, deliberately: it is what the pane shrinks to at 1280×800 rather
 *  than vanishing, and a cramped Details column beats no Details column when
 *  promoting it out of the sidebar was the whole point. */
export const DETAILS_MIN_WIDTH = 220;

/** Wider floor than Details: the pane draws a graph rather than reading prose,
 *  and below ~260px the tier labels collide with the nodes. */
export const SPEC_MIN_WIDTH = 260;

/** Description reads as one chain of prose. Below ~240 the indented child
 *  lists wrap every second word. */
export const DESCRIPTION_MIN_WIDTH = 240;

/** The sidebar holds a tree, a table and a settings form, all of which have
 *  rows that stop being readable before the pane stops being visible. */
export const SIDEBAR_MIN_WIDTH = 200;

/**
 * Below this the canvas stops being the thing you are looking at, so the
 * side columns give way rather than squeezing it further.
 *
 * Set so the canvas stays wider than every side column put together at
 * 1280×800, the narrowest window the layout is checked at (UI-020): that
 * window less the sidebar's 360 and the three 20px toggle strips leaves
 * Details its 220px minimum, and 640 > 360 + 220.
 */
export const MIN_CANVAS_WIDTH = 640;

/**
 * The floor while a pane is being read (UI-094), which is a different
 * question from the floor while the canvas is being read.
 *
 * Focus-expand on a 1440px laptop is worth nothing against the 640 floor —
 * there is no slack to hand over. Dropping the floor to 360 for the duration
 * is what makes the mode buy anything on a screen that isn't enormous, and it
 * is safe because the reader has said, by focusing a pane, that the canvas is
 * not what they are looking at right now. Focus the graph and the full floor
 * comes back, because then it is.
 */
export const MIN_CANVAS_FOCUS_WIDTH = 360;

/** Each collapse strip is 20px of chrome that no pane can claim. */
export const TOGGLE_STRIP_WIDTH = 20;

// ─────────────────────────────────────────────────────────────────────────────
// The budget
// ─────────────────────────────────────────────────────────────────────────────

/** One column's ask. `present: false` is a pane the project does not have —
 *  the spec pane without any `.elv` files — which is different from a pane
 *  that is closed. */
export interface PaneWant {
  present: boolean;
  open: boolean;
  /** The width it remembers. Never written back to; the result is derived. */
  want: number;
  min: number;
}

export type ColumnId = 'sidebar' | 'spec' | 'details' | 'description';

export interface LayoutInput {
  winWidth: number;
  /** How many collapse strips are drawn. Five when the spec pane exists. */
  strips: number;
  sidebar: PaneWant;
  spec: PaneWant;
  details: PaneWant;
  description: PaneWant;
  /**
   * Whether this window draws the canvas (UI-098).
   *
   * Closed, it reserves nothing — and the floor it stops reserving is the
   * largest number in this module, which is why closing it is what lets four
   * columns fit a window that could never hold five. The leftover goes to the
   * rightmost open column rather than being left as background: without a
   * canvas there is no natural filler, and four columns at their remembered
   * widths against a 1920px window would otherwise sit beside 500px of nothing.
   */
  canvasOpen: boolean;
  /**
   * The pane the keyboard is in, when focus-expand is on; `null` when it is
   * off. Not the same as `focusedPane`: focus is always somewhere, but it only
   * changes the arithmetic while the mode is enabled.
   */
  focus: PaneId | null;
}

export interface LayoutResult {
  widths: Record<ColumnId | 'canvas', number>;
  /** Wanted, but the window had no room. The toggle says so, so a button that
   *  does nothing visible still explains itself. */
  squashed: Record<ColumnId, boolean>;
}

/**
 * What one column takes.
 *
 * `reserveAfter` is what must survive for the columns that have not claimed
 * yet — the sum of the floors behind this one, or zero when there is not
 * enough room to promise them anything. Reserving is what turns "claim
 * greedily" into "claim what is yours", and it is why opening a fourth column
 * on a wide monitor no longer silently costs you the last one.
 *
 * Returns 0 for a column the window cannot fit, which is the same value the
 * caller draws for a closed one — the difference is recorded in `squashed`.
 */
function claim(pane: PaneWant, room: number, reserveAfter: number): number {
  if (!pane.present || !pane.open) return 0;
  const ceiling = room - reserveAfter;
  if (ceiling < pane.min) return 0;
  return Math.max(pane.min, Math.min(pane.want, ceiling));
}

/** The order columns claim in, left to right except that the right-hand pair
 *  is ranked by how much of the reading it carries. */
const ORDER: readonly ColumnId[] = ['sidebar', 'spec', 'details', 'description'];

/** Which pane id focuses which column. `graph` and `view` focus no column, so
 *  focusing either floors all four and hands the slack to the canvas. */
const COLUMN_OF: Partial<Record<PaneId, ColumnId>> = {
  sidebar: 'sidebar', spec: 'spec', details: 'details', description: 'description',
};

export function layoutPanes(input: LayoutInput): LayoutResult {
  const expanding = input.focus !== null;
  const focusColumn = input.focus ? COLUMN_OF[input.focus] ?? null : null;
  const floor = !input.canvasOpen
    ? 0
    : expanding && input.focus !== 'graph' ? MIN_CANVAS_FOCUS_WIDTH : MIN_CANVAS_WIDTH;

  const widths = { sidebar: 0, spec: 0, details: 0, description: 0, canvas: 0 };
  const squashed = { sidebar: false, spec: false, details: false, description: false };

  let room = input.winWidth - input.strips * TOGGLE_STRIP_WIDTH - floor;

  /**
   * Can every open column have at least its floor?
   *
   * When it can, each one reserves the floors behind it and nothing drops out
   * — which is the whole of "let me open all five at once" (UI-098). Five
   * panes on a 1920 monitor need 920px of floors against 1180px of room, and
   * used to lose Description anyway, because the sidebar and the spec pane
   * claimed their full remembered width before Description was asked.
   *
   * When it cannot, reserving would starve the columns that claim *first*, so
   * the older rule stands: claim greedily in priority order and let the last
   * ones give way. Details outranks Description there because Details is what
   * you keep an eye on while working and Description is read in bursts.
   *
   * Focus-expand always reserves: its whole point is growing a pane into the
   * slack rather than over its neighbours.
   */
  const floorsFit = ORDER.reduce((sum, id) => sum + openFloor(input[id]), 0) <= room;
  const reserving = expanding || floorsFit;

  for (let i = 0; i < ORDER.length; i++) {
    const id = ORDER[i];
    const asked = input[id];
    // Focus-expand replaces what each column *wants*: everything but the
    // focused one falls to its floor, and the focused one takes whatever is
    // left after the ones behind it are guaranteed theirs.
    const pane: PaneWant = !expanding
      ? asked
      : { ...asked, want: id === focusColumn ? Number.MAX_SAFE_INTEGER : asked.min };
    const reserveAfter = reserving
      ? ORDER.slice(i + 1).reduce((sum, later) => sum + openFloor(input[later]), 0)
      : 0;

    const width = claim(pane, room, reserveAfter);
    widths[id] = width;
    squashed[id] = asked.present && asked.open && width === 0;
    room -= width;
  }

  const slack = Math.max(0, room);
  if (input.canvasOpen) {
    widths.canvas = floor + slack;
  } else {
    // No canvas to absorb it, so the rightmost drawn column does. Rightmost
    // rather than the focused one: focus moves as you read and a layout that
    // rearranged itself every time you pressed a digit would be unusable —
    // that is what `z` is for, and it is opt-in.
    const filler = [...ORDER].reverse().find((id) => widths[id] > 0);
    if (filler) widths[filler] += slack;
  }
  return { widths, squashed };
}

/** A closed or absent column reserves nothing. */
function openFloor(pane: PaneWant): number {
  return pane.present && pane.open ? pane.min : 0;
}

/**
 * The ceiling a resize handle stops at: how wide this column could be if it
 * took everything the layout is willing to give it.
 *
 * Dragging past the point where the pane would stop rendering is the failure
 * this prevents — the handle would keep moving and the column would vanish.
 * Computed by asking the same budget for the width it would grant an
 * infinitely greedy column, so the handle and the layout can never disagree.
 */
export function maxWidthFor(id: ColumnId, input: LayoutInput): number {
  const greedy: LayoutInput = { ...input, [id]: { ...input[id], open: true, want: Number.MAX_SAFE_INTEGER } };
  return layoutPanes(greedy).widths[id];
}
