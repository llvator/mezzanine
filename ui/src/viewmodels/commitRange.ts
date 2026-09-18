/**
 * What a picked pair of commits means, and how the list draws it (UI-151).
 *
 * The picker used to hand `From` to the server as-is, and the server checks
 * that ref out as the *baseline* tree. So the commit a reader clicked was the
 * one commit their comparison could not see: to review four commits they had
 * to find the fifth, whose subject has nothing to do with the work they were
 * looking at. Every reader hit this once and then compensated by hand.
 *
 * Here `From` names the oldest commit that is *inside* the comparison, and
 * [`baseRefFor`] translates that into the tree the diff starts from — the
 * commit's first parent. The wire is untouched: `from_ref` still means "the
 * baseline tree" to the server, the CLI and the MCP tools, and git's own
 * `A..B` is still the only model in the engine. The translation lives in the
 * one place a person is looking at a list of commits and can be shown what it
 * did, which is what [`railRows`] draws.
 *
 * Pure and store-free, its only import being a type, so `node --test` reaches
 * it without svelte:
 *
 *   npm run test:commitrange
 */

import type { Commit } from '../stores/diff';

/**
 * Find the commit a ref names, in whatever spelling it arrived in.
 *
 * Same prefix-tolerant rule as `refLabel`'s own lookup and for the same
 * reason: a ref reaches here as a full hash from a click, a short hash from
 * `diff.json`, or whatever a reader typed. A prefix comparison covers all
 * three and cannot collide inside one repository's list at these lengths.
 */
export function findCommit(ref: string, commits: Commit[]): Commit | undefined {
  if (!ref) return undefined;
  return commits.find(
    (c) => c.hash === ref || c.short_hash === ref
      || c.hash.startsWith(ref) || ref.startsWith(c.hash),
  );
}

/** Why a `From` cannot be translated into a baseline tree. */
export type BaseProblem = 'root';

export interface BaseRef {
  /** What to send as `from_ref`. Null when the pick has no baseline. */
  ref: string | null;
  /** Set when there is none, so the picker can say which case this is. */
  problem?: BaseProblem;
}

/**
 * The tree a comparison must start from so that `fromRef` is inside it.
 *
 * Three cases, and the third is why this returns a record rather than a
 * string. A commit in the list carries its own first parent, so the answer is
 * a hash and no round trip. A ref the list does not hold — a branch name, a
 * tag, `HEAD~12`, a commit older than the window — gets `~1` appended and git
 * resolves it; the meaning is the same one the label promises, and appending
 * is the only thing that can be done without asking the server. And the root
 * commit has no earlier tree in the repository at all, which is not an error
 * to report after a minute of analysis but a click to decline.
 */
export function baseRefFor(fromRef: string, commits: Commit[]): BaseRef {
  const commit = findCommit(fromRef, commits);
  if (!commit) return { ref: `${fromRef}~1` };
  if (commit.parent_hash) return { ref: commit.parent_hash };
  return isRoot(commit, commits) ? { ref: null, problem: 'root' } : { ref: `${fromRef}~1` };
}

/**
 * Whether a commit having no parent means it is the first one.
 *
 * It only means that if the listing carries parents at all. A UI talking to a
 * server built before `%P` sees every commit as parentless, and reading that
 * literally would declare the whole history unusable as a `From` — so the
 * absence is treated as "not said" unless something in the same listing said
 * it, and the pick falls back to letting git resolve `~1`.
 */
export function isRoot(commit: Commit, commits: Commit[]): boolean {
  return !commit.parent_hash && commits.some((c) => !!c.parent_hash);
}

/** One row of the commit list, and where it sits relative to the range. */
export interface RailRow {
  hash: string;
  /** Inside the comparison — the ends included. */
  included: boolean;
  isFrom: boolean;
  isTo: boolean;
  /** The commit the baseline tree is taken from: the one below `From`. */
  isBase: boolean;
  /** Draw the "everything below is the base" divider above this row. */
  boundaryAbove: boolean;
  /** Light the rail segment running up out of this node, and down out of it.
   *  Two flags rather than one, so the accent stops at the end nodes instead
   *  of running off the top and bottom of the span. */
  litAbove: boolean;
  litBelow: boolean;
}

/**
 * Where a ref sits in the listing, or -1.
 *
 * `HEAD` is resolved rather than searched for. It is the picker's default
 * `To`, it is not a hash, and the list it is being looked up in may be
 * another branch's — in which case the checkout's HEAD is genuinely not in it
 * and -1 is the right answer.
 */
function indexOf(hashes: string[], ref: string, headHash: string | null): number {
  if (!ref) return -1;
  if (ref === 'HEAD') return headHash ? hashes.indexOf(headHash) : -1;
  return hashes.findIndex((h) => h === ref || h.startsWith(ref) || ref.startsWith(h));
}

/**
 * Draw the range onto the listing.
 *
 * The list is newest-first, so `To` sits above `From` and the included span
 * is the block between them. A pair the list cannot place — one end on
 * another branch, one end older than the fifty commits loaded — leaves every
 * row plain rather than guessing at a span, because a half-drawn range would
 * assert a boundary in the wrong place, which is the confusion this whole
 * thing exists to remove.
 *
 * An inverted pick (`From` newer than `To`) is left plain for the same
 * reason. The picker says so in words; the rail declines to draw a span that
 * runs backwards.
 */
export function railRows(
  hashes: string[],
  fromRef: string,
  toRef: string,
  headHash: string | null = null,
): RailRow[] {
  const from = indexOf(hashes, fromRef, headHash);
  const to = indexOf(hashes, toRef, headHash);
  const drawable = from >= 0 && to >= 0 && to <= from;
  return hashes.map((hash, i) => {
    const included = drawable && i >= to && i <= from;
    return {
      hash,
      included,
      isFrom: drawable && i === from,
      isTo: drawable && i === to,
      isBase: drawable && i === from + 1,
      boundaryAbove: drawable && i === from + 1,
      litAbove: included && i > to,
      litBelow: included && i < from,
    };
  });
}

/** How many commits the comparison covers, or null when it cannot be drawn. */
export function includedCount(rows: RailRow[]): number | null {
  const n = rows.filter((r) => r.included).length;
  return n > 0 ? n : null;
}

/**
 * Why the picked pair cannot be drawn as a span, in the reader's terms.
 *
 * Null when it can, or when there is nothing yet to complain about — an
 * unfinished pick is not a mistake, and a warning under a half-filled form
 * reads as one.
 */
export function rangeWarning(
  hashes: string[],
  fromRef: string,
  toRef: string,
  headHash: string | null = null,
): string | null {
  if (!fromRef || !toRef) return null;
  const from = indexOf(hashes, fromRef, headHash);
  const to = indexOf(hashes, toRef, headHash);
  if (from < 0 || to < 0) {
    // Not an error: a base on one branch and a target on another is a
    // comparison this picker is built to make (UI-143). It just cannot be
    // drawn as a block of this list, and saying nothing would leave the
    // unmarked rail looking like a failure.
    return 'One end is not in the list below, so the range is not drawn. The comparison still runs.';
  }
  if (to > from) {
    return 'From is newer than To. The comparison will run backwards — swap them to read it as work added.';
  }
  return null;
}
