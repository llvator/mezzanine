/**
 * Unit tests for the column budget — how wide each pane ends up, and which
 * one gives way when the window can't hold them all (UI-093, UI-094).
 *
 * This arithmetic used to live in `App.svelte` as reactive statements, where
 * the only thing checking it was a browser probe. The cases worth guarding are
 * the ones a probe is bad at: a width remembered from a monitor being read on
 * a laptop, a pane that must shrink rather than vanish, and the promise that
 * focus-expand never writes to what a pane remembers.
 *
 *   npm run test:panes
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  layoutPanes,
  maxWidthFor,
  DETAILS_MIN_WIDTH,
  SPEC_MIN_WIDTH,
  DESCRIPTION_MIN_WIDTH,
  SIDEBAR_MIN_WIDTH,
  MIN_CANVAS_WIDTH,
  MIN_CANVAS_FOCUS_WIDTH,
  type LayoutInput,
  type PaneWant,
} from '../src/viewmodels/paneLayout.ts';

function pane(want: number, min: number, over: Partial<PaneWant> = {}): PaneWant {
  return { present: true, open: true, want, min, ...over };
}

/** A 1920px window with everything open and no spec pane, unless said
 *  otherwise. Three strips, which is what the app draws without a spec.
 *  Wide enough that all three columns fit — the old comment put that
 *  threshold at ~1620px, and these cases are about what happens either side
 *  of it. */
function input(over: Partial<LayoutInput> = {}): LayoutInput {
  return {
    winWidth: 1920,
    strips: 3,
    sidebar: pane(360, SIDEBAR_MIN_WIDTH),
    spec: pane(400, SPEC_MIN_WIDTH, { present: false, open: false }),
    details: pane(340, DETAILS_MIN_WIDTH),
    description: pane(300, DESCRIPTION_MIN_WIDTH),
    canvasOpen: true,
    focus: null,
    ...over,
  };
}

// ─── the ordinary case ───────────────────────────────────────────────────────

test('a window with room gives every column exactly what it asked for', () => {
  const { widths } = layoutPanes(input());
  assert.equal(widths.sidebar, 360);
  assert.equal(widths.details, 340);
  assert.equal(widths.description, 300);
  assert.equal(widths.canvas, 1920 - 60 - 360 - 340 - 300);
});

// UI-098 moved this line. A column now survives wherever every open column
// can have its floor — the old threshold was where they could all have what
// they *asked* for, which cost you a pane while the room for it existed.
test('a column survives as long as every open column can have its floor', () => {
  const roomy = layoutPanes(input({ winWidth: 1660 }));
  const tight = layoutPanes(input({ winWidth: 1560 }));
  assert.equal(roomy.widths.description, 260, 'trimmed, not dropped');
  assert.equal(tight.widths.description, DESCRIPTION_MIN_WIDTH, 'at its floor, still drawn');

  // Below the floors themselves the older rule stands: the last column gives
  // way rather than every column becoming unreadable.
  const cramped = layoutPanes(input({ winWidth: 1280 }));
  assert.equal(cramped.widths.description, 0);
  assert.equal(cramped.squashed.description, true);
});

test('opening a fourth column no longer silently costs you the last one', () => {
  const allFive = input({ winWidth: 1920, strips: 5, spec: pane(400, SPEC_MIN_WIDTH) });
  const { widths, squashed } = layoutPanes(allFive);
  for (const id of ['sidebar', 'spec', 'details', 'description'] as const) {
    assert.ok(widths[id] > 0, `${id} was dropped though every floor fits`);
    assert.equal(squashed[id], false);
  }
  assert.equal(widths.canvas, MIN_CANVAS_WIDTH);
});

test('the canvas keeps every pixel no column claimed', () => {
  const { widths } = layoutPanes(input({ winWidth: 2560 }));
  const columns = widths.sidebar + widths.spec + widths.details + widths.description;
  assert.equal(columns + widths.canvas + 60, 2560);
});

// ─── no per-pane ceiling any more ────────────────────────────────────────────

test('Details passes the old 560 cap on a monitor with the room for it', () => {
  const wide = input({ winWidth: 2560, details: pane(1400, DETAILS_MIN_WIDTH) });

  // Stopped by Description's floor rather than by a number in this file, and
  // that is the trade UI-098 made: a drag now yields to a pane you have open
  // behind it instead of evicting it. `maxWidthFor` runs the same budget, so
  // the handle stops here too and the cap is never a surprise.
  const { widths } = layoutPanes(wide);
  assert.equal(widths.details, 2560 - 60 - 360 - MIN_CANVAS_WIDTH - DESCRIPTION_MIN_WIDTH);
  assert.equal(maxWidthFor('details', wide), widths.details);
  assert.ok(widths.description >= DESCRIPTION_MIN_WIDTH, 'and Description is still drawn');

  // Close what it was yielding to and the full remembered width is honoured.
  const alone = layoutPanes({ ...wide, description: pane(300, DESCRIPTION_MIN_WIDTH, { open: false }) });
  assert.equal(alone.widths.details, 1400);
});

test('a pane may grow until the canvas would drop below its floor, and no further', () => {
  const greedy = input({
    winWidth: 1920,
    details: pane(99_999, DETAILS_MIN_WIDTH),
    description: pane(300, DESCRIPTION_MIN_WIDTH, { open: false }),
  });
  const { widths } = layoutPanes(greedy);
  assert.equal(widths.canvas, MIN_CANVAS_WIDTH);
  assert.equal(widths.details, 1920 - 60 - 360 - MIN_CANVAS_WIDTH);
});

test('a width remembered from a monitor is clamped for display, not lost', () => {
  const remembered = pane(1400, DETAILS_MIN_WIDTH);
  const { widths } = layoutPanes(input({ winWidth: 1280, details: remembered }));
  assert.ok(widths.details < 1400);
  // The caller's store is the `want` it passed in, and nothing wrote to it.
  assert.equal(remembered.want, 1400);
});

// ─── giving way ──────────────────────────────────────────────────────────────

test('Details outranks Description when only one of them fits', () => {
  const { widths, squashed } = layoutPanes(input({ winWidth: 1280 }));
  assert.ok(widths.details > 0);
  assert.equal(widths.description, 0);
  assert.equal(squashed.description, true);
  assert.equal(squashed.details, false);
});

test('a squashed pane is told apart from a closed one', () => {
  const closed = layoutPanes(input({
    winWidth: 1280,
    description: pane(300, DESCRIPTION_MIN_WIDTH, { open: false }),
  }));
  assert.equal(closed.widths.description, 0);
  assert.equal(closed.squashed.description, false, 'the user closed it, the window did not');
});

test('a pane the project does not have is never squashed', () => {
  const { widths, squashed } = layoutPanes(input({ winWidth: 900 }));
  assert.equal(widths.spec, 0);
  assert.equal(squashed.spec, false);
});

test('Details shrinks to its floor before it gives up', () => {
  const roomy = layoutPanes(input({ winWidth: 1660 }));
  const tight = layoutPanes(input({ winWidth: 1280 }));
  assert.equal(roomy.widths.details, 340, 'what it asked for');
  assert.equal(tight.widths.details, DETAILS_MIN_WIDTH, 'narrower, rather than gone');
  assert.equal(tight.squashed.details, false);
});

test('the canvas dominates at 1280x800, the narrowest window checked (UI-020)', () => {
  const { widths } = layoutPanes(input({ winWidth: 1280 }));
  assert.ok(widths.canvas >= MIN_CANVAS_WIDTH);
  assert.ok(widths.canvas > widths.sidebar + widths.details + widths.description,
    'the canvas stays wider than every side column put together');
});

test('the sidebar can no longer be dragged over the canvas', () => {
  const { widths } = layoutPanes(input({ winWidth: 1280, sidebar: pane(99_999, SIDEBAR_MIN_WIDTH) }));
  assert.ok(widths.canvas >= MIN_CANVAS_WIDTH);
});

// ─── focus-expand (UI-094) ───────────────────────────────────────────────────

test('the focused pane grows and the others fall to their floor', () => {
  const { widths } = layoutPanes(input({ focus: 'details' }));
  assert.equal(widths.sidebar, SIDEBAR_MIN_WIDTH);
  assert.equal(widths.description, DESCRIPTION_MIN_WIDTH);
  assert.ok(widths.details > 340, 'it took the room the others gave up');
});

test('nothing disappears while a pane is expanded', () => {
  for (const focus of ['sidebar', 'details', 'description'] as const) {
    const { widths, squashed } = layoutPanes(input({ winWidth: 1440, focus }));
    assert.equal(squashed.details, false, `details vanished with focus on ${focus}`);
    assert.equal(squashed.description, false, `description vanished with focus on ${focus}`);
    assert.ok(widths.description >= DESCRIPTION_MIN_WIDTH);
    assert.ok(widths.details >= DETAILS_MIN_WIDTH);
  }
});

test('an earlier column expanding still leaves the later ones their floor', () => {
  // The sidebar claims first. Without a reservation it would eat everything
  // and the two right-hand panes would report themselves squashed.
  const { widths } = layoutPanes(input({ focus: 'sidebar' }));
  assert.equal(widths.details, DETAILS_MIN_WIDTH);
  assert.equal(widths.description, DESCRIPTION_MIN_WIDTH);
  assert.ok(widths.sidebar > 360);
});

test('the canvas floor drops while a pane is being read, and not otherwise', () => {
  const reading = layoutPanes(input({ winWidth: 1440, focus: 'details' }));
  assert.equal(reading.widths.canvas, MIN_CANVAS_FOCUS_WIDTH);

  const onCanvas = layoutPanes(input({ winWidth: 1440, focus: 'graph' }));
  assert.ok(onCanvas.widths.canvas >= MIN_CANVAS_WIDTH);
});

test('focusing the canvas floors every pane and hands the slack back', () => {
  const { widths } = layoutPanes(input({ focus: 'graph' }));
  assert.equal(widths.sidebar, SIDEBAR_MIN_WIDTH);
  assert.equal(widths.details, DETAILS_MIN_WIDTH);
  assert.equal(widths.description, DESCRIPTION_MIN_WIDTH);
  assert.ok(widths.canvas > layoutPanes(input()).widths.canvas);
});

test('the view controls focus no column, so they behave like the canvas', () => {
  const view = layoutPanes(input({ focus: 'view' }));
  const graph = layoutPanes(input({ focus: 'graph' }));
  assert.deepEqual(view.widths, graph.widths);
});

test('turning the mode off returns every pane to what it remembered', () => {
  const before = layoutPanes(input()).widths;
  layoutPanes(input({ focus: 'details' }));
  const after = layoutPanes(input()).widths;
  assert.deepEqual(after, before);
});

test('expanding is worth something on a laptop', () => {
  const plain = layoutPanes(input({ winWidth: 1440 })).widths.details;
  const expanded = layoutPanes(input({ winWidth: 1440, focus: 'details' })).widths.details;
  assert.ok(expanded > plain + 200, `expanded to ${expanded} from ${plain}`);
});

// ─── where a drag stops ──────────────────────────────────────────────────────

test('a handle stops exactly where the pane would stop rendering', () => {
  const at = input({ winWidth: 1920 });
  const ceiling = maxWidthFor('details', at);
  const atCeiling = layoutPanes({ ...at, details: pane(ceiling, DETAILS_MIN_WIDTH) });
  const pastIt = layoutPanes({ ...at, details: pane(ceiling + 200, DETAILS_MIN_WIDTH) });
  assert.equal(atCeiling.widths.details, ceiling);
  assert.equal(pastIt.widths.details, ceiling, 'dragging past the ceiling changes nothing');
});

test('a closed pane still reports the width it could be dragged to', () => {
  const closed = input({ details: pane(340, DETAILS_MIN_WIDTH, { open: false }) });
  assert.ok(maxWidthFor('details', closed) >= DETAILS_MIN_WIDTH);
});

// ─── the spec pane ───────────────────────────────────────────────────────────

test('the spec pane claims before the right-hand side, and adds a fourth strip', () => {
  const withSpec = input({
    winWidth: 1600,
    strips: 4,
    spec: pane(400, SPEC_MIN_WIDTH),
  });
  const { widths, squashed } = layoutPanes(withSpec);
  assert.equal(widths.spec, 400);
  assert.ok(widths.canvas >= MIN_CANVAS_WIDTH);
  assert.equal(widths.description, 0, 'the right-hand side gave way, not the spec pane');
  assert.equal(squashed.spec, false);
});

// ─── a window with no canvas (UI-098) ────────────────────────────────────────

test('closing the canvas draws no canvas at all', () => {
  const { widths } = layoutPanes(input({ canvasOpen: false }));
  assert.equal(widths.canvas, 0);
});

// The point of the whole switch: five panes need 1640px in one window, and
// the canvas floor is the largest term in that sum.
test('all four columns fit a window that could never hold them beside a canvas', () => {
  const cramped = { winWidth: 1200, strips: 5, spec: pane(400, SPEC_MIN_WIDTH) };
  const withCanvas = layoutPanes(input({ ...cramped, canvasOpen: true }));
  const without = layoutPanes(input({ ...cramped, canvasOpen: false }));

  assert.ok(withCanvas.widths.description === 0 || withCanvas.widths.details === 0,
    'a 1200px window cannot hold four columns and a canvas');
  for (const id of ['sidebar', 'spec', 'details', 'description'] as const) {
    assert.ok(without.widths[id] > 0, `${id} should be drawn once the canvas is closed`);
  }
});

test('nothing is reported squashed when the canvas is what gave way', () => {
  const { squashed } = layoutPanes(input({
    winWidth: 1200, strips: 5, spec: pane(400, SPEC_MIN_WIDTH), canvasOpen: false,
  }));
  assert.deepEqual(squashed, { sidebar: false, spec: false, details: false, description: false });
});

// Without a canvas there is no natural filler, so four columns at their
// remembered widths would sit beside a few hundred pixels of background.
test('the rightmost drawn column absorbs what the canvas would have', () => {
  const { widths } = layoutPanes(input({ winWidth: 1920, canvasOpen: false }));
  const drawn = widths.sidebar + widths.spec + widths.details + widths.description;
  assert.equal(drawn, 1920 - 60, 'the columns fill the window less its strips');
  assert.equal(widths.description, 300 + (1920 - 60 - 360 - 340 - 300));
  assert.equal(widths.details, 340, 'the columns left of it keep what they asked for');
});

test('the filler is whichever column is rightmost, not Description by name', () => {
  const { widths } = layoutPanes(input({
    winWidth: 1920,
    canvasOpen: false,
    description: pane(300, DESCRIPTION_MIN_WIDTH, { open: false }),
  }));
  assert.equal(widths.description, 0);
  assert.equal(widths.details, 340 + (1920 - 60 - 360 - 340));
});

test('a window with nothing open at all is empty rather than broken', () => {
  const { widths } = layoutPanes(input({
    winWidth: 1920,
    canvasOpen: false,
    sidebar: pane(360, SIDEBAR_MIN_WIDTH, { open: false }),
    details: pane(340, DETAILS_MIN_WIDTH, { open: false }),
    description: pane(300, DESCRIPTION_MIN_WIDTH, { open: false }),
  }));
  assert.deepEqual(widths, { sidebar: 0, spec: 0, details: 0, description: 0, canvas: 0 });
});

// Focus-expand drops the canvas floor to 360; with no canvas there is no
// floor to drop, and the focused column should still not eat its neighbours.
test('focus-expand still leaves the other columns their floors with no canvas', () => {
  const { widths } = layoutPanes(input({
    winWidth: 1400, canvasOpen: false, focus: 'details',
  }));
  assert.equal(widths.sidebar, SIDEBAR_MIN_WIDTH);
  assert.equal(widths.description, DESCRIPTION_MIN_WIDTH);
  assert.equal(widths.details, 1400 - 60 - SIDEBAR_MIN_WIDTH - DESCRIPTION_MIN_WIDTH);
});
