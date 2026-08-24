import { writable, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import type { D3Field } from '../types/graph';
import { apiUrl } from '../vscodeAdapter';

export interface EntityDetails {
  documentation?: string;
  source_code?: string;
  fields?: D3Field[];
  impl_blocks?: string[];
}

type DetailsMap = Record<string, EntityDetails>;

// Cache: loaded once, keyed by original entity ID
const detailsCache = writable<DetailsMap | null>(null);
let fetchPromise: Promise<DetailsMap> | null = null;

async function fetchDetailsFile(): Promise<DetailsMap> {
  try {
    const resp = await fetch(apiUrl('/api/details'), { cache: 'no-store' });
    if (!resp.ok) return {};
    return await resp.json();
  } catch {
    return {};
  }
}

/** The whole sidecar map, fetched once and cached. Repo-wide, so lookups
 *  work for entities outside the current scope — `descriptionChain` relies
 *  on that to keep walking past a scope boundary. */
export function ensureDetailsLoaded(): Promise<DetailsMap> {
  return ensureLoaded();
}

/**
 * The same map as a read-only store, for the consumer that wants every
 * entity's source rather than the selected one — `diffChurnIndex` line-diffs
 * the whole changed set at once.
 *
 * Subscribe-only: `ensureDetailsLoaded` stays the single door that fetches,
 * so a second reader cannot start a second request for the one file everyone
 * shares. `null` until that resolves, which a derived consumer reads as "not
 * measurable yet" rather than "empty".
 */
export const detailsMap: Readable<DetailsMap | null> = { subscribe: detailsCache.subscribe };

function ensureLoaded(): Promise<DetailsMap> {
  const cached = get(detailsCache);
  if (cached !== null) return Promise.resolve(cached);
  fetchPromise ??= fetchDetailsFile().then((data) => {
    detailsCache.set(data);
    return data;
  });
  return fetchPromise;
}

/** Current detail for a specific entity (reactive store). Includes the
 *  entity ID so consumers can verify the detail still belongs to the
 *  entity they're displaying — prevents stale cross-assignment when two
 *  EntityInfo instances (selected + hovered) share the same store. */
export const currentDetail = writable<{ entityId: string; detail: EntityDetails } | null>(null);

/** Clear the details cache so the next `loadDetail` call re-fetches from the server. */
export function resetDetailsCache(): void {
  detailsCache.set(null);
  fetchPromise = null;
  currentDetail.set(null);
}

/** Load details for a given entity ID. Updates the currentDetail store. */
export async function loadDetail(entityId: string | null): Promise<void> {
  if (!entityId) {
    currentDetail.set(null);
    return;
  }
  const map = await ensureLoaded();
  const detail = map[entityId] || null;
  currentDetail.set(detail ? { entityId, detail } : null);
}
