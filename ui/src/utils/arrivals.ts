/**
 * Which nodes just arrived on the canvas, and for how long that still counts
 * (UI-066).
 *
 * A live-reload update fades a new node in over 400ms and then it is
 * indistinguishable from the several hundred nodes it landed among. That
 * fade is a *transition*, not a *marker*: it says "something moved" for as
 * long as it takes the eye to get there, which on a busy canvas is not long
 * enough to answer "what moved". Watching a repo re-analyze while you edit
 * it is the workflow where that answer is the whole point.
 *
 * So arrivals are remembered rather than animated-and-forgotten. This module
 * owns the bookkeeping — ids and deadlines, no DOM, no clock of its own — so
 * the window's behaviour is testable without a browser and the View is left
 * with just "paint these ids".
 *
 * The clock is a parameter for the same reason: a test that has to sleep
 * 30 seconds to check a 30-second window will be deleted by the third person
 * who runs the suite.
 */

/** How long a node stays marked as new. Long enough to finish reading the
 *  canvas and look back at it; short enough that the marks are gone by the
 *  next edit. */
export const ARRIVAL_HIGHLIGHT_MS = 30_000;

/**
 * Above this many at once, nothing is highlighted.
 *
 * The mark means "these are the new ones". Two hundred of them means the
 * graph was replaced, not added to — every node is new, so the mark
 * distinguishes nothing and costs a canvas full of animation. Widening a
 * scope can clear this in one step, which is exactly the case that should
 * stay quiet.
 */
export const ARRIVAL_BURST_LIMIT = 60;

/** Node id → the timestamp at which its mark expires. */
export type ArrivalLog = Map<string, number>;

/** Ids present in `current` that were not in `previous`. */
export function newcomers(previous: ReadonlySet<string>, current: Iterable<string>): string[] {
  const out: string[] = [];
  for (const id of current) if (!previous.has(id)) out.push(id);
  return out;
}

/**
 * Whether a batch of newcomers is worth marking.
 *
 * An empty batch has nothing to say and a burst says too much; both leave
 * the log untouched.
 */
export function worthMarking(count: number, limit = ARRIVAL_BURST_LIMIT): boolean {
  return count > 0 && count <= limit;
}

/**
 * Record arrivals, each expiring `windowMs` from `now`.
 *
 * Re-arriving refreshes the deadline: a node that leaves and comes back has
 * just arrived again, whatever the log used to think.
 */
export function noteArrivals(
  log: ArrivalLog,
  ids: Iterable<string>,
  now: number,
  windowMs: number = ARRIVAL_HIGHLIGHT_MS,
): ArrivalLog {
  for (const id of ids) log.set(id, now + windowMs);
  return log;
}

/** Drop expired marks. Expiry is inclusive — at the deadline the mark is
 *  over, which is what "30 seconds" means to the person watching. */
export function pruneArrivals(log: ArrivalLog, now: number): ArrivalLog {
  for (const [id, deadline] of log) if (deadline <= now) log.delete(id);
  return log;
}

/**
 * Milliseconds until the next mark expires, or null when none will.
 *
 * The View schedules one timer off this rather than one per node: a hundred
 * arrivals in a burst is a hundred timers that all do the same sweep.
 */
export function msUntilNextExpiry(log: ArrivalLog, now: number): number | null {
  let soonest: number | null = null;
  for (const deadline of log.values()) {
    if (soonest === null || deadline < soonest) soonest = deadline;
  }
  return soonest === null ? null : Math.max(0, soonest - now);
}
