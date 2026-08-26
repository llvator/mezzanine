/**
 * Repo submissions and their live progress (UI-008).
 *
 * `POST /api/repos` returns a slug immediately and does the clone/analyze
 * work in the background; this store owns the resulting job cards and the
 * `EventSource` per job that keeps them current.
 *
 * Deliberately separate from `liveReload.ts`: that store is the *graph*
 * reload channel for `mezz watch` and has different reconnect semantics (it
 * retries forever). A job stream ends when its job does.
 */

import { get, writable } from 'svelte/store';
import { refreshRepos } from './serveMode';
import { flatApiUrl } from '../vscodeAdapter';

/** Pipeline states SRV-004 emits. Unknown strings are kept verbatim so a
 *  future backend state shows up without a UI change. */
export type JobStatus = 'queued' | 'cloning' | 'analyzing' | 'ready' | 'failed' | string;

export interface Submission {
  slug: string;
  url: string;
  status: JobStatus;
  error?: string;
  /** False for cards restored from a previous page load — those report
   *  progress but must not yank the user into a repo they asked for in an
   *  earlier session. */
  live: boolean;
}

export const submissions = writable<Submission[]>([]);

/** Called with a slug when a submission made *this session* becomes ready. */
let onReady: ((slug: string) => void) | null = null;

export function setOnReady(cb: (slug: string) => void): void {
  onReady = cb;
}

const STORAGE_KEY = 'mezz.submissions';
const MAX_REMEMBERED = 5;

const streams = new Map<string, EventSource>();

// ─────────────────────────────────────────────────────────────────────────────
// Client-side URL check
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Mirror of `parse_github_url` in src/server/repo.rs, for error UX only —
 * the server's copy is authoritative and re-runs on every submission.
 *
 * Strict on purpose: `git@github.com:o/r` and `github.com/orgs/foo` must be
 * rejected here so the user gets a useful message instead of a bare 400.
 */
export function parseGithubUrl(input: string): { url: string; slug: string } | { error: string } {
  const raw = input.trim();
  const PREFIX = 'https://github.com/';
  if (!raw.startsWith(PREFIX)) {
    return { error: 'Enter an https://github.com/owner/repo URL.' };
  }

  let rest = raw.slice(PREFIX.length);
  if (rest.endsWith('/')) rest = rest.slice(0, -1);
  if (rest.endsWith('.git')) rest = rest.slice(0, -4);

  const parts = rest.split('/');
  if (parts.length !== 2) {
    // Catches `owner` alone, `owner/repo/tree/main`, and `orgs/foo`-shaped
    // paths that aren't a repository.
    return { error: 'That URL points at a page, not a repository.' };
  }

  const [owner, repo] = parts;
  const NAME = /^[A-Za-z0-9._-]+$/;
  for (const [label, part] of [['owner', owner], ['repository', repo]] as const) {
    if (!part) return { error: `The URL has an empty ${label}.` };
    if (part === '.' || part === '..') return { error: `Invalid ${label} "${part}".` };
    if (!NAME.test(part)) return { error: `Invalid ${label} "${part}".` };
  }

  const slug = `${owner}__${repo}`;
  if (slug.includes('..') || slug.startsWith('-') || slug.startsWith('.')) {
    return { error: 'That repository name cannot be used.' };
  }
  return { url: `${PREFIX}${owner}/${repo}`, slug };
}

// ─────────────────────────────────────────────────────────────────────────────
// Submission
// ─────────────────────────────────────────────────────────────────────────────

export type SubmitResult =
  /** Rejected before or by the server; nothing is running. */
  | { kind: 'error'; message: string }
  /** The server already had this repo analyzed — open it. */
  | { kind: 'ready'; slug: string }
  /** A job is running and a progress card is now following it. */
  | { kind: 'tracking'; slug: string };

/** Submit `input` and, unless it was rejected, start following the job. */
export async function submitRepo(input: string): Promise<SubmitResult> {
  const parsed = parseGithubUrl(input);
  if ('error' in parsed) return { kind: 'error', message: parsed.error };

  let resp: Response;
  try {
    resp = await fetch(flatApiUrl('/api/repos'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: parsed.url }),
    });
  } catch {
    return { kind: 'error', message: 'Could not reach the server.' };
  }

  if (!resp.ok && resp.status !== 202) {
    const detail = await resp.text().catch(() => '');
    return {
      kind: 'error',
      message: detail || `Submission failed (HTTP ${resp.status}).`,
    };
  }

  const body = await resp.json().catch(() => null);
  // Trust the server's slug over the locally derived one — its validator is
  // the authoritative parse.
  const slug: string = body?.slug ?? parsed.slug;

  // 200 means the server already has it analyzed; there's nothing to watch.
  if (resp.status === 200 && body?.status === 'ready') {
    await refreshRepos();
    return { kind: 'ready', slug };
  }

  track({ slug, url: parsed.url, status: body?.status ?? 'queued', live: true });
  remember(slug, parsed.url);
  subscribe(slug);
  return { kind: 'tracking', slug };
}

/** Add or replace a submission card. */
function track(sub: Submission): void {
  submissions.update((list) => [sub, ...list.filter((s) => s.slug !== sub.slug)]);
}

function update(slug: string, patch: Partial<Submission>): void {
  submissions.update((list) =>
    list.map((s) => (s.slug === slug ? { ...s, ...patch } : s)),
  );
}

/** Drop a card. The job itself is unaffected — it's server-side. */
export function dismiss(slug: string): void {
  closeStream(slug);
  submissions.update((list) => list.filter((s) => s.slug !== slug));
  forget(slug);
}

// ─────────────────────────────────────────────────────────────────────────────
// Progress stream
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Watch one job. SRV-004 sends the *current* status as the first frame, so
 * a card is correct even if it subscribes after a transition.
 */
function subscribe(slug: string): void {
  closeStream(slug);
  const source = new EventSource(flatApiUrl(`/api/repos/${slug}/events`));
  streams.set(slug, source);

  source.addEventListener('status', (e) => {
    let payload: { status?: string; error?: string } | null = null;
    try {
      payload = JSON.parse((e as MessageEvent).data);
    } catch {
      return;
    }
    if (!payload?.status) return;

    update(slug, { status: payload.status, error: payload.error });

    if (payload.status === 'ready') {
      closeStream(slug);
      void finish(slug);
    } else if (payload.status === 'failed') {
      // Card stays visible with the reason so the user can retry.
      closeStream(slug);
      forget(slug);
    }
  });

  source.onerror = () => {
    // The server closes the stream on shutdown, and a rehydrated card may
    // name a slug this server never had. Either way there's nothing to
    // reconnect to — report it once rather than retrying forever.
    const current = get(submissions).find((s) => s.slug === slug);
    if (current && current.status !== 'ready' && current.status !== 'failed') {
      update(slug, { status: 'failed', error: 'Lost connection to the server.' });
    }
    closeStream(slug);
  };
}

/** A job finished: make the repo selectable, then hand it to the page. */
async function finish(slug: string): Promise<void> {
  await refreshRepos();
  forget(slug);
  const sub = get(submissions).find((s) => s.slug === slug);
  if (sub?.live) onReady?.(slug);
}

function closeStream(slug: string): void {
  streams.get(slug)?.close();
  streams.delete(slug);
}

/** Close every stream. Called when the picker unmounts. */
export function closeAllStreams(): void {
  for (const slug of [...streams.keys()]) closeStream(slug);
}

// ─────────────────────────────────────────────────────────────────────────────
// localStorage — "I asked for this" survives a reload
// ─────────────────────────────────────────────────────────────────────────────

function readStored(): { slug: string; url: string }[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed)
      ? parsed.filter((e) => typeof e?.slug === 'string' && typeof e?.url === 'string')
      : [];
  } catch {
    return [];
  }
}

function writeStored(entries: { slug: string; url: string }[]): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(entries.slice(0, MAX_REMEMBERED)));
  } catch {
    // Private browsing or a full quota — remembering is a nicety, not a
    // requirement, and the job runs server-side regardless.
  }
}

function remember(slug: string, url: string): void {
  writeStored([{ slug, url }, ...readStored().filter((e) => e.slug !== slug)]);
}

function forget(slug: string): void {
  writeStored(readStored().filter((e) => e.slug !== slug));
}

/**
 * Restore cards for jobs that were still running when the page reloaded and
 * subscribe fresh. Rehydrated cards never auto-navigate — the user didn't
 * ask for that *now*.
 */
export function rehydrateSubmissions(): void {
  for (const { slug, url } of readStored()) {
    track({ slug, url, status: 'queued', live: false });
    subscribe(slug);
  }
}
