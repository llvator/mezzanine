/**
 * The wayback: what "back" and "forward" mean over the pictures the canvas
 * has drawn (UI-092).
 *
 * A frame is a `ViewState` — the same record UI-082 defined for saved views,
 * and deliberately not a second one. A saved view and a history frame differ
 * in exactly one respect: somebody named the view. So this module holds no
 * codec of its own; `stores/savedViews.ts` reads and writes the stores, and
 * this decides only which frame is next.
 *
 * The model is the browser's, because it is the one every reader already has
 * in their hands: `back` is a stack of where you were, `forward` a stack of
 * where you came back from, and any new navigation makes the forward stack
 * unreachable rather than merging into it. What is *not* the browser's model
 * is what counts as a navigation — see `stores/scope.ts`, which records
 * gestures that replace the picture and ignores the ones that adjust it.
 *
 * Pure and store-free so the whole thing is unit-testable (`npm run
 * test:history`) without dragging `applySelection` and a graph fetch in
 * behind it.
 */

// Extension-bearing, like `regionSpec.ts`: these are value imports, and the
// node test runner resolves this file directly rather than through Vite.
import { sameState, stateSummary, type ViewState } from './savedViews.ts';

/**
 * Where the reader has been, and where they came back from.
 *
 * The *current* picture is not in here. It lives in the stores, where every
 * control can keep editing it, and is read out with `captureState()` at the
 * moment a step is taken — which is what lets back work honestly after the
 * reader has drifted from the frame they last restored.
 */
export interface ViewHistory {
  /** Most recent last — the top of the stack is `back[back.length - 1]`. */
  back: ViewState[];
  forward: ViewState[];
}

export const NO_HISTORY: ViewHistory = { back: [], forward: [] };

/**
 * How many frames back the reader can reach.
 *
 * A cap rather than unbounded because a frame is a whole `ViewState` — scope
 * rules, two id lists, the per-level override table — and a long session
 * navigates hundreds of times. Fifty is far past the depth anyone walks back
 * by pressing a button repeatedly; past that the way back to a reading is to
 * have saved it.
 */
export const HISTORY_LIMIT = 50;

/**
 * Is this picture worth being able to return to?
 *
 * An empty scope draws nothing, and the first navigation of every session
 * replaces exactly that: the app opens with no scope and picks one
 * (`App.svelte`'s first-run auto-scope). Recording it would put a blank
 * canvas at the bottom of every stack and make the back button's last press
 * a punishment.
 *
 * Only the includes count. A state holding nothing but exclusions still
 * draws nothing, which is the same case.
 */
export function worthRecording(state: ViewState): boolean {
  return state.scope.some((r) => !r.negate);
}

/**
 * Push `frame` as the picture being navigated away from.
 *
 * Two frames the reader cannot tell apart are one frame: a gesture that
 * re-applies the scope already on screen (clicking the same region twice,
 * a VS Code file focus for the file already focused) must not cost a back
 * press to undo. Comparison is `sameState`, so insertion order in the
 * captured sets never makes two identical pictures look different.
 *
 * The forward stack is dropped, not merged. It described a future reached
 * from a picture the reader has now left; keeping it would offer a forward
 * button that lands somewhere they never were.
 */
export function record(history: ViewHistory, frame: ViewState): ViewHistory {
  if (!worthRecording(frame)) return history.forward.length ? { ...history, forward: [] } : history;
  const top = history.back[history.back.length - 1];
  if (top && sameState(top, frame)) {
    return history.forward.length ? { ...history, forward: [] } : history;
  }
  const back = [...history.back, frame];
  return { back: back.slice(-HISTORY_LIMIT), forward: [] };
}

/** One step, and where it lands. `null` when the stack is empty that way. */
export interface Step {
  history: ViewHistory;
  restore: ViewState;
}

export function canGoBack(history: ViewHistory): boolean {
  return history.back.length > 0;
}

export function canGoForward(history: ViewHistory): boolean {
  return history.forward.length > 0;
}

/**
 * Step back, putting `current` on the forward stack.
 *
 * `current` is passed in rather than remembered from the last step because
 * the reader may have changed the picture since — hidden a language, pinned
 * the level. Forward should return them to what they were actually looking
 * at when they pressed back, not to the frame that got them there.
 */
export function stepBack(history: ViewHistory, current: ViewState): Step | null {
  const restore = history.back[history.back.length - 1];
  if (!restore) return null;
  return {
    history: {
      back: history.back.slice(0, -1),
      forward: [...history.forward, current].slice(-HISTORY_LIMIT),
    },
    restore,
  };
}

export function stepForward(history: ViewHistory, current: ViewState): Step | null {
  const restore = history.forward[history.forward.length - 1];
  if (!restore) return null;
  return {
    history: {
      back: [...history.back, current].slice(-HISTORY_LIMIT),
      forward: history.forward.slice(0, -1),
    },
    restore,
  };
}

/** What the next frame in each direction is, for the controls to name. Null
 *  where there is nothing to go to, which is also what disables the button. */
export function peekBack(history: ViewHistory): ViewState | null {
  return history.back[history.back.length - 1] ?? null;
}

export function peekForward(history: ViewHistory): ViewState | null {
  return history.forward[history.forward.length - 1] ?? null;
}

/**
 * What a button pointing at `state` should say it leads to.
 *
 * Reuses the saved-view row's subtitle, so a frame and a view describe
 * themselves the same way, and the reader who has read one can read the
 * other. `null` — nothing to go to — becomes the disabled control's reason.
 */
export function stepTitle(direction: 'Back' | 'Forward', state: ViewState | null): string {
  if (!state) {
    return direction === 'Back'
      ? 'Nothing to go back to — this is the first picture of the session'
      : 'Nothing to go forward to — press Back first';
  }
  return `${direction} to ${stateSummary(state)}`;
}
