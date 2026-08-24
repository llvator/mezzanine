/**
 * Which folder the shape view is drawing, and the picture the engine sent
 * back for it (UI-108).
 *
 * The picture is fetched rather than derived from `graphData`, and that is
 * the whole design. Levels, loops, doors and every edge's reading were
 * decided by the pass that produced the folder's verdict; re-deriving them
 * here would be a second answer, free to draw a picture the number in the
 * Quality panel denies. The same argument `ShapeBlocker` is centralised on,
 * one layer out.
 *
 * `GET /api/shape` returns the verdict *with* the picture for the same
 * reason they are stored together here: two fetches could straddle a
 * re-analysis and pair one run's drawing with another's number.
 */

import { derived, get, writable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import type { FolderShape, ShapeResponse } from '../types/graph';
import { shapePicture, viewMode } from './graph';

/** The folder being drawn, root-relative. `null` when the view has never
 *  been opened. `''` is the repo root, which is a real folder with a real
 *  shape and not an absent value. */
export const shapeFolder = writable<string | null>(null);

/** The verdict that came back with the picture. Re-exported alongside it so
 *  a panel needs one import, while the picture itself lives in `graph.ts`
 *  because `graphData` derives from it — see the note there. */
export { shapePicture };
export const shapeVerdict = writable<FolderShape | null>(null);

/** In flight. The canvas keeps drawing the previous picture meanwhile —
 *  blanking on every fetch would make walking down a folder tree flicker. */
export const shapeLoading = writable(false);

/**
 * Why there is no picture, in words a reader can act on. `null` when there
 * is one, or when none was ever asked for.
 *
 * Kept apart from `shapePicture` being null: "not requested" and "requested
 * and refused" are different states, and the second is the one that has
 * something to say.
 */
export const shapeError = writable<string | null>(null);

/** Whether the canvas should be drawing a shape picture right now — the
 *  mode is on AND there is something to draw. The view falls back to the
 *  force layout rather than to a blank canvas while a fetch is in flight or
 *  after one failed. */
export const shapeActive = derived(
  [viewMode, shapePicture],
  ([$mode, $picture]) => $mode === 'shape' && $picture !== null,
);

/**
 * Draw `folder`. Switches the canvas into shape mode once the picture is in
 * hand — never before, so a failed fetch leaves the reader looking at the
 * graph they had rather than at nothing.
 */
export async function openShape(folder: string): Promise<void> {
  shapeFolder.set(folder);
  shapeLoading.set(true);
  try {
    const resp = await fetch(apiUrl(`/api/shape?path=${encodeURIComponent(folder)}`));
    if (resp.status === 404) {
      // The engine analysed no such folder. Its own message names why, and
      // it is better than anything this layer could reconstruct.
      shapeError.set(
        (await resp.text())
          || `Nothing analysed at ${folder || '(root)'}, so it draws no graph.`,
      );
      shapePicture.set(null);
      shapeVerdict.set(null);
      return;
    }
    if (!resp.ok) {
      shapeError.set(await resp.text());
      return;
    }
    const body = (await resp.json()) as ShapeResponse;
    shapePicture.set(body.picture);
    shapeVerdict.set(body.shape ?? null);
    shapeError.set(null);
    viewMode.set('shape');
  } catch (e) {
    shapeError.set(e instanceof Error ? e.message : String(e));
  } finally {
    shapeLoading.set(false);
  }
}

/** Re-fetch the folder already open, after the watch reports a re-analysis.
 *  Silent when the view was never opened — there is nothing to refresh. */
export async function refreshShape(): Promise<void> {
  const folder = get(shapeFolder);
  if (folder === null) return;
  await openShape(folder);
}

/** Leave the view, keeping the folder so reopening it costs no fetch. */
export function closeShape(): void {
  if (get(viewMode) === 'shape') viewMode.set('graph');
}
