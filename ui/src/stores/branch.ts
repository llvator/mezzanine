/**
 * Which branch the analysed checkout is on (UI-114).
 *
 * The graph has always been a picture of one checkout, and the UI has always
 * been able to compare *other* commits against it. Those two facts together
 * are why this store exists: with a comparison loaded the canvas is covered in
 * refs — `From`, `To`, a stash's base — and not one of them is the branch the
 * circles came from.
 *
 * Kept current by the engine rather than by polling. `GET /api/branch` is two
 * git calls, but asking it on a timer would mean asking forever on a repo
 * nobody is touching; the watcher instead announces a moved `HEAD` on the SSE
 * stream (`head`), which is the only event that can make this stale.
 */

import { writable, get } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';

/** `GET /api/branch`. Mirrors `BranchInfo` in src/server/types.rs. */
export interface BranchInfo {
  /** The branch HEAD is on, `null` when detached or when `git` is false. */
  branch: string | null;
  /** HEAD names a commit rather than a branch. */
  detached: boolean;
  /** Abbreviated HEAD commit; `null` on an unborn branch. */
  head_short: string | null;
  /** Whether git could answer about the analysed root at all. */
  git: boolean;
}

/**
 * What the engine last said, or `null` for "nothing to show".
 *
 * `null` covers three cases on purpose, because the chip treats them the
 * same: not asked yet, the root is not a checkout, and an engine too old to
 * have the endpoint. None of them is a problem the reader can act on, and a
 * chip that appeared to report one would be inventing it.
 */
export const branchInfo = writable<BranchInfo | null>(null);

/**
 * Ask the engine what HEAD is.
 *
 * Failures leave the last known answer standing rather than blanking the
 * chip: a request lost while the engine restarts does not mean the branch
 * changed, and flickering the label off and on again would read as if it had.
 * The one exception is the first call, which starts from `null` anyway.
 */
export async function fetchBranch(): Promise<void> {
  try {
    const resp = await fetch(apiUrl('/api/branch'), { cache: 'no-store' });
    if (!resp.ok) return;
    const data = await resp.json();
    if (data && typeof data === 'object') branchInfo.set(data as BranchInfo);
  } catch {
    // Keep whatever we had. See above.
  }
}

/** Synchronous read, for the places that report state outwards rather than
 *  subscribe to it (the VS Code bridge). */
export function currentBranch(): BranchInfo | null {
  return get(branchInfo);
}
