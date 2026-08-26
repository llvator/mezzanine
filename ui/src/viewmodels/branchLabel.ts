/**
 * How the canvas says which branch it is drawing (UI-114).
 *
 * The engine analyses one checkout: whatever `HEAD` points at, as it stands on
 * disk. Every other ref the UI names — the `From`/`To` of a comparison, a
 * stash's base, the index — is something the *overlay* describes, and the
 * server deliberately keeps the circles themselves on the working tree
 * (SRV-019). So there is exactly one branch worth putting on the canvas, and
 * this turns the server's answer about it into the chip's two strings.
 *
 * Pure, and its only import is a type, so `node --test` can reach it without
 * svelte:
 *
 *   npm run test:branch
 */

import type { BranchInfo } from '../stores/branch';

/** What the chip renders. `null` means render nothing at all. */
export interface BranchLabel {
  /** The short form, on the chip. */
  text: string;
  /** The long form, on hover. Says what the chip is a label *for*. */
  title: string;
  /** True when `HEAD` is on no branch — the chip styles that differently,
   *  because it is a state most readers did not choose deliberately. */
  detached: boolean;
}

/** The one sentence the tooltip always ends with. The chip is a claim about
 *  the circles, not about whatever comparison happens to be loaded, and that
 *  is the half a reader cannot see for themselves. */
const DRAWS = 'The graph draws this checkout’s working tree.';

/**
 * The chip for a server answer, or `null` when there is nothing to say.
 *
 * Nothing is said for a root that is not a git checkout — `mezz watch` runs
 * against any directory, and a chip reading "no branch" there would invent a
 * problem out of an ordinary state. It is also what an older engine, which has
 * no `/api/branch`, leaves the store holding.
 */
export function branchLabel(info: BranchInfo | null): BranchLabel | null {
  if (!info || !info.git) return null;

  if (info.branch) {
    // An unborn branch has a name and no commit. Saying so is the difference
    // between "you are on main" and "you are on main, and nothing is on it".
    const at = info.head_short
      ? `at ${info.head_short}`
      : 'with no commits yet';
    return {
      text: info.branch,
      title: `On branch ${info.branch}, ${at}. ${DRAWS}`,
      detached: false,
    };
  }

  // Detached. The commit is the only name there is, so it goes on the chip
  // rather than in the tooltip — "detached" alone tells the reader they are
  // lost without telling them where.
  const at = info.head_short ?? 'unknown';
  return {
    text: `detached at ${at}`,
    title:
      `HEAD is detached at ${at} — this checkout is on no branch, `
      + `so commits made here belong to nothing until one is pointed at them. `
      + DRAWS,
    detached: true,
  };
}
