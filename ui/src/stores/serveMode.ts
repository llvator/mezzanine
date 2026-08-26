/**
 * Serve-mode detection and repo selection (UI-007).
 *
 * `mezz serve` hosts several analyzed repos at once under
 * `/api/repos/{slug}/*`, so the app has to pick one before it can load a
 * graph. `mezz watch` and the VS Code webview have exactly one implicit repo
 * and never enter this mode.
 *
 * Detection is a single `GET /api/repos` at boot: a 200 with an array means
 * serve mode. Watch mode has no such route, so it 404s and we fall through
 * to today's behaviour unchanged.
 */

import { get, writable } from 'svelte/store';
import { flatApiUrl, isVscode, setServeRepo } from '../vscodeAdapter';

/** One entry of `GET /api/repos`. Mirrors `RepoSummary` in src/server/repo.rs. */
export interface RepoSummary {
  slug: string;
  url: string | null;
  sha: string | null;
  /** Unix epoch seconds. */
  ready_at: number;
  status: string;
  entity_count: number;
  relationship_count: number;
}

/** True when the backend is `mezz serve`. Never true inside VS Code. */
export const serveMode = writable<boolean>(false);

/** Repos the server currently has loaded. Empty outside serve mode. */
export const availableRepos = writable<RepoSummary[]>([]);

/** The repo being viewed, or `null` while the picker is up. */
export const activeRepo = writable<RepoSummary | null>(null);

/** Synchronous read for guards in plain async functions, which can't
 *  subscribe. Used by callers of endpoints serve mode doesn't have. */
export function isServeMode(): boolean {
  return get(serveMode);
}

/**
 * Adopt serve mode and load the repo list.
 *
 * Which mode the backend is in used to be discovered here, by probing
 * `GET /api/repos` and reading a 404 as "not serve mode". UI-034's
 * `/api/hello` handshake now answers that question directly, which matters
 * once the engine can be on another origin: cross-origin, a 404 and a
 * refused origin are the same rejected `fetch`, so the old probe could not
 * tell "watch mode" from "this engine won't talk to you".
 *
 * Deliberately still not routed through `apiUrl()` — `/api/repos` is the one
 * endpoint outside the per-repo namespace it would prefix.
 */
export async function enterServeMode(): Promise<void> {
  if (isVscode()) return;
  serveMode.set(true);
  await refreshRepos();
}

/** Re-fetch the repo list. Used by the picker's refresh affordance and,
 *  from UI-008, after a submission completes. */
export async function refreshRepos(): Promise<void> {
  try {
    const resp = await fetch(flatApiUrl('/api/repos'), { cache: 'no-store' });
    if (!resp.ok) return;
    const data = await resp.json();
    if (Array.isArray(data)) availableRepos.set(data as RepoSummary[]);
  } catch {
    // Leave the last known list up rather than blanking the picker.
  }
}

/**
 * Point the app at `slug`. Returns false when the slug isn't one the
 * server has, so a stale deep link falls back to the picker instead of
 * firing a page of 404s.
 */
export function selectRepo(slug: string): boolean {
  const repo = get(availableRepos).find((r) => r.slug === slug);
  if (!repo) return false;
  setServeRepo(slug);
  activeRepo.set(repo);
  if (slugFromHash() !== slug) window.location.hash = `#/repo/${slug}`;
  return true;
}

/** `#/repo/{slug}` → `slug`. `null` for any other hash.
 *
 *  The dot is part of the charset because dotted repo names (`three.js`,
 *  `next.js`) are ordinary on GitHub — see `is_valid_slug` in
 *  src/server/repo.rs, which this mirrors. `..` is excluded so a crafted
 *  hash can't produce a slug the server would reject anyway. */
export function slugFromHash(): string | null {
  const m = /^#\/repo\/([A-Za-z0-9_-][A-Za-z0-9._-]*)$/.exec(window.location.hash);
  if (!m || m[1].includes('..')) return null;
  return m[1];
}

/**
 * Return to the picker.
 *
 * Reloads the page rather than resetting stores by hand. Every repo-derived
 * store — the index, the details cache, the memoized full-graph promise, the
 * filter sets seeded from the previous graph's entity kinds — would otherwise
 * need individual teardown, and one missed reset shows up as another repo's
 * data bleeding into the view. The picker is a landing page; a reload there
 * costs nothing.
 */
export function backToPicker(): void {
  window.location.hash = '#/';
  window.location.reload();
}
