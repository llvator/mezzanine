/**
 * Diff store: loads the diff.json produced by `nao diff` and exposes
 * per-entity change status for the graph view and detail panel.
 */

import { writable, derived, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import { isServeMode } from './serveMode';
import { rollUpByScope, rollUpCounts, normalizeScopePath, normalizeEntityId, type ScopeChange, type ScopeTally } from '../viewmodels/diffRollup';
import { buildChurnIndex, type ChurnIndex } from '../viewmodels/diffChurn';
import type { D3Node } from '../types/graph';
import { detailsMap, ensureDetailsLoaded } from './details';
import type { DiffLevel, DiffSeedFacet } from '../viewmodels/diffLevels';
import { headIsWorkingTree } from '../viewmodels/diffVerdict';

export type { DiffLevel, DiffSeedFacet };

export type ChangeStatus = 'added' | 'removed' | 'modified' | 'unchanged';

export interface MetricDelta {
  name: string;
  old?: number;
  new?: number;
  delta: number;
}

/** Which end of a changed edge the entity carrying it sits on. */
export type RelDirection = 'outgoing' | 'incoming';

/**
 * One relationship that appeared or disappeared, as the engine reports it
 * (`rel_deltas` in diff.json).
 *
 * Only entities present on both sides carry these: every edge of a brand-new
 * function is new by construction, so listing them there would bury the ones
 * that say something.
 */
export interface RelationshipDelta {
  /** Always 'added' or 'removed' — an edge either exists or it doesn't. */
  status: 'added' | 'removed';
  direction: RelDirection;
  /** Snake-case kind, matching the graph links (`calls`, `imports`). */
  kind: string;
  /** Already inflected for the direction: "calls" / "called by". */
  label: string;
  other_name: string;
  other_kind: string;
  other_file: string;
  /** Head-graph ID of the far end, when it still exists there. */
  other_entity_id?: string;
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
  /** Absent when this entity gained and lost nothing. */
  rel_deltas?: RelationshipDelta[];
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
  /** Edges that appeared / disappeared with at least one end that survived
   *  the diff. Absent from a diff.json written before UI-086. */
  relationships_added?: number;
  relationships_removed?: number;
}

export interface DiffData {
  from_ref: string;
  to_ref: string;
  summary: DiffSummary;
  entities: EntityDiff[];
}

/** Whether diff mode is active. */
export const diffActive = writable(false);

/**
 * How wide the diff draws — the ladder that replaced the `Changes` and `Core`
 * checkboxes (UI-088).
 *
 * Those two read as near-synonyms and needed three paragraphs of tooltip
 * apiece to tell apart, while neither said anything about *edges*: both were
 * node filters, and the canvas drew whatever wiring happened to run between
 * the survivors. The rungs are ordered narrow → wide, so the control says
 * which way it moves the picture. See `viewmodels/diffLevels.ts`.
 *
 * Defaults to `edits`, the narrowest — which is what `coreOnly: true` meant
 * before, for the same reason: on most diffs the ripple outnumbers the real
 * edits and drowns them.
 */
export const diffLevel = writable<DiffLevel>('edits');

/**
 * Which half of the change the ladder starts from (UI-109).
 *
 * Orthogonal to the rungs, and narrows the seed they widen from: `new` is code
 * that did not exist on the base side, `existing` is code that did and was
 * changed in place. See `viewmodels/diffLevels.ts` for why it is a second
 * control rather than a fourth rung.
 *
 * Defaults to `all`, which is the picture as it was before this existed — the
 * split is an addition, and a reader who never touches it sees no change.
 */
export const diffSeedFacet = writable<DiffSeedFacet>('all');

/** Master toggle for whether diff filters (changesOnly / coreOnly / dimOpacity)
 *  affect the graph. When false, a diff can still be loaded (entities colored
 *  added/removed/modified) but the filter toggles have no visual effect —
 *  useful when the user wants to see full context around changed nodes
 *  without the filters hiding neighbors. */
export const diffFiltersEnabled = writable(true);

/** Opacity for unchanged/filtered-out nodes when diff filters are active (0 = hidden, 1 = fully visible). */
export const diffDimOpacity = writable(0);

/**
 * How strongly the canvas draws what a rung recruited, against the edits it
 * grew from (UI-112).
 *
 * `Rest` fades what the ladder *rejected*; this weights what it *accepted*
 * for a reason other than being an edit — the far end of a changed edge at
 * `rewiring`, anything one hop out at `neighbourhood`. Those arrive at full
 * strength and outnumber the edits, which is how widening the ladder loses
 * the very nodes it was widened to give context to.
 *
 * A third tier, not a second ladder: it cannot add or remove a node. That is
 * why it is floored well above zero — a control that could empty the rung
 * would silently duplicate stepping down one, and the reader would have two
 * ways to reach the same picture with only one of them named for it.
 */
export const CONTEXT_OPACITY_FLOOR = 0.1;

/** Default weight for recruited context. Low enough that the edits read as
 *  the subject at a glance, high enough that a neighbour is still legible. */
export const diffContextOpacity = writable(0.4);

/** The loaded diff data. Null when no diff is loaded. */
export const diffData = writable<DiffData | null>(null);

/**
 * Normalize an entity ID by stripping the temp worktree path prefix, so
 * diff.json and data.json can be keyed alike.
 *
 * Re-exported rather than defined here: it is a pure string function with a
 * trap in it, and it belongs beside `normalizeScopePath`, which strips the
 * same prefix off paths for the same reason — and where `node --test` can
 * reach it without svelte (`npm run test:diff`). Every existing import site
 * keeps working.
 */
export { normalizeEntityId } from '../viewmodels/diffRollup';

/**
 * Whether the loaded diff's head is the tree the canvas is drawing.
 *
 * `→ WORKING` analyses the live tree, so the two agree. A commit-to-commit
 * comparison does not: the server declines to adopt a commit's head graph
 * (SRV-019), so the canvas keeps showing the working tree while the diff
 * describes a pair of older ones. That gap is invisible until something asks
 * what the diff's *silence* about a file means — see `diffVerdict` (UI-104).
 *
 * False with no diff loaded, which is the safe reading: nothing is claiming to
 * describe this tree.
 */
export const diffHeadIsWorking: Readable<boolean> = derived(
  diffData,
  ($d) => headIsWorkingTree($d?.to_ref),
);

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

/**
 * Scope path → rolled-up change, covering every file and directory the diff
 * reported on (UI-064).
 *
 * The two maps above answer "did this entity change". Above entity
 * aggregation the canvas draws scopes, not entities, so this answers "did
 * anything in this scope change" — and, because the diff reports on
 * unchanged entities too, `has(path)` separately answers "was this scope in
 * the diff at all". A file created after the diff ran is absent from both
 * maps and from this one, and that absence is a fact worth keeping: it is
 * not the same as unchanged.
 */
export const diffScopeChanges: Readable<Map<string, ScopeChange>> = derived(
  diffData,
  ($d) => rollUpByScope($d?.entities ?? []),
);

/**
 * Scope path → how many entities under it changed, and how.
 *
 * What the details pane shows for a file or folder node. Such a node has no
 * row in the diff at all — `compute_diff` walks entities, and a file is not
 * one — so without this the pane could say nothing about the very node the
 * canvas is most often drawing (UI-097).
 */
export const diffScopeCounts: Readable<Map<string, ScopeTally>> = derived(
  diffData,
  ($d) => rollUpCounts($d?.entities ?? []),
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

/** Normalized entity ID → the relationships it gained and lost. */
export const diffRelDeltaMap: Readable<Map<string, RelationshipDelta[]>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, RelationshipDelta[]>();
    if (!$d) return map;
    for (const e of $d.entities) {
      if (e.rel_deltas && e.rel_deltas.length > 0) {
        map.set(normalizeEntityId(e.entity_id), e.rel_deltas);
      }
    }
    return map;
  },
);

/**
 * Normalized ids of entities the diff reported as `added`.
 *
 * Needed because UI-086 deliberately gives an added entity no `rel_deltas`:
 * every edge of a brand-new function is new by construction, and listing them
 * under it would bury the deltas that carry information. True for a *list*,
 * false for the canvas — without this set the newest code draws with no wiring
 * at all, which on the measured repo is 45 of 86 real changes. `displayPlan`
 * reads it to call every head edge touching one of these entities `added`.
 */
export const diffAddedEntityIds: Readable<Set<string>> = derived(
  diffData,
  ($d) => {
    const ids = new Set<string>();
    if (!$d) return ids;
    for (const e of $d.entities) {
      if (e.status === 'added') ids.add(normalizeEntityId(e.entity_id));
    }
    return ids;
  },
);

/** The edges a diff reported, indexed for the canvas. */
export interface ChangedEdges {
  /** Every edge that appeared, keyed twice over.
   *
   *  At entity level: `${src}->${tgt}|${kind}`, on normalized entity ids.
   *  The kind belongs in the key there — two entities can be joined by a
   *  `calls` edge and an `imports` edge, and only one of them may have moved.
   *
   *  Above entity level: `${srcScope}->${tgtScope}`, with no kind, for file →
   *  file and directory → directory. `collapseGraph` merges every underlying
   *  edge between two scopes into one line and relabels it `DependsOn`, so
   *  the kind it would be matched against no longer exists; the claim a
   *  collapsed line can carry is "something between these two scopes moved",
   *  and that is what this key says. */
  added: Set<string>;
  /** Edges that disappeared, counted rather than keyed: a lost edge has no
   *  line in the head graph, so there is nothing to match it against. Kept so
   *  the UI can say how many it cannot draw instead of dropping them in
   *  silence. */
  removedCount: number;
  /** Reported changes whose far end carried no `other_entity_id` — the diff
   *  saw them but cannot place them on either side. */
  unplaceable: number;
}

const edgeKey = (src: string, tgt: string, kind: string) => `${src}->${tgt}|${kind}`;

/**
 * Every appeared/disappeared edge, keyed the way the canvas keys its links.
 *
 * The deltas arrive per *endpoint* — one row on the caller, a mirrored row on
 * the callee — so the same edge is seen twice and the `Set` deduplicates it.
 * Direction is normalized here: a row is stored source → target regardless of
 * which end reported it.
 */
export const diffChangedEdges: Readable<ChangedEdges> = derived(
  diffData,
  ($d) => {
    const added = new Set<string>();
    let removedCount = 0;
    let unplaceable = 0;
    if (!$d) return { added, removedCount, unplaceable };

    for (const e of $d.entities) {
      const selfId = normalizeEntityId(e.entity_id);
      const selfFile = normalizeScopePath(e.file_path ?? '');
      for (const r of e.rel_deltas ?? []) {
        if (r.status === 'removed') {
          // Counted once, not twice: only the outgoing row, so a delta
          // mirrored onto both ends doesn't inflate the total.
          if (r.direction === 'outgoing') removedCount++;
          continue;
        }
        if (!r.other_entity_id) {
          if (r.direction === 'outgoing') unplaceable++;
          continue;
        }
        const otherId = normalizeEntityId(r.other_entity_id);
        const otherFile = normalizeScopePath(r.other_file ?? '');
        const [src, tgt] = r.direction === 'outgoing' ? [selfId, otherId] : [otherId, selfId];
        const [srcFile, tgtFile] = r.direction === 'outgoing'
          ? [selfFile, otherFile]
          : [otherFile, selfFile];
        added.add(edgeKey(src, tgt, r.kind));
        for (const [s, t] of scopePairs(srcFile, tgtFile)) {
          if (s !== t) added.add(`${s}->${t}`);
        }
      }
    }
    return { added, removedCount, unplaceable };
  },
);

/** File → file and directory → directory, the two scope levels the collapsed
 *  canvas draws. Mixed pairs are never drawn, so they aren't emitted. */
function scopePairs(srcFile: string, tgtFile: string): [string, string][] {
  if (!srcFile || !tgtFile) return [];
  const dir = (p: string) => {
    const i = p.lastIndexOf('/');
    return i < 0 ? '' : p.slice(0, i);
  };
  return [[srcFile, tgtFile], [dir(srcFile), dir(tgtFile)]];
}

/**
 * Normalized head entity ID → base entity ID, for looking up before-source.
 *
 * Keyed the same way as `diffStatusMap` and every other map here. It wasn't:
 * the key went in raw while the panel looked it up normalized, so on any repo
 * whose paths contain one of `normalizeEntityId`'s markers — `src/` among them
 * — every lookup missed, no base source was found, and the details pane fell
 * back to printing the head source with no diff at all.
 */
export const diffBaseIdMap: Readable<Map<string, string>> = derived(
  diffData,
  ($d) => {
    const map = new Map<string, string>();
    if (!$d) return map;
    for (const e of $d.entities) {
      if (e.base_entity_id) map.set(normalizeEntityId(e.entity_id), e.base_entity_id);
    }
    return map;
  },
);

/**
 * Load diff from the API. Silently no-ops if no diff is available (404).
 *
 * `baseDetails` is skippable because a live refresh does not need them
 * (UI-067): the base of a `→ working` diff is a fixed commit, so its
 * before-source never changes while the watcher recomputes the head. Fetching
 * them anyway would re-download the whole base payload on every file save.
 */
export async function loadDiff(opts: { baseDetails?: boolean } = {}): Promise<void> {
  const { baseDetails = true } = opts;
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
      // The head half of what `diffChurnIndex` measures. Already cached after
      // the first entity anyone selects, so this only pays on a reader who
      // loads a diff before opening anything — and it is the same one file
      // the details pane would fetch a moment later anyway.
      void ensureDetailsLoaded();
      // Also load base details for side-by-side comparison
      if (baseDetails) await loadBaseDetails();
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

// ─────────────────────────────────────────────────────────────────────────────
// How many lines the diff moved — the `Lines changed` size channel's domain
// ─────────────────────────────────────────────────────────────────────────────
//
// Declared down here rather than beside the other rollups because `derived`
// runs at module evaluation: a churn store written above `baseDetailsCache`
// would read it before its `writable` had been assigned.

/**
 * Head sources keyed the way the diff keys everything else.
 *
 * The sidecar is keyed by the graph's raw `original_id` while a diff row is
 * keyed by `normalizeEntityId(entity_id)`, and on a commit-to-commit diff
 * those two spellings differ by a temp worktree prefix. Normalising the
 * sidecar's keys once here is what makes the two sides meet — the same join
 * `diffBaseIdMap` exists to make on the base side, and the same one whose
 * absence made the details pane print head source as though nothing had
 * changed.
 */
const headSources: Readable<Map<string, string>> = derived(detailsMap, ($m) => {
  const out = new Map<string, string>();
  if (!$m) return out;
  for (const [id, detail] of Object.entries($m)) {
    if (detail.source_code) out.set(normalizeEntityId(id), detail.source_code);
  }
  return out;
});

/**
 * Whether line-level churn can be measured at all.
 *
 * Two facts, not a heuristic: a diff is loaded, and its before-sources
 * arrived. Without the base map every changed entity is unmeasurable and the
 * channel would draw a canvas of hollow circles — so it is withheld from the
 * picker entirely, which is the rule `SIZE_CHANNELS` already follows for a
 * metric with no rollup at the current level.
 *
 * A live refresh skips the base fetch (`loadDiff({ baseDetails: false })`)
 * but does not clear it, so the channel survives a save.
 */
export const diffChurnAvailable: Readable<boolean> = derived(
  [diffData, baseDetailsCache],
  ([$d, $b]) => $d !== null && $b !== null,
);

/**
 * Lines added + removed, per entity and per scope.
 *
 * Recomputed when the diff changes or either source map lands — once per diff
 * load, not per frame. See `viewmodels/diffChurn` for why this is churn and
 * not the `loc` delta already sitting in `diff.json`.
 */
export const diffChurnIndex: Readable<ChurnIndex> = derived(
  [diffData, baseDetailsCache, headSources],
  ([$d, $base, $head]): ChurnIndex => buildChurnIndex($d?.entities ?? [], {
    head: (e) => $head.get(normalizeEntityId(e.entity_id)),
    base: (e) => (e.base_entity_id ? $base?.[e.base_entity_id]?.source_code : undefined),
    key: (e) => normalizeEntityId(e.entity_id),
  }),
);

/**
 * Churn for one drawn node, whatever grain the canvas is at.
 *
 * Entity id first, then scope path, exactly as `displayPlan`'s `changeOf`
 * resolves a status: a collapsed File or Module node carries its scope path
 * in `original_id` where an entity carries an id, and the two cannot collide
 * because an id carries `:line:name`. Same order, so size and colour can
 * never end up describing different nodes.
 */
export const diffChurnAt: Readable<(d: D3Node) => number | undefined> = derived(
  diffChurnIndex,
  ($index) => (d: D3Node) =>
    $index.byEntity.get(normalizeEntityId(d.original_id))
    ?? $index.byScope.get(d.original_id),
);

// ─────────────────────────────────────────────────────────────────────────────
// How the details pane draws a changed entity's source
// ─────────────────────────────────────────────────────────────────────────────

/** Unified is one column with +/- markers; split is before and after
 *  side by side, the way a review tool shows them. */
export type DiffViewMode = 'unified' | 'split';

const VIEW_MODE_KEY = 'nao-diff-view-mode';
const FULL_CONTEXT_KEY = 'nao-diff-full-context';

function loadStored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key) as T | null;
    if (v !== null && allowed.includes(v)) return v;
  } catch { /* SSR / blocked storage */ }
  return fallback;
}

/** Unified by default: the details pane is a ~340px column, and two code
 *  columns in it wrap every second line. Split is there for the reader who
 *  widens it. */
export const diffViewMode = writable<DiffViewMode>(
  loadStored(VIEW_MODE_KEY, ['unified', 'split'] as const, 'unified'),
);
diffViewMode.subscribe((v) => {
  try { localStorage.setItem(VIEW_MODE_KEY, v); } catch { /* ignore */ }
});

/** When false (the default), unchanged stretches more than three lines from
 *  a change collapse to a "N lines unchanged" note, as `git diff` does. */
export const diffFullContext = writable<boolean>(
  loadStored(FULL_CONTEXT_KEY, ['true', 'false'] as const, 'false') === 'true',
);
diffFullContext.subscribe((v) => {
  try { localStorage.setItem(FULL_CONTEXT_KEY, String(v)); } catch { /* ignore */ }
});

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

/**
 * One `git stash` entry.
 *
 * `base_hash` is the commit the stash was taken on — its own first parent,
 * and the only base that shows the stashed work and nothing else. `selector`
 * is the `stash@{N}` label and is for reading only: N is a position in the
 * list and every push renumbers it, so the refs sent to the server are the
 * two hashes (UI-107).
 */
export interface Stash {
  hash: string;
  short_hash: string;
  selector: string;
  base_hash: string;
  base_short: string;
  message: string;
  author: string;
  date: string;
}

/** Store for the repository's stash entries. */
export const stashes = writable<Stash[]>([]);

/** Whether we're currently fetching stashes. */
export const stashesLoading = writable<boolean>(false);

/**
 * Fetch the repository's stashes.
 *
 * An empty list is a normal answer, not a failure — most repositories have no
 * stashes — so it is left to the panel to say so rather than surfaced through
 * `diffApiError`.
 */
export async function fetchStashes(): Promise<void> {
  stashesLoading.set(true);
  diffApiError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/stashes'));
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    const data: Stash[] = await resp.json();
    stashes.set(data);
  } catch (err) {
    diffApiError.set(`Failed to fetch stashes: ${err}`);
    stashes.set([]);
  } finally {
    stashesLoading.set(false);
  }
}

/**
 * One path in the index, as `GET /api/staged` reports it.
 *
 * No hash of any kind, deliberately. The commit that names the index is
 * manufactured inside the diff call and is unreferenced, so there is nothing
 * here worth holding: the ref sent back is the `STAGED` sentinel and the server
 * resolves it afresh. This is the `stash@{N}` lesson from the other end — there
 * a label outlived the thing and stopped meaning it; here a hash would name a
 * commit that means whatever the index was when it was made (UI-111).
 */
export interface StagedFile {
  status: string;
  path: string;
}

/** Store for the paths currently in the index. */
export const stagedFiles = writable<StagedFile[]>([]);

/** Whether we're currently fetching the staged list. */
export const stagedLoading = writable<boolean>(false);

/**
 * Fetch what is in the index.
 *
 * Two git calls, against a diff's worktree checkout and full analysis — which
 * is why the picker asks this first. An empty list is a normal answer and left
 * to the panel to say, the way the stash list is.
 */
export async function fetchStaged(): Promise<void> {
  stagedLoading.set(true);
  diffApiError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/staged'));
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    const data: StagedFile[] = await resp.json();
    stagedFiles.set(data);
  } catch (err) {
    diffApiError.set(`Failed to read the index: ${err}`);
    stagedFiles.set([]);
  } finally {
    stagedLoading.set(false);
  }
}

/**
 * Trigger a diff computation between two refs (commits/branches), or the
 * `WORKING` / `STAGED` sentinels.
 *
 * A 200 is not the same as a diff. The server answers `success: false` for the
 * comparisons it declines rather than fails — another diff already running,
 * diff mode left while this one ran, nothing staged — and each of those has a
 * message the reader can act on. Reading only `resp.ok` swallowed every one of
 * them and then called `loadDiff`, which re-served the *previous* comparison:
 * the picker closed, the overlay stayed, and nothing said why.
 */
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
    const body = await resp.json();
    if (body?.success === false) {
      // The server's own words, unwrapped: "Nothing is staged" is not a
      // failure to compute anything and must not read as one.
      diffApiError.set(body.message || 'The diff was declined.');
      return;
    }
    // After successful diff, reload the diff.json
    await loadDiff();
  } catch (err) {
    diffApiError.set(`Failed to compute diff: ${err}`);
  } finally {
    diffComputing.set(false);
  }
}

/** Drop the overlay in this page. Says nothing to the server — see `stopDiff`. */
export function clearDiffOverlay(): void {
  diffActive.set(false);
  diffData.set(null);
  baseDetailsCache.set(null);
  diffApiError.set(null);
}

/**
 * Leave diff mode: tell the server to forget the comparison, then drop the
 * overlay here.
 *
 * The DELETE is the half that matters, and its absence is what left "Current
 * Changes" with no way out (UI-100). Clearing these stores alone leaves the
 * server following the working tree, so the next save pushes a `diff` event
 * that turns the overlay straight back on — and a reload re-fetches the
 * result the server is still serving.
 *
 * Local first, so the button answers immediately rather than after a round
 * trip. The server's own `diff` broadcast arrives shortly after and clears
 * every other window looking at the same engine.
 */
export async function stopDiff(): Promise<void> {
  clearDiffOverlay();
  // Serve mode has no diff endpoint at all (SRV-003), so there is nothing
  // there to stop — and no diff could have been started either.
  if (isServeMode()) return;
  try {
    await fetch(apiUrl('/api/diff'), { method: 'DELETE' });
  } catch {
    // Nothing to report and nothing to retry: the overlay is already gone
    // here, and a server we cannot reach is not recomputing anything for us.
  }
}
