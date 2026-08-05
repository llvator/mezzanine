/**
 * Roll per-entity diff statuses up to the scopes the collapsed graph draws
 * (UI-064).
 *
 * `diffStatusMap` is keyed by entity id. Above entity aggregation the canvas
 * is not drawing entities: `collapseGraph` emits one node per file or per
 * directory and puts the *scope path* in `original_id`. An entity-keyed
 * lookup therefore misses on every collapsed node, and a miss used to read as
 * "unchanged" — which, with `diffCoreOnly` defaulting on and `diffDimOpacity`
 * defaulting to 0, emptied the canvas the moment a diff was loaded.
 *
 * The fix is to answer the question the collapsed canvas is actually asking:
 * *did anything inside this scope change?* That is a rollup, and it belongs
 * here rather than in `displayPlan` for the same reason `drawCeiling` does —
 * it is a decision worth testing on its own, and this module's only import is
 * a type, so Node's type stripping can load it without a browser.
 *
 * Two properties the rest of the app leans on:
 *
 * - **Every scope the collapsed canvas can draw is a key.** So `has(path)`
 *   answers "did the diff look at this scope at all", which is what
 *   separates *unknown* from *unchanged* — a distinction the old code did
 *   not make, and the reason a file created after the diff was computed
 *   vanished instead of standing out.
 * - **A scope is `unchanged` only when nothing inside it moved.** Anything
 *   else rolls up to a real status, so `changesOnly` and `coreOnly` mean the
 *   same thing at File and Module level that they mean at entity level.
 */

import type { ChangeStatus, EntityDiff } from '../stores/diff';

/** A scope's change, rolled up from the entities inside it. */
export interface ScopeChange {
  status: ChangeStatus;
  /** True when at least one *changed* entity in the scope changed source or
   *  intrinsic metrics, rather than only its fan-in/fan-out. Mirrors
   *  `EntityDiff.source_changed`, which is what `coreOnly` filters on. */
  sourceChanged: boolean;
}

interface Tally {
  added: number;
  removed: number;
  modified: number;
  unchanged: number;
  /** Changed entities that are core changes. */
  core: number;
}

/**
 * Strip the temp-worktree prefix a diff-side path can carry.
 *
 * `diff.rs` already emits `file_path` relative to the analysis root, so this
 * is belt-and-braces for the paths that arrive from a base worktree — the
 * same prefix `normalizeEntityId` strips from entity ids, for the same
 * reason: the graph's paths are repo-relative and the two have to meet.
 */
export function normalizeScopePath(p: string): string {
  const m = p.match(/nao-diff-(?:head|base)-[^/]+\/(.+)$/);
  return m ? m[1] : p;
}

/**
 * The scopes a file is a member of, as the collapsed canvas defines
 * membership: the file itself, and its immediate directory.
 *
 * Deliberately *not* every ancestor. `collapseGraph` puts a file in exactly
 * one module — its parent directory — so `ui/src/stores/diff.ts` belongs to
 * the `ui/src/stores` node and not to the `ui/src` node drawn beside it.
 * Rolling all the way up would make `ui/src` claim a change that is already
 * drawn on its neighbour, and a `changesOnly` view would then show modules
 * whose own files never moved.
 *
 * A top-level file's directory is the root `''`, which is the scope id a
 * module-level collapse gives it.
 */
export function scopeChain(filePath: string): string[] {
  const i = filePath.lastIndexOf('/');
  const dir = i < 0 ? '' : filePath.slice(0, i);
  return dir === filePath ? [filePath] : [filePath, dir];
}

function tallyOf(map: Map<string, Tally>, scope: string): Tally {
  let t = map.get(scope);
  if (!t) {
    t = { added: 0, removed: 0, modified: 0, unchanged: 0, core: 0 };
    map.set(scope, t);
  }
  return t;
}

/**
 * Collapse a tally into the status a reader would give the scope.
 *
 * A scope that is *entirely* new reads as `added` and one that is entirely
 * gone reads as `removed` — those are the cases where the file-level colour
 * can say something stronger than "something in here moved". Any mix is
 * `modified`, which is the honest answer for a file that gained a function.
 */
function verdict(t: Tally): ScopeChange {
  const changed = t.added + t.removed + t.modified;
  if (changed === 0) return { status: 'unchanged', sourceChanged: false };
  let status: ChangeStatus = 'modified';
  if (t.unchanged === 0 && t.added === changed) status = 'added';
  else if (t.unchanged === 0 && t.removed === changed) status = 'removed';
  return { status, sourceChanged: t.core > 0 };
}

/**
 * Build the scope → change index from a diff's entity list.
 *
 * The diff reports on *every* head entity, unchanged ones included, so the
 * resulting key set is the full file tree as of the moment the diff ran.
 * That is what makes a missing key meaningful.
 */
export function rollUpByScope(entities: readonly EntityDiff[]): Map<string, ScopeChange> {
  const tallies = new Map<string, Tally>();
  for (const e of entities) {
    const path = normalizeScopePath(e.file_path ?? '');
    const isCore = (e.source_changed ?? true) && e.status !== 'unchanged';
    for (const scope of scopeChain(path)) {
      const t = tallyOf(tallies, scope);
      switch (e.status) {
        case 'added': t.added++; break;
        case 'removed': t.removed++; break;
        case 'modified': t.modified++; break;
        default: t.unchanged++; break;
      }
      if (isCore) t.core++;
    }
  }
  const out = new Map<string, ScopeChange>();
  for (const [scope, t] of tallies) out.set(scope, verdict(t));
  return out;
}
