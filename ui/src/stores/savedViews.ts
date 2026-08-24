/**
 * Saved views — capture the current picture, name it, get it back (UI-082).
 *
 * The rules for *what* a view holds live in `viewmodels/savedViews.ts`; this
 * module is the two things that need the stores: reading them into a
 * `ViewState`, and writing one back.
 *
 * **Restoring writes the same stores the controls write.** There is no view
 * mode and no second state layer — a restored view is indistinguishable from
 * having set it by hand, which is what lets the reader keep adjusting from
 * there. The cost is that the order matters, and `restoreView` documents why.
 *
 * **Where they live.** `<root>/.nao/views.json` through `/api/views`, so the
 * browser UI and the VS Code webview share one list and a team can commit it
 * (ADR 0008 puts repo-shaped state in the repo). `nao serve` does not have
 * that route — the tree there came from a URL a stranger pasted — so a 404
 * means "this server has no view store" and the list falls back to this
 * browser's `localStorage`. A 5xx means something else entirely: the file
 * exists and could not be read, and saving is refused until it can, rather
 * than overwriting a list we failed to parse.
 */

import { derived, get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { apiUrl, serveRepo } from '../vscodeAdapter';
import {
  graphData,
  graphLevel,
  ringFocusPath,
  ringReach,
  generalEntityTypes,
  generalRelTypes,
  generalOutgoing,
  generalIncoming,
  hiddenLanguages,
  hiddenFiles,
  levelOverrides,
  showGhostNodes,
  showBuiltinGhosts,
  showTemplateVars,
  structureOnly,
  searchHidesNonMatches,
} from './graph';
import { asNavigation, autoLevel, applySelection, fullGraphDataStore, scopeRules } from './scope';
import { searchTerm, committedSearchIds } from '../viewmodels/filterViewModel';
import { specSelection, setSpecSelection, specGraph } from './crossFilter';
import {
  emptyState,
  normalizeViews,
  pruneState,
  sameState,
  uniqueName,
  type Dropped,
  type Present,
  type SavedView,
  type ViewState,
} from '../viewmodels/savedViews';
import type { LevelOverrides } from '../types/graph';

export type { SavedView, ViewState } from '../viewmodels/savedViews';
export { droppedSummary, nothingDropped, stateSummary } from '../viewmodels/savedViews';

/** Where the list is being kept. `unknown` until the first load answers. */
export type ViewStore = 'unknown' | 'repo' | 'local';

export const savedViews = writable<SavedView[]>([]);
export const viewStore = writable<ViewStore>('unknown');
/** Non-null when the store is unusable. Saving is refused while it is set —
 *  every path that lands here is one where writing would destroy something. */
export const viewsError = writable<string | null>(null);
export const viewsBusy = writable(false);

/** Human name for where a view is being written, for the panel's hint. */
export const viewStoreLabel: Readable<string> = derived(viewStore, ($s) =>
  $s === 'repo' ? '.nao/views.json' : $s === 'local' ? 'this browser' : '…',
);

// ─────────────────────────────────────────────────────────────────────────────
// Capture and restore
// ─────────────────────────────────────────────────────────────────────────────

/** The picture on screen, as a `ViewState`. */
export function captureState(): ViewState {
  return {
    scope: get(scopeRules).map((r) => ({ ...r })),
    level: get(graphLevel),
    autoLevel: get(autoLevel),
    ringFocus: get(ringFocusPath),
    ringReach: get(ringReach),
    entityTypes: [...get(generalEntityTypes)],
    relTypes: [...get(generalRelTypes)],
    outgoing: get(generalOutgoing),
    incoming: get(generalIncoming),
    levelOverrides: structuredClone(get(levelOverrides)),
    hiddenLanguages: [...get(hiddenLanguages)],
    hiddenFiles: [...get(hiddenFiles)],
    showGhosts: get(showGhostNodes),
    showBuiltinGhosts: get(showBuiltinGhosts),
    showTemplateVars: get(showTemplateVars),
    structureOnly: get(structureOnly),
    spec: [...get(specSelection)],
    searchTerm: get(searchTerm),
    searchIds: [...get(committedSearchIds)],
    searchHides: get(searchHidesNonMatches),
  };
}

/**
 * What the current analysis still has, for pruning a view saved against an
 * older one.
 *
 * Three different universes, deliberately:
 *
 * - **files** come from the *full* graph, not the scoped one — a hidden file
 *   outside the restored scope is still a decision the reader made (UI-047).
 * - **entity ids** come from `graphData`, the graph *at the level now being
 *   drawn*: a search committed at File level holds collapsed ids, and
 *   checking those against entity-level ids would drop every one of them.
 * - **spec ids** come from the spec graph, which is cut from the unscoped
 *   data and so is unaffected by the scope the restore just applied.
 */
function present(): Present {
  const full = get(fullGraphDataStore);
  return {
    files: new Set((full?.nodes ?? []).map((n) => n.file_path)),
    entityIds: new Set(get(graphData).nodes.map((n) => n.id)),
    specIds: new Set(get(specGraph).nodes.map((n) => n.id)),
  };
}

/** Saved overrides laid over the freshly seeded ones, so every level and
 *  every kind present in the dataset still has an entry. `seedFromGraph`
 *  rebuilds those on publish; a wholesale replace would leave the kinds the
 *  view never mentioned with no tri-state at all. */
function mergeOverrides(saved: Record<number, LevelOverrides>): Record<number, LevelOverrides> {
  const current = get(levelOverrides);
  const out: Record<number, LevelOverrides> = {};
  for (const key of Object.keys(current)) {
    const level = Number(key);
    const base = current[level];
    const over = saved[level];
    out[level] = over
      ? {
          ...base,
          ...over,
          entityTypes: { ...base.entityTypes, ...over.entityTypes },
          relTypes: { ...base.relTypes, ...over.relTypes },
        }
      : base;
  }
  return out;
}

/**
 * Put the canvas back into the picture `s` describes.
 *
 * The order is the whole subtlety. `applySelection` republishes the dataset,
 * and publishing re-seeds the entity-kind, relationship-kind and per-level
 * tri-state filters to "everything in the new data" (see
 * `filterViewModel.seedFromGraph`). So anything the seed touches has to be
 * written *after* the scope has been applied, and anything it deliberately
 * leaves alone — the exclusions — before or after. Writing the kinds first
 * looks like it works and is silently undone one microtask later.
 *
 * Returns what had to be dropped, for the caller to report.
 *
 * Taken as a bare `ViewState` rather than a `SavedView` because the wayback
 * (UI-092) restores frames nobody named, and the only thing a name adds to a
 * restore is a label in a toast. `restoreView` below is the named door.
 *
 * The whole thing is one navigation step, so pressing Back after restoring a
 * view returns to the picture the reader restored it *from*. The history's
 * own steps come through `withoutNavigation`, which is what stops a back
 * press from recording the step it is undoing.
 */
export async function restoreState(s: ViewState): Promise<Dropped> {
  return asNavigation(async () => {
    // Before the republish: the level (which `applySelection` recomputes only
    // when `autoLevel` is on, which is exactly what "auto" means), the
    // exclusions the seed does not touch, and the display toggles.
    autoLevel.set(s.autoLevel);
    graphLevel.set(s.level);
    // Beside the level, and for the same reason: with a focus set the level
    // names the outer grain, so restoring one without the other draws a
    // picture the reader never saw.
    ringFocusPath.set(s.ringFocus);
    ringReach.set(s.ringReach);
    hiddenLanguages.set(new Set(s.hiddenLanguages));
    showGhostNodes.set(s.showGhosts);
    showBuiltinGhosts.set(s.showBuiltinGhosts);
    showTemplateVars.set(s.showTemplateVars);
    structureOnly.set(s.structureOnly);
    searchHidesNonMatches.set(s.searchHides);

    // Clear the outgoing search before the scope moves: its committed ids are
    // about the graph being replaced, and leaving them set would filter the
    // new one to nothing for as long as the fetch takes.
    searchTerm.set('');

    scopeRules.set(s.scope.map((r) => ({ ...r })));
    await applySelection();

    // After the republish, once the dataset — and the level — have settled.
    const { state: live, dropped } = pruneState(s, present());
    hiddenFiles.set(new Set(live.hiddenFiles));
    generalEntityTypes.set(new Set(live.entityTypes));
    generalRelTypes.set(new Set(live.relTypes));
    generalOutgoing.set(live.outgoing);
    generalIncoming.set(live.incoming);
    levelOverrides.set(mergeOverrides(live.levelOverrides));
    setSpecSelection(live.spec);

    // Term first: clearing it clears the commit, by subscription in
    // `filterViewModel`, so the two have to be written in this order.
    if (live.searchTerm.trim() !== '') {
      searchTerm.set(live.searchTerm);
      committedSearchIds.set(new Set(live.searchIds));
    }

    return dropped;
  });
}

/** Put the canvas back into a *named* view. The list's click. */
export function restoreView(view: SavedView): Promise<Dropped> {
  return restoreState(view.state);
}

/**
 * The picture on screen, as a store rather than a call.
 *
 * `captureState()` answers "what is it now"; this answers "tell me whenever it
 * changes", which is a different question and one that two features now ask.
 * The dependency list is the whole point: it is every store a `ViewState` is
 * made of, in one place, so a store added to the codec cannot be forgotten by
 * one consumer and remembered by the other.
 */
export const currentState: Readable<ViewState> = derived(
  [
    scopeRules, graphLevel, autoLevel, ringFocusPath, ringReach,
    generalEntityTypes, generalRelTypes,
    generalOutgoing, generalIncoming, levelOverrides, hiddenLanguages, hiddenFiles,
    showGhostNodes, showBuiltinGhosts, showTemplateVars, structureOnly, specSelection, searchTerm,
    committedSearchIds, searchHidesNonMatches,
  ],
  () => captureState(),
);

/**
 * The saved view the canvas is currently showing, or `null`.
 *
 * Recomputed from every store a view captures, so it goes `null` the moment
 * the reader changes anything — which is the honest signal, and the one that
 * makes "Update" meaningful. Derived on the stores rather than on a flag set
 * by `restoreView`, because a flag would go on claiming a view after the
 * picture had drifted from it.
 */
export const activeViewId: Readable<string | null> = derived(
  [savedViews, currentState],
  ([$views, $now]) => $views.find((v) => sameState(v.state, $now))?.id ?? null,
);

// ─────────────────────────────────────────────────────────────────────────────
// Persistence
// ─────────────────────────────────────────────────────────────────────────────

/**
 * The browser-storage key, used only in the fallback.
 *
 * Keyed by the serve-mode slug and nothing else, because those are the only
 * two shapes this branch ever sees: `nao serve`, where one origin hosts many
 * repos and the slug is what tells them apart, and an unreachable or
 * routeless server, where the origin *is* the repo. Keying by the analyzed
 * root would read better and be a bug — `rootPath` is fetched by a component
 * that mounts after this loads, so the list would be read under one key and
 * written under another.
 */
function localKey(): string {
  return `nao-saved-views:${serveRepo() ?? ''}`;
}

function readLocal(): SavedView[] {
  try {
    const raw = localStorage.getItem(localKey());
    return raw ? normalizeViews(JSON.parse(raw)) : [];
  } catch {
    return [];
  }
}

function writeLocal(views: SavedView[]): void {
  try { localStorage.setItem(localKey(), JSON.stringify(views)); } catch { /* blocked storage */ }
}

/** Load the list and find out where it lives. */
export async function loadViews(): Promise<void> {
  viewsBusy.set(true);
  try {
    const resp = await fetch(apiUrl('/api/views'), { cache: 'no-store' });
    if (resp.ok) {
      const body = await resp.json();
      viewStore.set('repo');
      viewsError.set(null);
      savedViews.set(normalizeViews(body?.views));
      return;
    }
    if (resp.status === 404) {
      // No such route: `nao serve`, or a server older than UI-082. Keep the
      // views in the browser rather than losing the feature.
      viewStore.set('local');
      viewsError.set(null);
      savedViews.set(readLocal());
      return;
    }
    viewStore.set('repo');
    viewsError.set(`Could not read saved views (HTTP ${resp.status}). ${await resp.text()}`);
    savedViews.set([]);
  } catch (e) {
    // The server is unreachable rather than answering — same conclusion as a
    // 404 for the reader's purposes, and the browser copy is better than none.
    viewStore.set('local');
    viewsError.set(null);
    savedViews.set(readLocal());
  } finally {
    viewsBusy.set(false);
  }
}

/**
 * Write the whole list.
 *
 * Whole-list replace, matching the endpoint: the client holds what it is
 * editing, and two clients editing one repo's views in the same instant is
 * not a case this tool has. Returns false and leaves `viewsError` set when
 * the write failed, so the caller can keep the user's edit on screen.
 */
async function persist(views: SavedView[]): Promise<boolean> {
  if (get(viewStore) === 'local') {
    writeLocal(views);
    return true;
  }
  try {
    const resp = await fetch(apiUrl('/api/views'), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ version: 1, views }),
    });
    if (!resp.ok) {
      viewsError.set(`Could not save (HTTP ${resp.status}). ${await resp.text()}`);
      return false;
    }
    viewsError.set(null);
    return true;
  } catch (e) {
    viewsError.set(`Could not save — ${e}`);
    return false;
  }
}

/** Apply `mutate` to the list, persist it, and roll back if the write failed.
 *  Rolling back is the point: a list that shows a view the file does not
 *  have is how a reader loses one without being told. */
async function commit(mutate: (views: SavedView[]) => SavedView[]): Promise<boolean> {
  const before = get(savedViews);
  const after = mutate(before);
  savedViews.set(after);
  viewsBusy.set(true);
  try {
    const ok = await persist(after);
    if (!ok) savedViews.set(before);
    return ok;
  } finally {
    viewsBusy.set(false);
  }
}

function newId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `v-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
  }
}

/** Save the current picture under `name` (uniquified against the list). */
export async function saveCurrentView(name: string): Promise<boolean> {
  const state = captureState();
  return commit((views) => [
    ...views,
    {
      id: newId(),
      name: uniqueName(name, views.map((v) => v.name)),
      saved_at: new Date().toISOString(),
      state,
    },
  ]);
}

/** Replace a saved view's state with the current picture, keeping its name. */
export async function updateView(id: string): Promise<boolean> {
  const state = captureState();
  return commit((views) =>
    views.map((v) => (v.id === id ? { ...v, state, saved_at: new Date().toISOString() } : v)),
  );
}

export async function renameView(id: string, name: string): Promise<boolean> {
  const trimmed = name.trim();
  if (trimmed === '') return false;
  return commit((views) =>
    views.map((v) =>
      v.id === id
        ? { ...v, name: uniqueName(trimmed, views.filter((o) => o.id !== id).map((o) => o.name)) }
        : v,
    ),
  );
}

export async function deleteView(id: string): Promise<boolean> {
  return commit((views) => views.filter((v) => v.id !== id));
}

/** An empty view state, for callers that need one before anything is saved. */
export { emptyState };
