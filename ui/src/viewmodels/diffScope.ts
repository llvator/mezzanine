/**
 * diffScope — the scope a comparison already implies.
 *
 * A diff names its own subject. `diff.json` is one row per entity and every
 * row carries the file it is in, so "which files did this change" is a
 * question the canvas can answer without anyone picking folders first. Until
 * UI-135 nothing asked it: the picture a comparison landed in was whatever the
 * scope tree happened to be set to, which on a repository above the render
 * budget meant folder circles or a blank canvas, and a reader shrinking the
 * scope by hand to find the change they had just asked to see.
 *
 * Two rules make the answer usable, and both were learned the expensive way.
 * They lived inside a `case` of App.svelte's message switch, where nothing
 * could test them; this module is that logic with the store access taken out,
 * so the button and the automatic path cannot drift and the rules can be
 * asserted without a browser.
 */

// The explicit extension is what lets the node test runner load this module
// directly — the convention `diffChurn.ts` and `mixedGrain.ts` already follow
// for a value import of a sibling.
import { isSourceEdit } from './changedFiles.ts';
import type { EntityDiff } from '../stores/diff';
import type { IndexData } from '../stores/scope';

/** What a comparison implies about the scope, and what it left out getting
 *  there. The two dropped counts are logged rather than shown — they are how
 *  a scope that comes out wrong is diagnosed. */
export interface ChangedFileScope {
  /** Leaf file paths to scope to, de-duplicated. */
  paths: string[];
  /** Rows that were ripple, not edit — see below. */
  droppedImpactOnly: number;
  /** Paths the index does not hold as files: folders, and ghosts. */
  droppedNonLeaf: string[];
}

/**
 * The files this comparison changed, as a scope.
 *
 * **Edits only, never the ripple.** A `modified` row whose `fan_in`/`fan_out`
 * moved because something *elsewhere* changed has not been edited, and on a
 * real diff those outnumber the edits several times over — 474 to 60 on the
 * comparison UI-134 was built under. Scoping to them would widen the scope to
 * most of the repository, which is the condition this exists to relieve. The
 * rule is `isSourceEdit`'s, borrowed rather than restated: the `Changes` pane
 * and the scope have to agree about what "changed" means or the pane accuses
 * the canvas of drawing files it never drew.
 *
 * **Leaf files only.** `minimizeSelection` treats a folder as covering
 * everything under it, so one folder path among the rows silently widens the
 * scope back to where it started. A row whose `file_path` is not a file in the
 * index — a ghost's empty path, or a path the index holds as a folder — is
 * dropped rather than trusted.
 *
 * With no index the paths pass through unfiltered: the caller then has nothing
 * to check them against, and an unscoped comparison is a better failure than
 * an empty one.
 */
export function changedFileScope(
  entities: readonly EntityDiff[] | null | undefined,
  index: IndexData | null,
): ChangedFileScope {
  if (!entities || entities.length === 0) {
    return { paths: [], droppedImpactOnly: 0, droppedNonLeaf: [] };
  }

  const edits = entities.filter(isSourceEdit);
  const named = [
    ...new Set(
      edits.map((e) => e.file_path).filter((p) => typeof p === 'string' && p.length > 0),
    ),
  ];

  const paths = index ? named.filter((p) => index.nodes[p]?.type === 'file') : named;
  const kept = new Set(paths);

  return {
    paths,
    droppedImpactOnly: entities.length - edits.length,
    droppedNonLeaf: named.filter((p) => !kept.has(p)),
  };
}
