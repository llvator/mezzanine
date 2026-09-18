/**
 * Folding what the engine says into what the page shows (UI-138).
 *
 * The engine reports through two channels that answer different questions,
 * and the whole difficulty is that the answers overlap:
 *
 * - the `activity` SSE event, one [`Update`] per thing said, live; and
 * - `GET /api/activity?since=<seq>`, a [`Snapshot`] of the bounded ring the
 *   engine keeps, for a page that opened *during* a run or reconnected after
 *   a drop and so has a gap to fill.
 *
 * A reconnect uses both at once — the stream comes back and the catch-up
 * fetch is in flight — so the fold has to be idempotent under replay. `seq`
 * is what makes it so: it is assigned by the engine, monotonic, and the only
 * thing here that decides whether a notice is new.
 *
 * Nothing in this file works out *whether the engine is busy*. That rule
 * — which notice opens a run, which closes it, what happens when a watcher
 * re-analysis overlaps a diff mid-flight — lives in `src/activity.rs`, and
 * every frame from either channel carries its result. A second copy of it
 * here would be a second thing to keep correct, and it would be the copy
 * that was wrong, because it only ever sees the frames that arrived.
 */

export type ActivityKind = 'start' | 'step' | 'warn' | 'end';

/** One thing the engine said. Mirrors `activity::Notice`. */
export interface Notice {
  seq: number;
  /** Unix epoch milliseconds. */
  at_ms: number;
  kind: ActivityKind;
  /** `analysis` or `diff` — which pipeline said it. */
  phase: string;
  message: string;
}

/** The run in flight, if one is. Mirrors `activity::Running`. */
export interface Running {
  phase: string;
  /** The latest step, or the opening line until a step replaces it. */
  message: string;
  since_ms: number;
}

/** One SSE frame. Mirrors `activity::Update`. */
export interface Update {
  notice: Notice;
  running: Running | null;
}

/** `GET /api/activity`. Mirrors `activity::Snapshot`. */
export interface Snapshot {
  seq: number;
  running: Running | null;
  notices: Notice[];
}

/** Everything the page knows about the engine's activity. */
export interface ActivityState {
  /** The run in flight, or `null` when the engine is idle. */
  running: Running | null;
  /** Retained notices, oldest first. */
  notices: Notice[];
  /**
   * The highest `seq` accounted for — what the next catch-up fetch asks
   * *since*. Not the same as the last notice's `seq`: the ring drops what it
   * has no room for, and a page that asked again for notices the engine has
   * already forgotten would get the same nothing forever.
   */
  seq: number;
}

/**
 * How many notices to keep on this side.
 *
 * Smaller than the engine's ring on purpose. The ring exists so a
 * reconnecting page can fill a gap; this exists so a reader can scroll back
 * through what just happened. The second needs less than the first.
 */
export const FEED_LIMIT = 120;

export const EMPTY_ACTIVITY: ActivityState = { running: null, notices: [], seq: 0 };

/**
 * Fold one live frame in.
 *
 * A frame at or below `seq` is dropped rather than trusted. Two things
 * produce them: the broadcast replay a reconnect can race against the
 * catch-up fetch, and a duplicate delivery. Replaying a `start` the snapshot
 * had already closed would leave the status line claiming a finished run is
 * still going — the one failure this whole feature exists to prevent.
 */
export function applyUpdate(state: ActivityState, update: Update | null): ActivityState {
  const notice = update?.notice;
  if (!notice || typeof notice.seq !== 'number' || notice.seq <= state.seq) return state;
  return {
    running: update.running ?? null,
    notices: trim([...state.notices, notice]),
    seq: notice.seq,
  };
}

/**
 * Adopt a snapshot's view of the world.
 *
 * `running` is taken wholesale rather than merged: it is the engine's own
 * answer to the question this module exists for, and it is newer than
 * anything assembled from frames.
 *
 * `seq` only ever moves forward. A snapshot that raced a live frame and lost
 * must not walk it back, or the next fetch would re-deliver notices already
 * on screen.
 */
export function applySnapshot(state: ActivityState, snap: Snapshot | null): ActivityState {
  if (!snap || typeof snap !== 'object') return state;
  const fresh = (snap.notices ?? []).filter((n) => n && n.seq > state.seq);
  return {
    running: snap.running ?? null,
    notices: fresh.length > 0 ? trim([...state.notices, ...fresh]) : state.notices,
    seq: Math.max(state.seq, snap.seq ?? 0, ...fresh.map((n) => n.seq)),
  };
}

/** Warnings among the notices still retained — the badge on the chip. */
export function warningCount(state: ActivityState): number {
  return state.notices.filter((n) => n.kind === 'warn').length;
}

/**
 * Newest first, for a panel that opens on what just happened rather than on
 * what has scrolled furthest away.
 */
export function newestFirst(state: ActivityState): Notice[] {
  return [...state.notices].reverse();
}

/**
 * Engine messages are formatted for a terminal, where the leading spaces
 * carry the nesting. In a chip or a row they are just a gap.
 */
export function clean(message: string): string {
  return (message ?? '').trim();
}

/** `42s`, then `1m 06s`. Seconds stay two-digit past the minute so the
 *  number does not jump width as it ticks. */
export function elapsed(sinceMs: number, nowMs: number): string {
  const secs = Math.max(0, Math.round((nowMs - sinceMs) / 1000));
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, '0')}s`;
}

function trim(notices: Notice[]): Notice[] {
  return notices.length > FEED_LIMIT ? notices.slice(notices.length - FEED_LIMIT) : notices;
}
