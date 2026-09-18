/**
 * The Changes pane's second channel onto the canvas: a *light*, not a seed.
 *
 * Clicking a row has always done two things at once — open the file's diff in
 * the Details pane, and make its node the graph's selection. The second is a
 * re-root: the tree grows from the selection and the canvas is narrowed to
 * that node's BFS reach, so a reader who clicked a row to *read the diff* paid
 * for it by having the rest of the change deleted from the picture. That is
 * the wrong answer to "which of these files am I looking at", which is a
 * question about where the file sits in what is already drawn.
 *
 * So the row's path is spent on two channels, exactly as `specHighlight` spends
 * a spec entity's `cr:` claims: `filterOnChangeClick` decides whether the click
 * still seeds the canvas, and this one rings the circles holding the file and
 * hides nothing. Hovering a row lights it for as long as the pointer is there;
 * the row you opened stays lit, which is what makes the answer survive the
 * gesture that asked for it.
 *
 * The predicate runs the *other* way round from the spec channel's, and that
 * is the whole difference between the two. There a declared path is a folder
 * and the question is which files it covers; here the path is one file and the
 * question is which drawn circle is holding it — the entity, the File rollup,
 * or the Folder rollup above it, depending on the grain the canvas is at.
 * `pathClaims(node.file_path, file)` answers all three with one comparison,
 * for the reason `f.visual_scopes.marked_set` gives for keying marks by path:
 * `file_path` means "the narrowest scope this circle is evidence of" at every
 * level, and it is what survives the fresh ids `collapseGraph` mints.
 *
 * Pure and store-free, so the set arithmetic is demonstrable without a
 * browser. See `scripts/change-highlight.test.ts`.
 */

import { pathClaims } from '../utils/refPaths.ts';
import { normalizeScopePath } from './diffRollup.ts';

/** The fields [`holdersOf`] reads. Deliberately not `D3Node`: the answer turns
 *  on two of its forty-odd fields, and saying so is what lets a test build a
 *  subject in one line. */
export interface Holder {
  id: string;
  file_path: string;
}

/**
 * Which changed files are lighting the canvas right now.
 *
 * The union of the row that is open and the row under the pointer, and the
 * union is the point: sweeping the list while a row is open compares the two,
 * which is the gesture "does this file live anywhere near the one I opened" is
 * made of. Letting the hover replace the open row would collapse that back to
 * one answer at a time, and the answer the reader asked for first is the one
 * that would be lost.
 *
 * Hovering the open row contributes nothing new — a `Set` deduplicates — so
 * passing over it does not appear to change what is lit.
 */
export function litPaths(opened: string | null, hovered: string | null): string[] {
  const paths = new Set<string>();
  if (opened) paths.add(opened);
  if (hovered) paths.add(hovered);
  return [...paths];
}

/**
 * The drawn circles holding those files, as ids.
 *
 * Empty in and empty out. A channel that hides nothing has no way to say "this
 * file is not on the canvas" by emptying anything, so nothing is lit and the
 * row's own agreement column — `not drawn`, `not analysed` — is what says why.
 *
 * Both sides go through `normalizeScopePath` for the reason every diff lookup
 * does: a path that came back from a base worktree carries a temp prefix, and
 * the graph's paths are repo-relative, so the two have to be spelled alike
 * before anything can be compared. A node with no path at all — a ghost, the
 * root rollup — is holding nothing and is never lit; ringing the root circle
 * would be true of every file and useful for none.
 */
export function holdersOf(
  paths: readonly string[],
  nodes: readonly Holder[],
): Set<string> {
  const ids = new Set<string>();
  const wanted = paths.map(normalizeScopePath).filter((p) => p.length > 0);
  if (wanted.length === 0) return ids;
  for (const node of nodes) {
    const scope = normalizeScopePath(node.file_path ?? '');
    if (!scope) continue;
    if (wanted.some((file) => pathClaims(scope, file))) ids.add(node.id);
  }
  return ids;
}
