/**
 * Why a node that exists in the loaded scope is not on the canvas.
 *
 * The search results list used to derive from `graphData` alone — the
 * *loaded scope* — and never consulted what the display plan actually drew.
 * So a hit could be listed, checked, committed, and still never appear,
 * because a kind toggle or a hidden file had removed it, with nothing on
 * screen saying so. The out-of-scope group had answered the equivalent
 * question for the scope tier since UI-016; this is the missing half.
 *
 * The per-node half of a pair. `viewmodels/filterPipeline` answers the same
 * question for the *view* — what is narrowing the canvas at all — and the two
 * are deliberately separate: this one is asked about a row the reader is
 * looking at, that one about a picture they are not sure they can trust.
 *
 * A leaf module by the rule in `context-for-issues-of-fuzzy-scope.md`: pure
 * functions, no store imports, so the view-model that owns the stores can
 * pass a snapshot and the result stays unit-testable. The caller maps a
 * reason to the action that reverses it — `blockReason` deliberately does
 * not know that `kind` is undone by `toggleEntityType`.
 */

import type { D3Node } from '../types/graph';
// Explicit `.ts` on the value imports: `scripts/search-results.test.ts` loads
// this module under `node --test` type stripping, which resolves no extensions.
import { isSpecNode } from '../types/graph.ts';
import { pathsClaim } from './refPaths.ts';

/** The filter state a node is judged against. Mirrors the fields of
 *  `ComputeArgs` that `nodePassesFilters` reads before it reaches the
 *  search restriction, and in the same order — the two must agree about
 *  which filter claims a node, or the badge names the wrong control. */
export interface FilterSnapshot {
  kinds: Set<string>;
  langs: Set<string>;
  files: Set<string>;
  showGhosts: boolean;
  showBuiltinGhosts: boolean;
  showTemplateVars: boolean;
  /** The split view draws the spec layer in its own pane (ADR 0011). */
  splitView: boolean;
  /** Paths the focused spec entity claims, or null when no cross-filter is
   *  running. `[]` means "claims nothing" and blocks everything. */
  crossFilterPaths: string[] | null;
}

/** Which control is holding the node back, and the value to hand the action
 *  that reverses it (`kind_raw`, a language, a file path). */
export type BlockKind =
  | 'ghost'
  | 'builtin-ghost'
  | 'template-var'
  | 'spec-layer'
  | 'cross-filter'
  | 'kind'
  | 'language'
  | 'file'
  | 'collapsed'
  | 'distance'
  | 'ceiling';

export interface BlockReason {
  kind: BlockKind;
  /** The specific value to re-admit — a `kind_raw`, a language, or a file
   *  path. Empty for reasons that take no argument. */
  value: string;
  /** Short badge text, e.g. `hidden: kind`. */
  label: string;
  /** Whether a per-row action can reverse this. `ceiling` cannot — the fix
   *  is to narrow the view, not to widen it. */
  reversible: boolean;
}

/**
 * The first filter that rejects `n`, or `null` when the hard filters all
 * pass.
 *
 * "First" matters: a node can fail several at once, and reporting the one
 * `nodePassesFilters` hits first means the badge names the control that is
 * actually deciding. Re-admitting it re-runs this and surfaces the next,
 * which is the honest behaviour — clicking once and having the row stay
 * blocked is confusing only if we claimed there was one cause.
 *
 * The search restriction is deliberately not consulted. A search hit that
 * is hidden by its own uncommitted search is not blocked by anything the
 * user needs to undo, and badging it would put a warning on every row.
 */
export function classifyBlock(n: D3Node, snap: FilterSnapshot): BlockReason | null {
  if (n.tags?.includes('ghost')) {
    if (!snap.showGhosts) {
      return { kind: 'ghost', value: '', label: 'hidden: ghosts', reversible: true };
    }
    if (!snap.showBuiltinGhosts && n.tags.includes('ghost_stdlib')) {
      return {
        kind: 'builtin-ghost',
        value: '',
        label: 'hidden: builtin ghosts',
        reversible: true,
      };
    }
  }
  if (!snap.showTemplateVars && n.tags?.includes('template_var')) {
    return { kind: 'template-var', value: '', label: 'hidden: template vars', reversible: true };
  }
  // Not hidden — drawn somewhere else. Worth a badge for the same reason
  // `collapsed` is: the row is off the code canvas, and "it is in the other
  // pane" is a different instruction from any of the reversible reasons
  // below. Nothing to undo, so it carries no action.
  if (snap.splitView && isSpecNode(n)) {
    return { kind: 'spec-layer', value: '', label: 'in the spec pane', reversible: false };
  }
  if (snap.crossFilterPaths && !pathsClaim(snap.crossFilterPaths, n.file_path)) {
    return { kind: 'cross-filter', value: '', label: 'hidden: spec filter', reversible: true };
  }
  if (!snap.kinds.has(n.kind_raw)) {
    return { kind: 'kind', value: n.kind_raw, label: `hidden: ${n.kind} filter`, reversible: true };
  }
  if (!snap.langs.has(n.language)) {
    return {
      kind: 'language',
      value: n.language,
      label: `hidden: ${n.language} filter`,
      reversible: true,
    };
  }
  if (n.file_path && !snap.files.has(n.file_path)) {
    return { kind: 'file', value: n.file_path, label: 'hidden: file filter', reversible: true };
  }
  return null;
}

/**
 * The entity is loaded and in the analysis scope, but the canvas is drawing
 * at File or Module level, so it has been folded into its container and has
 * no node of its own.
 *
 * Worth its own reason rather than being lumped in with "out of scope",
 * which is what it looked like before: the search corpus is entity-level and
 * `graphData` is not, so at File level *every* entity hit reported itself as
 * outside a scope it was plainly inside. The two have opposite fixes —
 * out-of-scope wants more data loaded, this wants the same data drawn finer.
 */
export const COLLAPSED_BLOCK: BlockReason = {
  kind: 'collapsed',
  value: '',
  label: 'collapsed into its file',
  reversible: true,
};

/** The node is filtered in but still off-canvas because a selection is
 *  active and it fell outside the BFS reach. Reversed by clearing the
 *  selection, which is why it carries no value. */
export const DISTANCE_BLOCK: BlockReason = {
  kind: 'distance',
  value: '',
  label: 'outside selection depth',
  reversible: true,
};

/** Nothing is drawn at all because the plan overflowed the draw ceiling.
 *  Not reversible per-row: the overflow card already asks the user to
 *  narrow, and re-admitting one node cannot help. */
export const CEILING_BLOCK: BlockReason = {
  kind: 'ceiling',
  value: '',
  label: 'over the draw ceiling',
  reversible: false,
};
