/**
 * What the engine is doing right now (UI-138).
 *
 * Every phase line the engine printed — `Analyzing base (784646003a) …`,
 * `Computing structural diff…`, the parse warnings — used to exist only on
 * the terminal that launched `mezz watch`. The SSE stream carried nothing but
 * *completion* pings, so a diff that takes seventy seconds on a large
 * repository left this page showing an unchanged canvas and no reason to
 * believe anything was happening.
 *
 * This is the transport half only: the stream, the catch-up fetch, and one
 * store. Every decision about what a frame *means* is in
 * [`../viewmodels/activityFeed`], which is pure and tested.
 */

import { derived, get, writable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import {
  applySnapshot,
  applyUpdate,
  EMPTY_ACTIVITY,
  newestFirst,
  warningCount,
  type ActivityState,
  type Running,
  type Snapshot,
  type Update,
} from '../viewmodels/activityFeed';

export type { ActivityKind, Notice, Running, Update } from '../viewmodels/activityFeed';

const state = writable<ActivityState>(EMPTY_ACTIVITY);

/** The run in flight, or `null` when the engine is idle. */
export const engineRunning = derived(state, ($s): Running | null => $s.running);

/** Recent notices, newest first — the order the panel reads in. */
export const engineFeed = derived(state, newestFirst);

/** True while the engine is working on something. */
export const engineBusy = derived(state, ($s) => $s.running !== null);

/** Warnings among the notices still retained — the badge on the chip. */
export const engineWarnings = derived(state, warningCount);

/** Fold in one live frame from the `activity` SSE event. */
export function ingestUpdate(update: Update | null): void {
  state.update((s) => applyUpdate(s, update));
}

/**
 * Ask for everything this page has not seen.
 *
 * Failures leave what we had standing. A request lost while the engine
 * restarts does not mean the engine went idle, and blanking the status line
 * would say that it did.
 */
export async function fetchActivity(): Promise<void> {
  const since = get(state).seq;
  try {
    const resp = await fetch(apiUrl(`/api/activity?since=${since}`), { cache: 'no-store' });
    if (!resp.ok) return;
    const snap = (await resp.json()) as Snapshot;
    state.update((s) => applySnapshot(s, snap));
  } catch {
    // Keep whatever we had. See above.
  }
}

/**
 * Drop everything.
 *
 * For when the page stops having grounds for what it is showing: a stream
 * that gave up, or a repoint at another engine whose sequence numbers have
 * nothing to do with this one's.
 */
export function resetActivity(): void {
  state.set(EMPTY_ACTIVITY);
}

/** Synchronous read, for the places that report state outwards rather than
 *  subscribe to it. */
export function currentActivity(): Running | null {
  return get(state).running;
}
