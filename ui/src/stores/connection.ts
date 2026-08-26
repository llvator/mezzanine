/**
 * Deciding which engine this UI talks to, and proving it answers (UI-034).
 *
 * `endpoint.ts` resolves *what* the endpoint is. This module answers whether
 * it works, and — when it doesn't — which of the failures it is, because they
 * have completely different remedies and a browser cannot tell them apart on
 * its own.
 *
 * From a page, "the engine refused this origin", "that port is a
 * `python3 -m http.server`" and "nothing is listening" are all the same
 * rejected `fetch` with no status and no message. Two things separate them:
 *
 *   - `GET /api/hello` answers every origin unconditionally (see
 *     `src/server/access.rs`), so reading it proves the thing on that port is
 *     mezz and not something else on the same port.
 *   - a `mode: 'no-cors'` request resolves opaquely when *something* is
 *     listening and rejects when nothing is, which separates "not mezz" from
 *     "not there".
 */

import { get, writable } from 'svelte/store';
import { endpoint, rememberEndpoint, type Endpoint } from '../endpoint';

/** Which route table the backend has. Mirrors `/api/hello`'s `mode`. */
export type EngineMode = 'watch' | 'serve';

export type Connection =
  /** Reachable and readable. The app can boot. */
  | {
      kind: 'ok'; mode: EngineMode; tokenRequired: boolean; agentSpawn: boolean;
      /** Crate version and short commit, for the build stamp. Both are
       *  optional: an engine built before `/api/hello` carried them answers
       *  the probe perfectly well and must keep booting the app. */
      version?: string; commit?: string;
    }
  /** mezz, reachable, but it will not answer *this* page. */
  | { kind: 'refused' }
  /** mezz, reachable, and it wants the pairing token from its banner. */
  | { kind: 'token-required' }
  /** Something is listening there. It is not mezz. */
  | { kind: 'not-mezz' }
  /** Nothing answered at all. */
  | { kind: 'unreachable' };

/** The current connection verdict, for the screen and the header chip. */
export const connection = writable<Connection | null>(null);

/** `true` while a probe is in flight, so the connect button can say so. */
export const probing = writable(false);

/** Health-check `base`, returning what a user would need to do about it. */
export async function probe(base: string, token: string | null): Promise<Connection> {
  const hello = await sayHello(base);
  if (hello.kind === 'absent') return { kind: 'unreachable' };
  if (hello.kind === 'stranger') return { kind: 'not-mezz' };

  // It is mezz, and it is up. Whether *we* may read it is a separate
  // question, and the endpoint that answers it is one the allowlist and the
  // token gate both apply to.
  const path = hello.mode === 'serve' ? '/api/repos' : '/api/root';
  const url = token
    ? `${base}${path}?token=${encodeURIComponent(token)}`
    : `${base}${path}`;
  try {
    const resp = await fetch(url, { cache: 'no-store' });
    if (resp.status === 401) return { kind: 'token-required' };
    if (!resp.ok) return { kind: 'not-mezz' };
    return {
      kind: 'ok', mode: hello.mode, tokenRequired: hello.tokenRequired,
      agentSpawn: hello.agentSpawn, version: hello.version, commit: hello.commit,
    };
  } catch {
    // `/api/hello` came back, so the server is there and is mezz. The only
    // thing that can block this one is the origin allowlist.
    return { kind: 'refused' };
  }
}

type Hello =
  | {
      kind: 'mezz'; mode: EngineMode; tokenRequired: boolean; agentSpawn: boolean;
      version?: string; commit?: string;
    }
  | { kind: 'stranger' }
  | { kind: 'absent' };

async function sayHello(base: string): Promise<Hello> {
  try {
    const resp = await fetch(`${base}/api/hello`, { cache: 'no-store' });
    if (!resp.ok) return { kind: 'stranger' };
    const body = await resp.json();
    if (body?.server !== 'mezz') return { kind: 'stranger' };
    return {
      kind: 'mezz',
      mode: body.mode === 'serve' ? 'serve' : 'watch',
      tokenRequired: !!body.token_required,
      // Advertised rather than probed: with the flag off the route is not
      // registered, and the static-UI fallback answers the path with 405
      // rather than 404 — so a probe cannot tell "absent" from "wrong method".
      agentSpawn: !!body.agent_spawn,
      version: typeof body.version === 'string' ? body.version : undefined,
      commit: typeof body.commit === 'string' ? body.commit : undefined,
    };
  } catch {
    // Blocked or unreachable — the browser will not say which. An opaque
    // request has no CORS requirement to fail, so it resolves whenever
    // anything is listening.
    try {
      await fetch(`${base}/api/hello`, { mode: 'no-cors', cache: 'no-store' });
      return { kind: 'stranger' };
    } catch {
      return { kind: 'absent' };
    }
  }
}

/**
 * Probe the endpoint the page resolved to, and publish the verdict.
 *
 * Run once at boot, before anything tries to load data — the connect screen
 * exists precisely so that a stranger sees a question rather than an empty
 * graph and a console error.
 */
export async function checkCurrentEndpoint(): Promise<Connection> {
  const { base, token } = endpoint();
  probing.set(true);
  try {
    const result = await probe(base, token);
    connection.set(result);
    return result;
  } finally {
    probing.set(false);
  }
}

/**
 * Try `base`, and adopt it only if it answered.
 *
 * Persisting an endpoint that was never verified is how a bad value becomes
 * permanent: it survives the reload, fails again, and the user has no way to
 * tell the stored value from the typed one.
 */
export async function connectTo(
  base: string,
  token: string | null
): Promise<Connection> {
  probing.set(true);
  try {
    const result = await probe(base, token);
    connection.set(result);
    if (result.kind === 'ok') rememberEndpoint(base, token);
    return result;
  } finally {
    probing.set(false);
  }
}

/** The endpoint currently in use, for the header chip. */
export function currentEndpoint(): Endpoint {
  return endpoint();
}

/** True when the verdict is one the connect screen should be showing. */
export function needsConnectScreen(c: Connection | null): boolean {
  return c !== null && c.kind !== 'ok';
}

/** Whether a probe is running right now, without subscribing. */
export function isProbing(): boolean {
  return get(probing);
}
