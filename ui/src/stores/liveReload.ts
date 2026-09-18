/**
 * Live-reload listener: connects to the `mezz watch` SSE endpoint and
 * triggers a data refresh whenever the backend signals that files changed.
 *
 * Once the endpoint can point at another origin (UI-032), this is the one
 * code path that is not a `fetch`, and `EventSource` differs from `fetch` in
 * ways that matter here: it cannot carry an `Authorization` header, so the
 * pairing token has to travel in the query string; and its failures arrive
 * as a bare `onerror` with no status, so "the origin was refused" and "the
 * server is not up yet" are indistinguishable at the point of failure.
 *
 * That second difference is why this store diagnoses instead of just
 * retrying (UI-033). A UI pointed at a refused origin used to re-request a
 * rejection every five seconds forever while showing "Static", which reads
 * as "nothing has changed" rather than "this is not connected".
 */

import { get, writable } from 'svelte/store';
import { refreshData } from './scope';
import { loadDiff } from './diff';
import { fetchBranch } from './branch';
import { refreshShape } from './shape';
import { apiUrl, isVscode } from '../vscodeAdapter';
import { endpoint } from '../endpoint';
import { probe } from './connection';
import { isServeMode } from './serveMode';
import { fetchActivity, ingestUpdate, resetActivity, type Update } from './activity';

/** True while a live-reload refresh is in progress. */
export const liveReloading = writable(false);

/** True when the SSE connection is active. */
export const liveConnected = writable(false);

/**
 * Why the stream is not running, when it isn't.
 *
 * - `off` — serve mode, which has no `/events` and never will.
 * - `refused` — the engine would not answer this origin. Needs
 *   `--allow-origin` on the command the user already ran.
 * - `token` — the engine wants the pairing token from its startup banner.
 * - `unreachable` — nothing answered, after the retries were spent.
 * - `no-stream` — the API answers but the stream does not. A proxy that
 *   buffers SSE looks like this.
 */
export type LiveStopReason = 'off' | 'refused' | 'token' | 'unreachable' | 'no-stream';

export type LiveStatus =
  | { kind: 'connecting' }
  | { kind: 'live' }
  | { kind: 'retrying'; attempt: number }
  | { kind: 'stopped'; reason: LiveStopReason };

export const liveStatus = writable<LiveStatus>({ kind: 'connecting' });

/**
 * Backoff schedule, in milliseconds. Escalating rather than the flat five
 * seconds it replaces: a server that is coming back comes back quickly, and
 * one that isn't should not be asked twelve times a minute indefinitely.
 * Running off the end of this list is what "gave up" means.
 */
const RETRY_DELAYS = [1_000, 2_000, 5_000, 10_000, 30_000];

let eventSource: EventSource | null = null;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
let attempt = 0;

/** Try to connect to the watch server's SSE endpoint.
 *  Falls back silently if the endpoint isn't available. */
export function connectLiveReload(url?: string): void {
  // `mezz serve` has no `/events` and never will — its repos are analyzed
  // once, not watched. Without this guard the retry loop below would
  // re-request a 404 every few seconds for the life of the page.
  if (isServeMode()) {
    liveStatus.set({ kind: 'stopped', reason: 'off' });
    return;
  }
  // `apiUrl` supplies both the configured origin and the pairing token, so
  // a cross-origin stream needs nothing extra here.
  url = url ?? apiUrl('/events');
  if (eventSource) return; // already connected

  if (attempt === 0) liveStatus.set({ kind: 'connecting' });

  try {
    eventSource = new EventSource(url);

    eventSource.addEventListener('connected', () => {
      console.log('[liveReload] connected to watch server');
      attempt = 0;
      liveConnected.set(true);
      liveStatus.set({ kind: 'live' });
      // Catch up on what happened before this stream existed. A page opened
      // mid-analysis, or one that has just reconnected, has missed the
      // notices that would have told it the engine is busy — and "busy" is
      // exactly the state it most needs to show (UI-138).
      void fetchActivity();
    });

    // The one event that carries its payload rather than telling us to come
    // and get it: it is sent *during* work, and a round trip per status line
    // would be a request every few hundred milliseconds for the length of an
    // analysis.
    eventSource.addEventListener('activity', onActivityFrame);

    eventSource.addEventListener('reload', async () => {
      console.log('[liveReload] reload signal received — refreshing data');
      liveReloading.set(true);
      try {
        await refreshData();
        // The shape picture is a separate fetch, so it does not come along
        // with the graph — and a stale one is worse than no picture at all
        // here: the whole view is a claim about edges that may have just
        // been the ones edited. Silent when the view was never opened.
        await refreshShape();
      } finally {
        liveReloading.set(false);
      }
    });

    // A `→ working` diff is recomputed by the engine after each
    // re-analysis and announced separately, because only the overlay moved
    // (UI-067). Re-fetching the graph here as well would restart a canvas
    // that has no reason to move, and the `reload` above has already done it.
    eventSource.addEventListener('diff', async () => {
      console.log('[liveReload] diff signal received — reloading the overlay');
      await loadDiff({ baseDetails: false });
    });

    // HEAD moved — a checkout, a new branch, a commit — and nothing else
    // did. Re-fetching the graph here would restart a canvas whose code has
    // not changed; the only thing that went stale is the label saying which
    // branch that canvas is (UI-114).
    eventSource.addEventListener('head', () => {
      console.log('[liveReload] head signal received — re-reading the branch');
      void fetchBranch();
    });

    eventSource.onerror = () => {
      // Connection lost or never established. `EventSource` gives no status
      // here, so ask the JSON API what is actually wrong.
      disconnectLiveReload();
      void handleFailure(url as string);
    };
  } catch {
    void handleFailure(url);
  }
}

/**
 * One activity frame off the stream.
 *
 * A named handler rather than an inline closure so `connectLiveReload` keeps
 * the shape it had — it is a list of listener registrations, and a `try`
 * block inside one of them made it a list with a branch in it.
 */
function onActivityFrame(e: Event): void {
  try {
    ingestUpdate(JSON.parse((e as MessageEvent).data) as Update);
  } catch {
    // A frame we cannot read is not worth dropping the stream over.
  }
}

export function disconnectLiveReload(): void {
  liveConnected.set(false);
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
}

/** Stop trying, and stay stopped. Used by the manual disconnect toggle. */
export function stopLiveReload(reason: LiveStopReason = 'off'): void {
  disconnectLiveReload();
  if (retryTimer) {
    clearTimeout(retryTimer);
    retryTimer = null;
  }
  attempt = 0;
  liveStatus.set({ kind: 'stopped', reason });
  // Only here, not on the transient disconnects a retry recovers from: a run
  // that was in flight when the stream blinked is very likely still in
  // flight, and the reconnect re-asks anyway. Having given up, though, we no
  // longer have any grounds for the claim — leaving "Analyzing…" on screen
  // would make a dead connection look like a busy engine.
  resetActivity();
}

/** Start over after a give-up, from a user action. */
export function reconnectLiveReload(): void {
  attempt = 0;
  connectLiveReload();
}

/** True when the stream is neither live nor deliberately off. */
export function liveIsBroken(status: LiveStatus): boolean {
  return status.kind === 'stopped' && status.reason !== 'off';
}

/**
 * Decide whether to retry, and say why not when the answer is no.
 *
 * A refusal and a missing token are settled facts — the engine has to be
 * restarted, or the user has to paste something — so retrying them is pure
 * noise. Everything else gets the backoff.
 */
async function handleFailure(url: string): Promise<void> {
  const terminal = await diagnose();
  if (terminal) {
    stopLiveReload(terminal);
    return;
  }
  if (attempt >= RETRY_DELAYS.length) {
    stopLiveReload('unreachable');
    return;
  }
  const delay = RETRY_DELAYS[attempt];
  attempt += 1;
  liveStatus.set({ kind: 'retrying', attempt });
  scheduleRetry(url, delay);
}

/**
 * Ask the JSON API what the stream could not say.
 *
 * Returns a terminal reason, or `null` to mean "try again".
 *
 * Runs the same handshake the connect screen does, rather than a bare
 * `fetch` of `/api/root`: a rejected cross-origin request is a CORS refusal
 * *or* a dead port, the browser will not say which, and reporting the wrong
 * one sends the user to fix a flag when the engine simply is not running.
 * `/api/hello` answers every origin, so "it is there and it blocked you" is
 * a fact rather than a guess.
 */
async function diagnose(): Promise<LiveStopReason | null> {
  const crossOrigin = !isVscode() && endpoint().base !== '';
  const { base, token } = endpoint();
  const verdict = await probe(base, token);
  switch (verdict.kind) {
    case 'refused':
      return 'refused';
    case 'token-required':
      return 'token';
    case 'ok':
      // The API answers but the stream did not. Give the retries a chance
      // first — a restarting engine can be up on one and not the other for
      // a moment — and only call it terminal once they are spent.
      return attempt >= RETRY_DELAYS.length ? 'no-stream' : null;
    default:
      // Same-origin, an unreachable server is very often one that has not
      // started yet: opening the UI before running `mezz watch` is a real
      // workflow, and it is what the retry loop is for.
      return crossOrigin ? 'unreachable' : null;
  }
}

function scheduleRetry(url: string, delay: number): void {
  if (retryTimer) return;
  retryTimer = setTimeout(() => {
    retryTimer = null;
    connectLiveReload(url);
  }, delay);
}
