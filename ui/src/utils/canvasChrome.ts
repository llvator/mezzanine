/**
 * Chrome colours the graph canvas draws with.
 *
 * These used to be literals copied out of the `midnight` theme and pasted
 * into both of GraphView's render paths. Under the `light` theme that put
 * white node labels on a near-white canvas at 1.08:1 contrast, and left the
 * link-label pills dark navy. UI-009.
 *
 * Two categories live here, and the distinction is load-bearing:
 *
 *   Theme-derived — the value sits on the canvas background, so it has to
 *   track the theme. Read from the CSS custom properties `applyTheme()`
 *   writes onto `:root`.
 *
 *   Fixed — the value sits on a *semantic* fill (a per-kind node colour, the
 *   amber order badge) that is deliberately theme-independent. Making these
 *   theme-derived would be the opposite bug: dark text on a dark-blue Class
 *   circle in the light theme. They live here as named constants rather than
 *   inline literals so there is one place to find every colour the canvas
 *   draws.
 */

export interface CanvasChrome {
  /** Ring around a node circle, separating it from the canvas. */
  nodeStroke: string;
  /** Entity name, drawn below the node directly on the canvas. */
  nameLabelFill: string;
  /** Pill behind a relationship label. */
  linkLabelBg: string;
  linkLabelBgOpacity: number;
  /** Edge arrowhead. */
  arrowFill: string;
  /** Ring around the amber call-order badge. */
  orderBadgeStroke: string;
  /** Digit inside the amber call-order badge. */
  orderBadgeText: string;
}

/** Near-black on the fixed `#FF9800` badge, for the same reason. */
const ON_BADGE_FILL = '#141414';

function prop(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

/**
 * Snapshot the active theme's chrome colours.
 *
 * Call at render time, not at module load — the theme can change while a
 * graph is on screen, and `applyCanvasChrome` re-reads this to restyle.
 */
export function canvasChrome(): CanvasChrome {
  return {
    // `--text` rather than `--bg-body`: in the dark themes this stays a
    // bright ring (#eeeeee, near the old #fff), and in `light` it becomes a
    // dark ring that is still visible. Using the background colour would
    // make the ring vanish in every theme.
    nodeStroke: prop('--text', '#eeeeee'),
    nameLabelFill: prop('--text', '#eeeeee'),
    // The two-letter kind code used to live here as a flat white, on the
    // assumption that every node fill was a saturated mid-tone. UI-014 makes
    // fill metric-driven, so the ink is now chosen per node against its own
    // fill — see `labelInk` in viewmodels/nodeEncoding.ts.
    linkLabelBg: prop('--bg-surface', '#16213e'),
    linkLabelBgOpacity: 0.85,
    arrowFill: prop('--text-dim', '#888888'),
    orderBadgeStroke: prop('--bg-body', '#1a1a2e'),
    orderBadgeText: ON_BADGE_FILL,
  };
}
