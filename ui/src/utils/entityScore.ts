/**
 * How an entity's fields compete for one score.
 *
 * The fuzzy matcher in `fuzzyPath.ts` scores a query against *one* string.
 * An entity is four of them — name, qualified name, filename, folder — and
 * turning four scores into the number that orders the result list is a
 * separate decision from how any one of them is computed. This is that
 * decision, on its own.
 *
 * The per-string scorer arrives as a parameter rather than as an import.
 * That is what makes this file loadable by the Node test runner, which
 * resolves no extensionless `.ts` imports — the same constraint that put
 * `drawCeiling` and `emptyCanvas` in their own modules. It also means the
 * weighting can be asserted against a stub scorer, so a test of the policy
 * is not also a test of the matcher.
 */

import type { D3Node } from '../types/graph';

/**
 * Per-field weights, applied before the fields compete.
 *
 * This is an *entity* search: the entity's own name is the thing being
 * asked for and the path is the context it sits in. That is the mirror of
 * the scope box, which discounts entity names against paths for the
 * equivalent reason (UI-043) — there "graph" means the file, here it means
 * the function. `qualifiedName` sits just under `name` because a hit landing
 * only on the qualifier, the enclosing class or module, is weaker evidence
 * than one on the name the user typed.
 */
export const FIELD_WEIGHT = {
  name: 1,
  qualifiedName: 0.9,
  filename: 0.8,
  folder: 0.6,
} as const;

export interface ScoreFields {
  inNames: boolean;
  inFiles: boolean;
  inFolders: boolean;
}

/** Split a file path into (folder, filename) so each can be matched
 *  independently without the other's characters polluting the search. */
export function splitPath(fp: string): { folder: string; filename: string } {
  const i = fp.lastIndexOf('/');
  if (i < 0) return { folder: '', filename: fp };
  return { folder: fp.slice(0, i), filename: fp.slice(i + 1) };
}

/**
 * The entity's score, or `null` when it does not match.
 *
 * The best-scoring enabled field wins rather than the sum: summing rewards a
 * node for matching in three mediocre places, which would let it outrank one
 * that matched the name exactly. `0` is a legitimate score, so callers must
 * test against `null` and not for falsiness.
 *
 * @param scoreOne  Scores the query against one string; `null` for no match.
 * @param rejected  True when a negated term matches the string. Checked
 *                  against every string that identifies the node, not just
 *                  the one that happened to score. Scoped per-field,
 *                  `graph !test` returned an entity named `graph` living in
 *                  `foo.test.ts`, because the name alone satisfied both
 *                  terms — exclusion has to be about the thing excluded.
 */
export function scoreEntity(
  n: D3Node,
  fields: ScoreFields,
  kinds: Set<string>,
  scoreOne: (candidate: string) => number | null,
  rejected: (candidate: string) => boolean,
): number | null {
  if (kinds.size > 0 && !kinds.has(n.kind_raw)) return null;

  if (rejected(n.name)) return null;
  if (n.qualified_name && rejected(n.qualified_name)) return null;
  if (n.file_path && rejected(n.file_path)) return null;

  let best: number | null = null;
  const consider = (raw: number | null, weight: number) => {
    if (raw === null) return;
    const weighted = raw * weight;
    if (best === null || weighted > best) best = weighted;
  };

  if (fields.inNames) {
    consider(scoreOne(n.name), FIELD_WEIGHT.name);
    // Skip the qualified name when it adds nothing over the name — the
    // common case for top-level entities, and this runs per node per
    // keystroke.
    if (n.qualified_name && n.qualified_name !== n.name) {
      consider(scoreOne(n.qualified_name), FIELD_WEIGHT.qualifiedName);
    }
  }
  if ((fields.inFiles || fields.inFolders) && n.file_path) {
    const { folder, filename } = splitPath(n.file_path);
    if (fields.inFiles) consider(scoreOne(filename), FIELD_WEIGHT.filename);
    if (fields.inFolders && folder) consider(scoreOne(folder), FIELD_WEIGHT.folder);
  }
  return best;
}
