/**
 * How many lines a diff moved, per entity and per scope — the domain of the
 * `Lines changed` size channel.
 *
 * The obvious number was already on the wire: `diff.json` carries a `loc`
 * `MetricDelta` per modified entity, free, no sources needed. It is the wrong
 * number, and wrong in the direction that matters most. `delta` is
 * `new - old`, so a function whose body is rewritten line for line reports
 * zero and draws at the *minimum* radius — the largest change on the canvas
 * rendered as the smallest circle. That is the same trap `SourceDiff` already
 * documents one level down: dropping one call and adding another leaves
 * `fan_out` exactly where it was, so no metric delta reports it either. A
 * channel whose whole claim is "this is where the work is" cannot be built on
 * a net figure.
 *
 * So churn is added + removed, measured from the two sources the app already
 * holds: `/api/details` for the head side and `/api/details/base` for the
 * base side, both whole-repo maps fetched once when a diff loads. Nothing new
 * is computed on the engine and no endpoint is added.
 *
 * It goes through `computeLineDiff` + `changeCounts` — the same pair the
 * details pane renders from — so the circle on the canvas and the `+12 −7` in
 * the pane can never be two different readings of one entity. That also means
 * this inherits `LCS_BUDGET`: past it, a rewrite degrades to "the whole middle
 * replaced", which is what the pane shows there too and is an honest upper
 * bound rather than a different kind of answer.
 *
 * Store-free on purpose, like `diffRollup` beside it: the id normalizer and
 * the two source lookups are passed in, so the whole of this can be asserted
 * under `node --test` with no svelte and no browser (`npm run test:churn`).
 */

import type { EntityDiff } from '../stores/diff';
// Extensions on both, the way `viewHistory` imports `savedViews`: this module
// is loaded by `node --test`'s type stripping, which resolves ESM specifiers
// literally and does not guess at a `.ts`.
import { normalizeScopePath, scopeChain } from './diffRollup.ts';
import { changeCounts, computeLineDiff } from '../utils/lineDiff.ts';

/** Where a row's two sides come from, and what key it is filed under.
 *
 *  All three are injected because each needs something this module must not
 *  import: the head map is keyed by the graph's `original_id`, the base map by
 *  the row's own `base_entity_id`, and the key is the `normalizeEntityId`
 *  spelling the canvas looks nodes up by — which lives in `stores/diff`. */
export interface ChurnSources {
  /** Source as of the head side, or undefined when it can't be found. */
  head(e: EntityDiff): string | undefined;
  /** Source as of the base side. */
  base(e: EntityDiff): string | undefined;
  /** The key `byEntity` is filed under. */
  key(e: EntityDiff): string;
}

export interface ChurnIndex {
  /**
   * Entity key → lines added + removed.
   *
   * An unchanged entity is present with `0`, which is a *fact* about it and
   * has to draw at the bottom of the ramp. A key that is **absent** is the
   * other thing entirely — the row exists but neither side's source could be
   * read — and has to draw hollow at `NO_DATA_RADIUS`. Collapsing the two is
   * the one bug this channel could ship with that the reader could not see.
   */
  byEntity: Map<string, number>;
  /**
   * File path and directory path → the sum over the entities inside it, for
   * the File and Module aggregation levels.
   *
   * Absent under the same rule, applied to the scope: a scope where something
   * changed and *nothing* about it could be measured is unknown, not zero. A
   * scope where some rows measured and one did not reports the sum it has,
   * which understates rather than withholding a whole folder over one
   * unreadable entity.
   */
  byScope: Map<string, number>;
}

/** Lines in a source blob. Empty string is zero lines, not one. */
function lineCount(src: string | undefined): number | undefined {
  if (src === undefined) return undefined;
  return src === '' ? 0 : src.split('\n').length;
}

/**
 * Churn for one diff row, or undefined when it cannot be measured.
 *
 * An entity that arrived or left is every line of itself — the same rule
 * `SourceDiff` renders by, and the reason a new 200-line module has to draw
 * large rather than at the bottom of the ramp.
 *
 * A `modified` row with `source_changed: false` moved no lines at all: only
 * its fan-in or fan-out changed, because something else in the repo did. That
 * is a real zero and not a missing measurement, which is what keeps the
 * ripple around a change small instead of drawing it as unknown.
 */
export function rowChurn(e: EntityDiff, src: ChurnSources): number | undefined {
  switch (e.status) {
    case 'unchanged':
      return 0;
    case 'added':
      return lineCount(src.head(e));
    case 'removed':
      return lineCount(src.base(e));
    default:
      break;
  }
  if (!(e.source_changed ?? true)) return 0;
  const base = src.base(e);
  const head = src.head(e);
  if (base === undefined || head === undefined) return undefined;
  // Identical text short-circuits the quadratic part. Reached more often than
  // it looks: a re-render with a moved span is a `modified` row.
  if (base === head) return 0;
  const { added, removed } = changeCounts(computeLineDiff(base, head));
  return added + removed;
}

interface ScopeAcc {
  lines: number;
  /** Rows under this scope whose churn could not be read. */
  unmeasured: number;
}

export function buildChurnIndex(
  entities: readonly EntityDiff[],
  src: ChurnSources,
): ChurnIndex {
  const byEntity = new Map<string, number>();
  const scopes = new Map<string, ScopeAcc>();

  for (const e of entities) {
    const churn = rowChurn(e, src);
    if (churn !== undefined) byEntity.set(src.key(e), churn);
    for (const scope of scopeChain(normalizeScopePath(e.file_path ?? ''))) {
      let acc = scopes.get(scope);
      if (!acc) {
        acc = { lines: 0, unmeasured: 0 };
        scopes.set(scope, acc);
      }
      if (churn === undefined) acc.unmeasured++;
      else acc.lines += churn;
    }
  }

  const byScope = new Map<string, number>();
  for (const [scope, acc] of scopes) {
    // Nothing measured and something did change: the scope is unknown. Zero
    // would claim the folder is untouched, which is the opposite of the truth.
    if (acc.lines === 0 && acc.unmeasured > 0) continue;
    byScope.set(scope, acc.lines);
  }
  return { byEntity, byScope };
}
