/**
 * What mezz has cached on this machine, and removing part of it (UI-153).
 *
 * Unlike everything else the panel shows, this is **not about the repository
 * being watched**. The parse store is one directory shared by every repo on
 * the machine, so the rows here can name checkouts this window has never
 * opened, and clearing one of them affects a different project. The panel
 * says so; a reader who thinks this is repo-scoped will clear the wrong row.
 *
 * Wire format: `GET /api/cache`, `POST /api/cache/clear`. A 404 means either
 * a server too old to answer or one that resolved no cache directory —
 * neither is an error worth showing, so the section simply stays away.
 *
 * Fetched on demand rather than polled: the report is a stat-only walk, and
 * a six-figure cache takes a second or so to count.
 */

import { writable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';

/** Size, count and last use of some set of cached files. */
export interface Usage {
  bytes: number;
  entries: number;
  /** Unix seconds, or null for an empty set. */
  last_used: number | null;
}

export interface RepoUsage extends Usage {
  /** The bucket a clear request names. Null for the pseudo-row holding a
   *  pre-UI-153 generation's flat entries, which only a generation-wide
   *  clear can reach. */
  id: string | null;
  label: string;
  common_dir: string | null;
}

export interface Generation extends Usage {
  tag: string;
  /** The generation in force. The rest are waiting out the grace window that
   *  keeps a still-installed older binary warm, and are what "Reclaim"
   *  targets. */
  current: boolean;
  repos: RepoUsage[];
}

export interface CacheReport {
  root: string;
  bytes: number;
  generations: Generation[];
  reshape: Usage;
}

/** What a clear may name. Mirrors `cache_report::ClearTarget`. */
export type ClearTarget =
  | { scope: 'everything' }
  | { scope: 'abandoned' }
  | { scope: 'reshape' }
  | { scope: 'generation'; generation: string }
  | { scope: 'repo'; generation: string; repo: string };

export const cacheReport = writable<CacheReport | null>(null);
export const cacheError = writable<string | null>(null);
export const cacheBusy = writable(false);
/** True once a fetch has been attempted, so "nothing cached" and "not looked
 *  yet" can be told apart in the markup. */
export const cacheLoaded = writable(false);

/** Fetch the report. Safe to call repeatedly — after a clear, for instance. */
export async function loadCacheReport(): Promise<void> {
  cacheBusy.set(true);
  try {
    const resp = await fetch(apiUrl('/api/cache'));
    if (resp.status === 404) {
      cacheReport.set(null);
      cacheError.set(null);
      return;
    }
    if (!resp.ok) {
      cacheError.set(await resp.text());
      return;
    }
    cacheReport.set((await resp.json()) as CacheReport);
    cacheError.set(null);
  } catch (e) {
    cacheError.set(e instanceof Error ? e.message : String(e));
  } finally {
    cacheBusy.set(false);
    cacheLoaded.set(true);
  }
}

/**
 * Remove part of the cache, then re-read so the panel shows what is actually
 * there rather than what it predicted would be.
 *
 * Returns the error text on failure and null on success, so the caller can
 * decide where to put it.
 */
export async function clearCache(target: ClearTarget): Promise<string | null> {
  cacheBusy.set(true);
  try {
    const resp = await fetch(apiUrl('/api/cache/clear'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(target),
    });
    if (!resp.ok) {
      const message = await resp.text();
      cacheError.set(message);
      return message;
    }
    cacheError.set(null);
    return null;
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    cacheError.set(message);
    return message;
  } finally {
    cacheBusy.set(false);
    await loadCacheReport();
  }
}

/** Bytes as a person reads them. Binary units, because that is what a disk
 *  reports and a mismatch here reads as the panel being wrong. */
export function humanBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** Coarse relative age. Precision beyond a day is noise for a cache. */
export function humanAge(stamp: number | null): string {
  if (stamp === null) return '—';
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - stamp);
  if (seconds < 90) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 90) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 36) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/** Counts with a thousands separator, since these run to six figures. */
export function humanCount(n: number): string {
  return n.toLocaleString();
}
