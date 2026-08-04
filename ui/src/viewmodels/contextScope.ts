import { derived, writable, get } from 'svelte/store';
import { selectedNode } from '../stores/graph';
import { apiUrl } from '../vscodeAdapter';
import { currentEndpoint } from '../stores/connection';

// --- Types ---

export type ScopeMode = 'manual' | 'refactor' | 'understand';

interface ScopeEntity {
  id: string;
  name: string;
  qualified_name: string;
  kind: string;
  file_path: string;
  line: number;
  end_line: number;
  source_code: string | null;
  reasons: string[];
}

interface ScopeExports {
  paths: string;
  ranges: string;
  entity_context: string;
  full_files: string;
  /**
   * Paste-ready refactoring instruction with the entity context inline
   * (SRV-010). Optional because the UI can be pointed at an engine that
   * predates the export — callers must treat `undefined` as "this engine
   * can't do that" and hide the affordance rather than copying nothing.
   */
  refactor_prompt?: string;
}

interface ScopeApiResponse {
  entities: ScopeEntity[];
  reason_summary: Record<string, number>;
  files: string[];
  token_count_entities: number;
  token_count_files: number;
  exports: ScopeExports;
}

// --- State ---

export const contextDepth = writable<number>(1);
export const scopeMode = writable<ScopeMode>('manual');
export const excludedFiles = writable<Set<string>>(new Set());

// Reset excluded files when selected node or mode changes
selectedNode.subscribe(() => {
  excludedFiles.set(new Set());
});
scopeMode.subscribe(() => {
  excludedFiles.set(new Set());
});

// --- API response store ---

export const scopeResponse = writable<ScopeApiResponse | null>(null);
export const scopeLoading = writable(false);

// Track last request to avoid stale responses
let requestId = 0;

/** Call the backend scope API and update the response store. */
export async function fetchScope(): Promise<void> {
  const node = get(selectedNode);
  if (!node) {
    scopeResponse.set(null);
    return;
  }

  const mode = get(scopeMode);
  const depth = get(contextDepth);
  const excluded = get(excludedFiles);

  const myRequestId = ++requestId;
  scopeLoading.set(true);

  try {
    const resp = await fetch(apiUrl('/api/scope'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        entity_id: node.original_id,
        mode,
        depth,
        excluded_files: [...excluded],
      }),
    });

    if (!resp.ok) {
      console.error('Scope API error:', await resp.text());
      scopeResponse.set(null);
      return;
    }

    const data: ScopeApiResponse = await resp.json();

    // Only apply if this is still the latest request
    if (myRequestId === requestId) {
      scopeResponse.set(data);
    }
  } catch (e) {
    console.error('Scope API fetch failed:', e);
    scopeResponse.set(null);
  } finally {
    if (myRequestId === requestId) {
      scopeLoading.set(false);
    }
  }
}

/**
 * Fetch just the refactor prompt for one entity, without disturbing the
 * shared scope stores.
 *
 * `fetchScope` above is bound to `selectedNode` and owns `scopeResponse`;
 * the quality leaderboard needs a prompt for a row the user has *not*
 * selected, so routing that through the shared store would clobber the
 * Context Scope panel and reset the file checkboxes. Always `refactor` mode —
 * this is the refactor affordance regardless of the panel's current mode.
 *
 * Returns `null` when the request fails or the engine predates the export.
 */
export async function fetchRefactorPrompt(entityId: string): Promise<string | null> {
  try {
    const resp = await fetch(apiUrl('/api/scope'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ entity_id: entityId, mode: 'refactor', depth: 1 }),
    });
    if (!resp.ok) {
      console.error('Scope API error:', await resp.text());
      return null;
    }
    const data: ScopeApiResponse = await resp.json();
    return data.exports?.refactor_prompt ?? null;
  } catch (e) {
    console.error('Refactor prompt fetch failed:', e);
    return null;
  }
}

/**
 * Ask the engine to open a Claude Code terminal on its own machine for one
 * entity (SRV-017).
 *
 * Only meaningful when the engine and the browser are the same machine, which
 * is the default. The pairing token is always required for this route — the
 * loopback exemption that applies to reading the graph does not extend to
 * executing code — so it is sent explicitly rather than relying on the
 * ambient policy.
 */
export async function spawnAgentTerminal(entityId: string): Promise<string | null> {
  try {
    const { token } = currentEndpoint();
    const resp = await fetch(apiUrl('/api/agents/terminal'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      // `hybrid`, not the API's `full` default: the agent is launched inside
      // the repo with a Read tool and uses it regardless, so embedding every
      // neighbour's source pays for an index it will not open. The target's
      // own source — the one thing certainly needed — is still carried in
      // full. ~4.4k tokens instead of ~23.5k per launch.
      //
      // The copy-prompt button deliberately keeps `full`: whoever pastes it
      // may have no repository to read from.
      body: JSON.stringify({ entity_id: entityId, token, prompt_context: 'hybrid' }),
    });
    if (!resp.ok) return (await resp.text()) || `HTTP ${resp.status}`;
    return null;
  } catch (e) {
    return e instanceof Error ? e.message : String(e);
  }
}

// --- Derived convenience stores from API response ---

export const contextFiles = derived(scopeResponse, ($resp) => {
  if (!$resp) return [];
  // Count entities per file
  const counts = new Map<string, number>();
  for (const e of $resp.entities) {
    counts.set(e.file_path, (counts.get(e.file_path) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, count]) => ({ path, count }));
});

export const scopeAnnotations = derived(scopeResponse, ($resp) => {
  if (!$resp) return new Map<string, Set<string>>();
  const map = new Map<string, Set<string>>();
  for (const e of $resp.entities) {
    map.set(e.id, new Set(e.reasons));
  }
  return map;
});

// --- Actions ---

export function toggleContextFile(path: string, included: boolean): void {
  excludedFiles.update((set) => {
    const next = new Set(set);
    if (included) {
      next.delete(path);
    } else {
      next.add(path);
    }
    return next;
  });
}

export function toggleAllContextFiles(included: boolean, allPaths: string[]): void {
  if (included) {
    excludedFiles.set(new Set());
  } else {
    excludedFiles.set(new Set(allPaths));
  }
}
