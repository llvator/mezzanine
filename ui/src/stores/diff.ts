/**
 * Diff store: loads the diff.json produced by `nao diff` and exposes
 * per-entity change status for the graph view and detail panel.
 */

import { writable, derived, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import { isServeMode } from './serveMode';

export type ChangeStatus = 'added' | 'removed' | 'modified' | 'unchanged';

export interface MetricDelta {
  name: string;
  old?: number;
  new?: number;
  delta: number;
}

export interface EntityDiff {
  entity_id: string;
  name: string;
  kind: string;
  file_path: string;
  status: ChangeStatus;
  /** True if source code or intrinsic metrics changed (core change).
   *  False if only relational metrics (fan_in/fan_out) changed (impact). */
  source_changed: boolean;
  metric_deltas: MetricDelta[];
  base_entity_id?: string;
}

export interface DiffSummary {
  total_base: number;
  total_head: number;
  added: number;
  removed: number;
  modified: number;
  /** Subset of modified: entities whose source code or intrinsic metrics changed. */
  modified_source: number;
  /** Subset of modified: entities where only relational metrics changed. */
  modified_impact: number;
  unchanged: number;
}

export interface DiffData {
  from_ref: string;
  to_ref: string;
  summary: DiffSummary;
  entities: EntityDiff[];
}

/** Whether diff mode is active. */
export const diffActive = writable(false);

/** When true, only show added/removed/modified entities — hide unchanged. */
export const diffChangesOnly = writable(false);

/** When true, only show core changes (source_changed=true) — hide impact-only changes.
 *  Defaults to true: for most diffs the impact-only "ripple" entities outnumber the actual
 *  code-changed entities and drown the signal. The user can turn the filter off to see them. */
export const diffCoreOnly = writable(true);

/** Master toggle for whether diff filters (changesOnly / coreOnly / dimOpacity)
 *  affect the graph. When false, a diff can still be loaded (entities colored
 *  added/removed/modified) but the filter toggles have no visual effect —
 *  useful when the user wants to see full context around changed nodes
 *  without the filters hiding neighbors. */
export const diffFiltersEnabled = writable(true);

/** Opacity for unchanged/filtered-out nodes when diff filters are active (0 = hidden, 1 = fully visible). */
export const diffDimOpacity = writable(0);

/** The loaded diff data. Null when no diff is loaded. */
export const diffData = writable<DiffData | null>(null);

/**
 * Normalize an entity ID by stripping the temp worktree path prefix.
 * Entity IDs look like: `/var/folders/.../nao-diff-head-xxx/src/main.rs:123:foo`
 * We want to extract: `src/main.rs:123:foo` (the relative path portion).
 *
 * This allows matching between diff.json and data.json even when they were
 * generated with different temp directory paths.
 */
export function normalizeEntityId(id: string): string {
  // Match the pattern: .../nao-diff-(head|base)-<hash>/<relative-path>
  const match = id.match(/nao-diff-(?:head|base)-[^/]+\/(.+)$/);
  if (match) return match[1];
  // Fallback: try to extract just the file:line:name portion (last path segment with colons)
  const lastSlash = id.lastIndexOf('/');
  if (lastSlash !== -1 && id.includes(':')) {
    // Walk back to find where the relative path starts (first component after a recognizable root)
    // Common patterns: src/, test_data/, ui/
    for (const marker of ['src/', 'test_data/', 'ui/', 'agents/', 'docs/']) {
      const idx = id.indexOf(marker);
      if (idx !== -1) return id.slice(idx);
    }
  }
  return id;
}

/** Map from normalized entity ID → change status for O(1) lookup. */
export const diffStatusMap: Readable<Map<string, ChangeStatus>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, ChangeStatus>();
    if (!$d) return map;
    for (const e of $d.entities) {
      map.set(normalizeEntityId(e.entity_id), e.status);
    }
    return map;
  },
);

/** Map from normalized entity ID → whether it's a core change (source_changed=true). */
export const diffSourceChangedMap: Readable<Map<string, boolean>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, boolean>();
    if (!$d) return map;
    for (const e of $d.entities) {
      map.set(normalizeEntityId(e.entity_id), e.source_changed ?? true);
    }
    return map;
  },
);

/** Map from normalized entity ID → metric deltas (only for modified entities). */
export const diffDeltaMap: Readable<Map<string, MetricDelta[]>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, MetricDelta[]>();
    if (!$d) return map;
    for (const e of $d.entities) {
      if (e.metric_deltas && e.metric_deltas.length > 0) {
        map.set(normalizeEntityId(e.entity_id), e.metric_deltas);
      }
    }
    return map;
  },
);

/** Map from head entity ID → base entity ID (for loading base details). */
export const diffBaseIdMap: Readable<Map<string, string>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, string>();
    if (!$d) return map;
    for (const e of $d.entities) {
      if (e.base_entity_id) map.set(e.entity_id, e.base_entity_id);
    }
    return map;
  },
);

/** Load diff from the API. Silently no-ops if no diff is available (404). */
export async function loadDiff(): Promise<void> {
  // Serve mode has no diff endpoint (SRV-003 left it out of the per-repo
  // namespace), so skip the request rather than probing for a 404.
  if (isServeMode()) return;
  try {
    const resp = await fetch(apiUrl('/api/diff'), { cache: 'no-store' });
    if (resp.ok) {
      const data: DiffData = await resp.json();
      console.log(`[diff] Loaded: ${data.summary.added} added, ${data.summary.removed} removed, ${data.summary.modified} modified`);
      diffData.set(data);
      diffActive.set(true);
      // Also load base details for side-by-side comparison
      await loadBaseDetails();
    } else {
      diffData.set(null);
      diffActive.set(false);
      baseDetailsCache.set(null);
    }
  } catch {
    diffData.set(null);
    diffActive.set(false);
    baseDetailsCache.set(null);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Base details for side-by-side comparison
// ─────────────────────────────────────────────────────────────────────────────

interface BaseEntityDetail {
  source_code?: string;
  documentation?: string;
  fields?: { name: string; type_name: string }[];
  impl_blocks?: string[];
}

type BaseDetailsMap = Record<string, BaseEntityDetail>;

/** Cache of base (before) entity details, loaded alongside diff. */
export const baseDetailsCache = writable<BaseDetailsMap | null>(null);

/** Fetch base details from the API. Called after diff is loaded. */
async function loadBaseDetails(): Promise<void> {
  try {
    const resp = await fetch(apiUrl('/api/details/base'), { cache: 'no-store' });
    if (resp.ok) {
      const data: BaseDetailsMap = await resp.json();
      baseDetailsCache.set(data);
    } else {
      baseDetailsCache.set(null);
    }
  } catch {
    baseDetailsCache.set(null);
  }
}

/** Look up base source code for an entity by its base entity ID. */
export function getBaseSource(baseEntityId: string | undefined): string | undefined {
  if (!baseEntityId) return undefined;
  return get(baseDetailsCache)?.[baseEntityId]?.source_code;
}

/** Color for each change status (used by GraphView node stroke). */
export const DIFF_COLORS: Record<ChangeStatus, string> = {
  added: '#4CAF50',
  removed: '#F44336',
  modified: '#FFA726',
  unchanged: '',
};

// ─────────────────────────────────────────────────────────────────────────────
// Commit selection API
// ─────────────────────────────────────────────────────────────────────────────

export interface Commit {
  hash: string;
  short_hash: string;
  message: string;
  author: string;
  date: string;
}

/** Store for available commits. */
export const commits = writable<Commit[]>([]);

/** Whether we're currently fetching commits. */
export const commitsLoading = writable<boolean>(false);

/** Whether we're currently computing a diff. */
export const diffComputing = writable<boolean>(false);

/** Error message from commit/diff operations. */
export const diffApiError = writable<string | null>(null);

/** Fetch recent commits from the server. */
export async function fetchCommits(limit = 50): Promise<void> {
  commitsLoading.set(true);
  diffApiError.set(null);
  try {
    const resp = await fetch(apiUrl(`/api/commits?limit=${limit}`));
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    const data: Commit[] = await resp.json();
    commits.set(data);
  } catch (err) {
    diffApiError.set(`Failed to fetch commits: ${err}`);
    commits.set([]);
  } finally {
    commitsLoading.set(false);
  }
}

/** Trigger a diff computation between two refs (commits/branches). */
export async function triggerDiff(fromRef: string, toRef: string): Promise<void> {
  diffComputing.set(true);
  diffApiError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/diff'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ from_ref: fromRef, to_ref: toRef }),
    });
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    // After successful diff, reload the diff.json
    await loadDiff();
  } catch (err) {
    diffApiError.set(`Failed to compute diff: ${err}`);
  } finally {
    diffComputing.set(false);
  }
}
