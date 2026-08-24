/**
 * Picking rows out of a result list.
 *
 * Two decisions that both result lists in the filter panel need, kept here
 * as plain functions over plain values: the sidebar lists are the only
 * callers, but the behaviour is the part worth testing and the components
 * that host it cannot be imported by the test runner (they reach for
 * stores). Same reason `entityScore.ts` and `blockReason.ts` sit here.
 */

/**
 * The rows a checkbox click applies to.
 *
 * Shift extends from the last-clicked row (the anchor) to this one, the
 * convention every file manager uses. With no usable anchor — none yet, or
 * one addressing a list that has since changed length — shift behaves as a
 * plain click rather than guessing at a range.
 *
 * The range is over the rows as passed, which are the rows on screen, so it
 * can never reach something the reader cannot see.
 */
export function rowsForClick<T>(rows: T[], anchor: number, index: number, shift: boolean): T[] {
  if (index < 0 || index >= rows.length) return [];
  if (!shift || anchor < 0 || anchor >= rows.length) return [rows[index]];
  return rows.slice(Math.min(anchor, index), Math.max(anchor, index) + 1);
}

/**
 * Which matches an overlay actually marks, given what the reader picked.
 *
 * No picks means the reader has expressed no preference, so every match is
 * marked — that is the state the list starts in and the behaviour it had
 * before picking existed.
 *
 * Once there are picks, the marked set is the picks that are still matches.
 * A pick can stop being a match without being unticked: the "search in
 * view" list is scoped to what the canvas draws, and the canvas moves. That
 * intersection can be empty, and then nothing is marked — falling back to
 * "all of them" there would light up the whole result set at the moment the
 * reader's own choice went out of view, which reads as the tick having been
 * ignored.
 */
export function pickedHighlight(matchIds: string[], picked: ReadonlySet<string>): Set<string> {
  if (picked.size === 0) return new Set(matchIds);
  return new Set(matchIds.filter((id) => picked.has(id)));
}
