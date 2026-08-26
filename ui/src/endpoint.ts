/**
 * Which engine this UI talks to (UI-032).
 *
 * The visualizer used to be able to reach exactly one server: the one that
 * served it. That is why `ui/dist` had to sit beside the binary, and why two
 * components with no compile-time dependency on each other shipped together.
 *
 * The mechanism to do better already existed with one caller — the VS Code
 * webview passes `window.__MEZZ_VSCODE__.apiBase` and has been a cross-origin
 * client of `mezz watch` all along. This module generalises that one hook into
 * a resolution order, so a UI opened from anywhere can be pointed at a running
 * engine.
 *
 * Resolution, first hit wins:
 *
 *   1. `window.__MEZZ_VSCODE__.apiBase` — the webview keeps priority. The
 *      extension is the primary surface; nothing here may outrank it.
 *   2. `?api=<origin>` in the query string — a session, and the demo path.
 *   3. `localStorage` — what the connect screen (UI-034) writes.
 *   4. `''` — same origin. Today's behaviour, and the default.
 *
 * Resolved once per page load. UI-034 reloads the page when the endpoint
 * changes, for the same reason `backToPicker` does: every store derived from
 * the old engine's data would otherwise need individual teardown.
 */

/** Where the active endpoint came from. Shown by the connect screen. */
export type EndpointSource = 'webview' | 'query' | 'stored' | 'same-origin';

export interface Endpoint {
  /** Origin to prefix onto API paths. `''` means same-origin. */
  base: string;
  /** Pairing token (SRV-009), or `null` when none is configured. */
  token: string | null;
  source: EndpointSource;
}

const STORAGE_BASE = 'mezz.apiBase';
const STORAGE_TOKEN = 'mezz.apiToken';

let resolved: Endpoint | null = null;

/**
 * Accept an origin and nothing else: `http://host[:port]`, no path, no
 * credentials, no other scheme.
 *
 * Anything else returns `null` and the caller falls through to the next
 * source rather than concatenating user input into a broken URL — a
 * `?api=` typo should look like "not configured", not like a server that
 * answers 404 to everything.
 */
export function normalizeOrigin(raw: string): string | null {
  const trimmed = raw.trim().replace(/\/+$/, '');
  if (!trimmed) return null;
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return null;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
  if (url.pathname !== '/' && url.pathname !== '') return null;
  if (url.search || url.hash || url.username || url.password) return null;
  return url.origin;
}

/**
 * What a human types into the connect screen: a bare port (`3200`), a
 * `host:port`, or a full origin. Returns a normalized origin or `null`.
 *
 * A bare port is the common case by a wide margin — the engine is almost
 * always on this machine, and the only thing that varies is which port the
 * user passed to `mezz watch`.
 */
export function parseEndpointInput(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  if (/^\d{1,5}$/.test(trimmed)) {
    const port = Number(trimmed);
    if (port < 1 || port > 65535) return null;
    return `http://localhost:${port}`;
  }
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) {
    return normalizeOrigin(`http://${trimmed}`);
  }
  return normalizeOrigin(trimmed);
}

/** Resolve the endpoint, memoized for the life of the page. */
export function endpoint(): Endpoint {
  if (!resolved) resolved = resolve();
  return resolved;
}

function resolve(): Endpoint {
  const webviewBase = window.__MEZZ_VSCODE__?.apiBase;
  if (webviewBase) {
    return { base: webviewBase, token: null, source: 'webview' };
  }

  const params = new URLSearchParams(window.location.search);
  const fromQuery = params.get('api');
  if (fromQuery) {
    const base = normalizeOrigin(fromQuery);
    // A malformed `?api=` is ignored rather than falling through to a stored
    // endpoint: the user asked for *this* engine, and silently connecting to
    // a different one would be worse than not connecting.
    if (base) {
      return { base, token: params.get('token'), source: 'query' };
    }
  }

  const stored = readStored();
  if (stored) return stored;

  return { base: '', token: null, source: 'same-origin' };
}

function readStored(): Endpoint | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_BASE);
    if (!raw) return null;
    const base = normalizeOrigin(raw);
    if (!base) {
      window.localStorage.removeItem(STORAGE_BASE);
      return null;
    }
    return {
      base,
      token: window.localStorage.getItem(STORAGE_TOKEN),
      source: 'stored',
    };
  } catch {
    // Private browsing, or a webview with storage disabled. Same-origin is
    // the right answer there anyway.
    return null;
  }
}

/**
 * Remember an endpoint across visits, and adopt it now.
 *
 * Called by the connect screen once a health check has passed, so what gets
 * persisted is always an endpoint that answered.
 */
export function rememberEndpoint(base: string, token: string | null): void {
  resolved = { base, token, source: 'stored' };
  try {
    window.localStorage.setItem(STORAGE_BASE, base);
    if (token) window.localStorage.setItem(STORAGE_TOKEN, token);
    else window.localStorage.removeItem(STORAGE_TOKEN);
  } catch {
    // Not persisted, but the session still works.
  }
}

/** Forget the stored endpoint. The next load falls back to same-origin. */
export function forgetEndpoint(): void {
  resolved = null;
  try {
    window.localStorage.removeItem(STORAGE_BASE);
    window.localStorage.removeItem(STORAGE_TOKEN);
  } catch {
    // Nothing was stored to begin with.
  }
}

/**
 * Add the pairing token to a URL, as a query parameter.
 *
 * The query form rather than an `Authorization` header because `EventSource`
 * (UI-033) cannot set headers at all, and the live-reload stream is not
 * optional — a UI that loads once and never updates is the feature missing.
 * Given the query form has to work anyway, using it everywhere is one code
 * path instead of two that can disagree.
 */
export function withToken(url: string, token: string | null): string {
  if (!token) return url;
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}token=${encodeURIComponent(token)}`;
}
