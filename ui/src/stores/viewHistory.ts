/**
 * The wayback, wired to the stores (UI-092).
 *
 * The rules for what a step *is* live in `viewmodels/viewHistory.ts`; this
 * module is the three things that need the stores: hearing that a navigation
 * is about to happen, reading the picture out, and writing one back.
 *
 * All three already existed. `stores/scope.ts` grew a `onBeforeNavigate`
 * hook, and UI-082 built the codec — `captureState` reads every store a
 * picture is made of, `restoreState` writes them back in the order the
 * republish demands. A history is that codec plus two arrays, which is why
 * there is no second notion of "a view" anywhere in here.
 *
 * **Not persisted, on purpose.** UI-082 drew a line between a reading (what
 * the canvas is drawing) and a position (where the reader is standing), and
 * put only the first in `.mezz/views.json`. A history is squarely the second:
 * it dies with the tab, the way the camera does. The durable way back to a
 * picture is to have saved it.
 *
 * **Registered at import.** The hook fires from module scope, so this file
 * has to be loaded before the first navigation to catch it. `keymapActions`
 * imports it and is itself imported by `App`, so it is live before the first
 * frame renders. If that ever stops being true the failure is benign —
 * navigation keeps working and stops being remembered — but the controls
 * would be permanently disabled, which is the symptom to look for.
 */

import { derived, get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { onBeforeNavigate, withoutNavigation } from './scope';
import { captureState, restoreState } from './savedViews';
import type { ViewState } from '../viewmodels/savedViews';
import {
  NO_HISTORY,
  canGoBack as hasBack,
  canGoForward as hasForward,
  peekBack,
  peekForward,
  record,
  stepBack,
  stepForward,
  stepTitle,
  type ViewHistory,
} from '../viewmodels/viewHistory';

const history = writable<ViewHistory>(NO_HISTORY);

export const canGoBack: Readable<boolean> = derived(history, hasBack);
export const canGoForward: Readable<boolean> = derived(history, hasForward);

/** What each control should say it leads to — the destination's own summary,
 *  so "Back" names the picture rather than the direction. */
export const backTitle: Readable<string> = derived(history, ($h) => stepTitle('Back', peekBack($h)));
export const forwardTitle: Readable<string> = derived(history, ($h) =>
  stepTitle('Forward', peekForward($h)),
);

/** How deep the way back goes, for the control's badge. */
export const backDepth: Readable<number> = derived(history, ($h) => $h.back.length);

// The one subscriber. `record` decides whether the picture is worth keeping
// (a scope of nothing is not) and whether it is new (re-applying the scope
// already on screen is not a step).
onBeforeNavigate(() => history.update((h) => record(h, captureState())));

/**
 * Take one step, if there is one that way.
 *
 * `withoutNavigation` is load-bearing: `restoreState` writes the same stores
 * every gesture writes, and without it a back press would record itself as a
 * navigation — pushing the frame it was in the middle of leaving, so the
 * stack never got shorter and forward pointed at where you already were.
 *
 * The stack is advanced *before* the restore rather than after, so a second
 * press landing during the republish steps from the new position instead of
 * repeating the first one.
 */
async function step(direction: 'back' | 'forward'): Promise<boolean> {
  const take = direction === 'back' ? stepBack : stepForward;
  const next = take(get(history), captureState());
  if (!next) return false;
  history.set(next.history);
  await withoutNavigation(() => restoreState(next.restore));
  return true;
}

export function goBack(): Promise<boolean> {
  return step('back');
}

export function goForward(): Promise<boolean> {
  return step('forward');
}
