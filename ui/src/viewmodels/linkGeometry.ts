/**
 * Edge geometry for the graph canvas: how thick a link draws, and where its
 * arrow head lands.
 *
 * Split out of `GraphView.svelte` because both decisions were wrong in a way
 * only arithmetic reveals, and arithmetic is testable without a canvas.
 *
 * The bug (UI — Module view arrows): the `<marker>` was defined with SVG's
 * default `markerUnits="strokeWidth"`, so its rendered size was
 * `markerWidth × stroke-width`. Link stroke-width came off the merged edge
 * weight and saturated at 6 px, which turned a nominally 6-unit arrow into a
 * 36 px one. `refX` is scaled by the same factor, so the head also sat
 * `15 × stroke-width` = 90 px back from the target centre — most of a 120 px
 * link. At Module level, where collapsing merges every cross-directory
 * relationship into one edge (weights of 200-380 in this repo), that is the
 * common case, not the tail.
 *
 * The two fixes here, and the reasoning behind each:
 *
 *  1. The head is a fixed `ARROW_LEN` px in user space. Edge weight is a fact
 *     about the *line*; the arrow only says "this way", and a directional
 *     glyph that grows has nothing more to say for growing.
 *
 *  2. The head's setback is computed per link from the radius of the node it
 *     points at, not from a constant. Node radius is a user-chosen channel
 *     (`nodeEncoding`) ranging 7-34 px, so no single `refX` can be right: the
 *     old 25 buried the head inside a large circle and left it floating in
 *     space off a small one.
 */

/** Arrow-head length in px, in the canvas' own coordinates.
 *
 *  Paired with `markerUnits: 'userSpaceOnUse'` at the definition site — that
 *  attribute is what makes this a px count rather than a multiple of the
 *  link's stroke-width, and the two only mean anything together. */
export const ARROW_LEN = 10;

/** Breathing room between the tip of the arrow and the rim of the circle it
 *  points at. Without it the tip merges into the node's 2 px stroke and the
 *  edge reads as touching rather than arriving. */
export const ARROW_GAP = 2;

/** Shortest line we will leave behind after trimming. Two nodes can sit
 *  closer together than the head node's radius (mid-settle, or when a hub
 *  drags its neighbours in), and trimming to the rim would otherwise push the
 *  head past the tail and flip the arrow around. */
const MIN_LINE = 4;

/**
 * Stroke width for a link carrying `weight` merged relationships.
 *
 * Kept as a channel — weight is a property of the edge that nothing else on
 * the canvas carries, and the size legend only speaks about nodes — but
 * re-ranged. The old ramp ran to 6 px and the top 6 % of Module-level edges
 * all pinned there, so the channel stopped discriminating exactly where it
 * had the most to say, while dominating the picture. 1.2-3.5 px keeps
 * "heavier than its neighbour" legible without a link out-weighing the module
 * it connects.
 */
export function linkStrokeWidth(weight: number | null | undefined): number {
  const wt = weight ?? 1;
  if (wt <= 1) return 1.2;
  return Math.min(1.2 + Math.log2(wt) * 0.5, 3.5);
}

export interface Point { x: number; y: number }

/**
 * Where the arrow end of a link should stop: on the rim of the node it points
 * at, plus `ARROW_GAP`, so that a marker with `refX = ARROW_LEN` puts its tip
 * exactly there.
 *
 * `headRadius` is the *rendered* radius of the head node, which changes with
 * the size channel — pass `0` when the head is still an unresolved id string
 * (the force simulation replaces those with node objects on its first tick).
 */
export function arrowHeadPoint(
  tailX: number, tailY: number,
  headX: number, headY: number,
  headRadius: number,
): Point {
  const dx = headX - tailX;
  const dy = headY - tailY;
  const len = Math.hypot(dx, dy);
  if (len < MIN_LINE) return { x: headX, y: headY };
  const back = Math.min(headRadius + ARROW_GAP, len - MIN_LINE);
  if (back <= 0) return { x: headX, y: headY };
  return { x: headX - (dx / len) * back, y: headY - (dy / len) * back };
}
