/**
 * A store that lags another one, so expensive derivations downstream run on
 * pauses rather than on keystrokes.
 *
 * The scope search box binds straight to its input, so every character used
 * to re-run the whole match-and-project chain synchronously. That is the
 * wrong shape twice over: the intermediate values are never read by anyone
 * (nobody wants results for `gra` on the way to `graph`), and the early
 * prefixes are the *expensive* ones — a one-letter query matches the entire
 * repo. Trailing the input turns a per-character cost into a per-pause one.
 *
 * Two things this has that a bare `setTimeout` wrapper doesn't:
 *
 *   - `immediateWhen`, for values that must not wait. Clearing the box is the
 *     case that matters: making Escape feel laggy to save work that costs
 *     nothing (an empty query matches nothing) would be a bad trade.
 *   - `flush`, for when something needs the current value *now*. Committing a
 *     query on Enter reads the match set, and a debounce with no escape hatch
 *     would scope to whatever the user had typed a moment earlier.
 *
 * A leaf module: `svelte/store` only, no project imports. Trailing stores are
 * module-level singletons that live as long as the app, so this deliberately
 * never unsubscribes from its source.
 */

import { writable } from 'svelte/store';
import type { Readable } from 'svelte/store';

export interface Trailing<T> extends Readable<T> {
  /** Publish the source's current value now, cancelling any pending wait.
   *  A no-op when nothing is pending. */
  flush(): void;
}

export function trailing<T>(
  source: Readable<T>,
  ms: number,
  immediateWhen?: (value: T) => boolean,
): Trailing<T> {
  const out = writable<T>(undefined as T);

  let published!: T;
  let pending!: T;
  let primed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const cancel = () => {
    if (timer === undefined) return;
    clearTimeout(timer);
    timer = undefined;
  };

  const publish = (value: T) => {
    cancel();
    published = value;
    out.set(value);
  };

  source.subscribe((value) => {
    pending = value;

    // Svelte calls this synchronously on subscribe, which is where the
    // initial value comes from — the store must not sit on `undefined` for
    // the first `ms` of its life.
    if (!primed) {
      primed = true;
      publish(value);
      return;
    }

    // Back to what is already out there — typing a character and deleting it
    // should leave no pending work behind.
    if (Object.is(value, published)) {
      cancel();
      return;
    }

    if (immediateWhen?.(value)) {
      publish(value);
      return;
    }

    cancel();
    timer = setTimeout(() => {
      timer = undefined;
      publish(value);
    }, ms);
  });

  return {
    subscribe: out.subscribe,
    flush() {
      if (Object.is(pending, published)) cancel();
      else publish(pending);
    },
  };
}
