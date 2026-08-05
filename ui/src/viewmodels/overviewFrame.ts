/**
 * The geometry behind the overview panel — the unzoomed graph in a corner of
 * the canvas, with a box around the part the viewport is showing.
 *
 * All of it is a mapping problem between three coordinate spaces, and none of
 * it needs the DOM, which is why it lives here rather than in the component:
 *
 *   world     — where the force simulation puts nodes. Unbounded, and its
 *               origin means nothing; the graph settles wherever it settles.
 *   screen    — canvas pixels. `world × d3.zoomTransform` gets you here.
 *   overview  — pixels inside the little panel.
 *
 * The panel draws world→overview and the box draws screen→world→overview, so
 * the two agree by construction rather than by two similar-looking formulas
 * kept in step by hand.
 */

/** An axis-aligned rectangle, in whichever space the caller is working in. */
export interface Rect {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/** The `k`/`x`/`y` triple of a d3 zoom transform: `screen = world × k + (x, y)`. */
export interface ViewTransform {
  k: number;
  x: number;
  y: number;
}

/** One circle in the overview. Position and radius are world-space — the
 *  panel scales them itself, so a frame captured at one panel size is still
 *  correct at another. */
export interface OverviewDot {
  x: number;
  y: number;
  r: number;
  fill: string;
  opacity: number;
}

/** world → overview: `overview = world × scale + (offsetX, offsetY)`. */
export interface OverviewFit {
  scale: number;
  offsetX: number;
  offsetY: number;
}

/**
 * A rectangle can't be inverted or empty, and a zero-span one divides by zero
 * two functions down. One node on the canvas is a real case (a scope narrowed
 * to a single file), and so is a viewport at a scale so high it spans less
 * than a pixel of world.
 */
const MIN_SPAN = 1;

/** The world rectangle a `w × h` viewport is currently showing.
 *
 *  The inverse of the transform, which is the whole trick: d3 tells you where
 *  the world goes on screen, and the box needs the opposite reading. */
export function viewportWorldRect(t: ViewTransform, w: number, h: number): Rect {
  const k = t.k || 1;
  return {
    minX: (0 - t.x) / k,
    minY: (0 - t.y) / k,
    maxX: (w - t.x) / k,
    maxY: (h - t.y) / k,
  };
}

/** World extents of the drawn nodes, circles included, or null when nothing
 *  is drawn. Labels are deliberately not counted the way `fitView` counts
 *  them — the overview draws no labels, so padding for them would leave a
 *  margin of empty world around a graph that is already small on screen. */
export function dotsExtent(dots: OverviewDot[]): Rect | null {
  if (dots.length === 0) return null;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const d of dots) {
    if (d.x - d.r < minX) minX = d.x - d.r;
    if (d.y - d.r < minY) minY = d.y - d.r;
    if (d.x + d.r > maxX) maxX = d.x + d.r;
    if (d.y + d.r > maxY) maxY = d.y + d.r;
  }
  return { minX, minY, maxX, maxY };
}

export function unionRect(a: Rect | null, b: Rect | null): Rect | null {
  if (!a) return b;
  if (!b) return a;
  return {
    minX: Math.min(a.minX, b.minX),
    minY: Math.min(a.minY, b.minY),
    maxX: Math.max(a.maxX, b.maxX),
    maxY: Math.max(a.maxY, b.maxY),
  };
}

/**
 * What the panel has to cover: the graph *and* the viewport, never the graph
 * alone.
 *
 * Panning off the side of the graph is an ordinary thing to do — it is half
 * of getting lost, which is the state this panel exists to fix. Fitted to the
 * nodes only, the box would slide out of the panel exactly then, and the one
 * moment the reader most needs to see where they are is the one moment the
 * answer is off the edge. Including the viewport means the panel zooms out to
 * hold it instead, so the box is always somewhere inside and the graph
 * visibly shrinks into a corner — which reads, correctly, as "you have gone
 * a long way past it".
 */
export function overviewWorld(dots: OverviewDot[], view: Rect | null): Rect | null {
  return unionRect(dotsExtent(dots), view);
}

/**
 * Fit a world rectangle into a `boxW × boxH` panel, uniformly and centred.
 *
 * One scale for both axes, because the panel is a picture of the graph and
 * two scales would stretch it into a different shape from the canvas it is
 * summarising — the reader is meant to recognise one in the other.
 */
export function fitWorld(world: Rect, boxW: number, boxH: number, pad = 4): OverviewFit {
  const spanX = Math.max(MIN_SPAN, world.maxX - world.minX);
  const spanY = Math.max(MIN_SPAN, world.maxY - world.minY);
  const innerW = Math.max(1, boxW - pad * 2);
  const innerH = Math.max(1, boxH - pad * 2);
  const scale = Math.min(innerW / spanX, innerH / spanY);
  return {
    scale,
    offsetX: pad + (innerW - spanX * scale) / 2 - world.minX * scale,
    offsetY: pad + (innerH - spanY * scale) / 2 - world.minY * scale,
  };
}

/** A world rectangle in panel pixels, ready for an SVG `rect`. */
export function projectRect(r: Rect, fit: OverviewFit): { x: number; y: number; width: number; height: number } {
  return {
    x: r.minX * fit.scale + fit.offsetX,
    y: r.minY * fit.scale + fit.offsetY,
    width: Math.max(1, (r.maxX - r.minX) * fit.scale),
    height: Math.max(1, (r.maxY - r.minY) * fit.scale),
  };
}

/** Panel pixels back to world — what a click or a drag in the panel means. */
export function overviewToWorld(px: number, py: number, fit: OverviewFit): { x: number; y: number } {
  return {
    x: (px - fit.offsetX) / fit.scale,
    y: (py - fit.offsetY) / fit.scale,
  };
}

/**
 * The transform that centres `(cx, cy)` of world in a `w × h` viewport,
 * keeping the current scale.
 *
 * Scale is kept rather than recomputed because the panel moves you, it does
 * not re-frame you: a click that also changed the zoom would answer a
 * question the reader did not ask, and would undo a magnification they had
 * just set up by hand.
 */
export function centreTransform(cx: number, cy: number, k: number, w: number, h: number): ViewTransform {
  return { k, x: w / 2 - cx * k, y: h / 2 - cy * k };
}

/**
 * The smallest dot radius worth drawing, in panel pixels.
 *
 * A whole-repo graph fitted into ~170px puts most nodes well under a pixel,
 * and a sub-pixel circle renders as a faint smudge whose opacity depends on
 * where it happens to land — so the cloud shows density-of-antialiasing
 * rather than density-of-code. Clamping up costs the size channel its
 * meaning at the bottom of the range, which the panel can afford: it is
 * answering "where am I", and the canvas beside it still answers "how big".
 */
export const MIN_DOT_R = 1.1;

export function dotRadius(worldR: number, fit: OverviewFit): number {
  return Math.max(MIN_DOT_R, worldR * fit.scale);
}
