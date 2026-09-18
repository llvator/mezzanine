/**
 * Which kinds of change a comparison contains, and which of them the reader
 * has asked to see (UI-149).
 *
 * A view filter, and deliberately only that. The Changes tab is an instrument
 * — it exists so the canvas can be checked against git — and an instrument
 * that quietly changed what it measures would not be one. So the totals in
 * the header keep describing the whole comparison, the residue keeps listing
 * every path the two sides disagree about, and nothing here reaches the seed,
 * the ladder or the scope. The only thing a facet hides is a row.
 *
 * Pure and store-free for the same reason `changedFiles.ts` is: the rule about
 * what a selection means has to be checkable without a browser.
 */

import {
  statusChange, statusLetter, statusPhrase,
  type ChangedFile, type ChangedFileRow,
} from './changedFiles.ts';

/**
 * One kind of change, with the number of files in it.
 *
 * Keyed by the letter a *reader* sees rather than the letter git sends, which
 * is the whole point of offering the filter: `U` and `A` are one letter to git
 * and two different questions to a reviewer — "what have I not committed yet"
 * against "what is new in this change" — and `statusLetter` is where that
 * distinction already lives.
 */
export interface ChangeFacet {
  /** `U` for untracked, git's own letter otherwise. */
  letter: string;
  /** The same thing in words: `untracked`, `modified`, `deleted`, … */
  phrase: string;
  /** Which of the diff colours the chip reads as, matching its rows. */
  change: 'added' | 'removed' | 'modified';
  /** Files of this kind in the comparison. Zero only for a kind the reader
   *  has selected and the comparison no longer contains — see `changeFacets`. */
  count: number;
  selected: boolean;
}

/**
 * Reading order for the chips, which is not frequency order.
 *
 * A row of chips that reshuffled itself every time the working tree moved
 * would cost more to read than it saves — under `→ working` the counts change
 * on every save, and the reader is aiming a click at a position. So the order
 * is fixed and roughly by how often a reviewer wants each one: the two that
 * make up most of an ordinary change, then the two that are the reason to
 * look, then the three that are rare.
 *
 * A letter git sends that is not here still gets a chip; it sorts to the end
 * alphabetically, the same way `statusLabel` lets an unknown letter through as
 * itself rather than swallowing it.
 */
const LETTER_ORDER = ['M', 'A', 'U', 'D', 'R', 'C', 'T'];

/** The two fields the vocabulary functions need, recovered from a letter. */
function fromLetter(letter: string): Pick<ChangedFile, 'status' | 'untracked'> {
  return letter === 'U' ? { status: 'A', untracked: true } : { status: letter, untracked: false };
}

function rank(letter: string): number {
  const i = LETTER_ORDER.indexOf(letter);
  return i < 0 ? LETTER_ORDER.length : i;
}

/**
 * The chips to draw for one reading, given what is selected.
 *
 * A selected letter the comparison no longer contains still gets a chip, at
 * zero. It is the one case where showing an empty count is right: without it
 * the control that would turn the filter off disappears at exactly the moment
 * the filter empties the list — the reader filters to deletions, saves the
 * file that had the only deletion, and is left looking at nothing with nothing
 * to click. The chip stays until they let go of it.
 */
export function changeFacets(
  rows: ChangedFileRow[],
  selected: ReadonlySet<string> = new Set(),
): ChangeFacet[] {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const letter = statusLetter(row.file);
    counts.set(letter, (counts.get(letter) ?? 0) + 1);
  }
  for (const letter of selected) if (!counts.has(letter)) counts.set(letter, 0);

  return [...counts]
    .map(([letter, count]) => ({
      letter,
      phrase: statusPhrase(fromLetter(letter)),
      change: statusChange(fromLetter(letter).status),
      count,
      selected: selected.has(letter),
    }))
    .sort((a, b) => rank(a.letter) - rank(b.letter) || a.letter.localeCompare(b.letter));
}

/**
 * The rows a selection leaves on screen.
 *
 * An empty selection is *no filter*, not "show nothing". Both readings are
 * defensible in the abstract and only one of them is defensible as a default:
 * the tab opens with nothing selected, and a tab that opened empty would look
 * like a comparison that found nothing.
 */
export function filterRows(
  rows: ChangedFileRow[],
  selected: ReadonlySet<string>,
): ChangedFileRow[] {
  if (selected.size === 0) return rows;
  return rows.filter((row) => selected.has(statusLetter(row.file)));
}

/** Add or drop one letter, as a new set — the shape a svelte store update
 *  takes, and the reason this is a function rather than a mutation at the call
 *  site: a set mutated in place is a store that does not notify. */
export function toggleFacet(selected: ReadonlySet<string>, letter: string): Set<string> {
  const next = new Set(selected);
  if (!next.delete(letter)) next.add(letter);
  return next;
}
