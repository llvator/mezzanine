/**
 * What a diff says about one drawn node — and, when it says nothing, whether
 * that silence means *interesting* or *out of scope* (UI-104).
 *
 * This used to live inside `displayPlan` as three private functions. It moved
 * out because the rule that decides the silent case turned out to depend on
 * something none of them knew: **which tree the diff actually looked at**.
 *
 * A `→ working` diff analyses the very tree the canvas is drawing, so the only
 * files it never saw are ones created since it ran — a handful, and exactly
 * what the reader opened the diff to see. Showing them is right.
 *
 * A commit-to-commit diff does not. The server deliberately declines to adopt
 * a commit's head graph (SRV-019: it is rooted in a temp worktree that has
 * already been deleted), so the canvas keeps drawing the working tree while
 * the diff describes some pair of older trees. There "the diff never saw this
 * file" means *every file added in the N commits since*, which grows with N
 * and has nothing to do with the change being compared. Measured on this repo
 * at ~120 commits back: a comparison reporting **3** changed functions seeded
 * the `edits` rung with **2,263** nodes, 2,262 of them merely younger than the
 * head commit. That is over `DRAW_CEILING`, so the canvas refused to draw
 * anything at all — and because the flood was in the *seed* set, all three
 * rungs of the ladder drew the same picture. The diff filters looked broken
 * because everything they could add was already in.
 *
 * Pure, and its only import is a type, so `node --test` can reach it without
 * svelte:
 *
 *   npm run test:verdict
 */

import type { ChangeStatus } from '../stores/diff';
import type { ScopeChange } from './diffRollup';
import { normalizeEntityId } from './diffRollup.ts';
import type { EditKind } from './diffLevels';

export type { EditKind };

/** As much of a node as any of these questions needs. */
export interface VerdictNode {
  /** Entity id at entity level; scope path on a collapsed File / Module node. */
  original_id: string;
  /** Repo-relative, and empty on a ghost. */
  file_path: string;
}

/** The diff, reduced to the three lookups and the one fact below. */
export interface DiffFacts {
  statuses: ReadonlyMap<string, ChangeStatus>;
  sourceChanged: ReadonlyMap<string, boolean>;
  /** Scope path → rolled-up change. Its key set is the whole file tree as of
   *  the moment the diff ran, which is what makes a *missing* key mean
   *  something. */
  scopes: ReadonlyMap<string, ScopeChange>;
  /**
   * Whether the diff's head is the tree on screen.
   *
   * `diff.json` carries this literally: `to_ref` is the string `working` for a
   * working-tree diff and a sha otherwise. The whole of `unknownVerdict` turns
   * on it.
   */
  headIsWorking: boolean;
}

/**
 * What the diff says about a node, or null when it has nothing to say.
 *
 * Entity nodes are keyed by entity id. A collapsed File or Module node carries
 * its scope path in `original_id` instead, so the entity lookup misses and the
 * scope rollup answers (UI-064). Entity ids and scope paths do not collide —
 * an id carries `:line:name` — so trying them in this order needs no
 * discriminator on the node.
 */
export function changeOf(n: VerdictNode, facts: DiffFacts): ScopeChange | null {
  const id = normalizeEntityId(n.original_id);
  const status = facts.statuses.get(id);
  if (status) {
    return { status, sourceChanged: facts.sourceChanged.get(id) ?? true };
  }
  return facts.scopes.get(n.original_id) ?? null;
}

/**
 * Verdict for a node the diff never mentioned.
 *
 * Unknown is not unchanged, and conflating the two is how the interesting
 * nodes disappear. But unknown is not *edited* either, and conflating those
 * two is how the uninteresting ones flood in. Which mistake is available
 * depends on whether the diff looked at this tree:
 *
 * - **head is the working tree.** The diff reports on unchanged entities too,
 *   so its scope keys are the whole file tree as of the moment it ran. A file
 *   missing from them was created since; dimming it to `diffDimOpacity: 0`
 *   would hide exactly the thing the reader opened a diff to see. Show it.
 *   A file that *is* in the tree but whose entity is not in the maps is one of
 *   the kinds the diff skips (`Parameter`) or a ghost — dim it.
 * - **head is a commit.** The tree on screen is not the tree the diff looked
 *   at, so "missing" says nothing about the comparison. Dim it, like anything
 *   else the diff did not report as changed.
 *
 * Ghosts carry an empty `file_path` and dim under either rule, leaving their
 * visibility to `showGhosts` where it belongs.
 */
export function unknownVerdict(n: VerdictNode, facts: DiffFacts): 'visible' | 'dimmed' {
  if (!facts.headIsWorking) return 'dimmed';
  if (n.file_path && !facts.scopes.has(n.file_path)) return 'visible';
  return 'dimmed';
}

/**
 * Which side of the change this node is on, or null when it is not an edit at
 * all (UI-109).
 *
 * The two populations `edits` has always held together, and which the reader
 * asks about separately: **new** is code that did not exist on the base side,
 * **existing** is code that did and was changed in place. A deletion is
 * `existing` — it existed — which is also why the control offers three buttons
 * and not two: `New` / `Edited` would imply it covers the seed, and a
 * disappearance is neither of those words.
 *
 * Two ways to qualify as an edit, and the second is the one that is easy to
 * get wrong in both directions:
 *
 * - the diff says it changed, at the source level. Impact-only movement
 *   (`fan_in`/`fan_out` shifting because something nearby changed) is not an
 *   edit — on most diffs the ripple outnumbers the real edits and drowns them.
 * - the diff never looked, *and* it was looking at this tree. See
 *   `unknownVerdict`.
 *
 * That second case has no `ChangeStatus` to read — the maps are silent about
 * it, which is the whole point of the rule — and it is counted **new**. Under
 * a working head those are the files created since the diff ran, so new is
 * what they are; and it is the direction whose failure is visible (a node you
 * did not expect) rather than silent (a node that is gone from the one view
 * named for new code). Under a commit head `unknownVerdict` dims it and it is
 * not in the seed at all, which is UI-105's rule and stays untouched here.
 */
export function editKind(n: VerdictNode, facts: DiffFacts): EditKind | null {
  const change = changeOf(n, facts);
  if (!change) return unknownVerdict(n, facts) === 'visible' ? 'new' : null;
  if (change.status === 'unchanged' || !change.sourceChanged) return null;
  return change.status === 'added' ? 'new' : 'existing';
}

/**
 * Does the diff call this node *edited*?
 *
 * Derived from `editKind` rather than deciding it again: one rule, so the
 * ladder's seed and the facet that splits it can never disagree about what an
 * edit is.
 */
export function isEdit(n: VerdictNode, facts: DiffFacts): boolean {
  return editKind(n, facts) !== null;
}

/**
 * Does `to_ref` name the working tree?
 *
 * The server writes the literal `working` there for a `→ WORKING` diff and a
 * resolved sha for any commit. Read through a helper rather than compared
 * inline at each call site, because the string is a wire-format detail: it is
 * set in `compute_diff_blocking` and travels through `compute_diff` into
 * `diff.json`.
 */
export const WORKING_TREE_REF = 'working';

export function headIsWorkingTree(toRef: string | null | undefined): boolean {
  return toRef === WORKING_TREE_REF;
}
