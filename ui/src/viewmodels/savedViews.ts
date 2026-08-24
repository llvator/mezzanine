/**
 * What a **saved view** is, apart from the stores (UI-082).
 *
 * A saved view records *what the canvas is drawing* — a scope, an
 * aggregation level, the kind / language / file filters, the spec
 * cross-filter, a committed search — so a reading worth returning to can be
 * named and restored, and two of them compared by switching between.
 *
 * The line it draws, and every field below sits on one side of it: a saved
 * view holds what decides **which entities and relationships reach the
 * canvas**, and nothing about **where the reader is standing**. So the camera,
 * the selected entity, the expanded scopes, the open panes and the Quality
 * population are all absent — restoring a view should not move the reader's
 * cursor, and a view that reopened a pane would be doing two jobs. Label
 * toggles, hover depth and tree density are absent for the mirror reason:
 * they change how what is drawn *looks*, not what is drawn.
 *
 * Kept pure and store-free so it can be unit-tested (`npm run
 * test:saved-views`) without dragging `displayPlan`, `scope` and the VS Code
 * adapter in behind it; `stores/savedViews.ts` wires it to the stores and to
 * the `/api/views` endpoint.
 */

import type { GraphLevel, LevelOverrides } from '../types/graph';
import type { ScopeRule } from '../utils/scopeRules';

/** The captured picture. Plain JSON — this is what lands in
 *  `.nao/views.json`, so every field has to survive a round trip through a
 *  file a human may have edited. */
export interface ViewState {
  /** ADR 0009 rule list — the visual scope. */
  scope: ScopeRule[];
  level: GraphLevel;
  autoLevel: boolean;
  /** UI-104. The scope the rings are centred on, or null for one grain
   *  everywhere. In here rather than beside `folderCohesion` because it
   *  decides WHICH entities reach the canvas — the line this codec draws —
   *  and a view that restored the level but not the rings would put the
   *  reader's focus back at whatever grain the rest of the graph is at. A
   *  path, so it survives the level change it causes. */
  ringFocus: string | null;
  /** Hops from the focus that stay at Entity grain. */
  ringReach: number;
  entityTypes: string[];
  relTypes: string[];
  outgoing: boolean;
  incoming: boolean;
  levelOverrides: Record<number, LevelOverrides>;
  /** Exclusions, matching the stores: an exclusion is the decision the user
   *  made, and the visible set is derived from the dataset (UI-047). */
  hiddenLanguages: string[];
  hiddenFiles: string[];
  showGhosts: boolean;
  showBuiltinGhosts: boolean;
  showTemplateVars: boolean;
  /** UI-113 — declarations only, or declarations and their insides. Squarely
   *  on the "which entities reach the canvas" side of this codec's line, and
   *  the one field most likely to make two saved views of the same scope look
   *  like different repositories. */
  structureOnly: boolean;
  /** Spec cross-filter — entity ids, ADR 0011. */
  spec: string[];
  searchTerm: string;
  searchIds: string[];
  searchHides: boolean;
}

/** One entry in `.nao/views.json`. Mirrors the server's typed envelope. */
export interface SavedView {
  id: string;
  name: string;
  /** ISO-8601, minted by the browser — the only participant that knows the
   *  reader's clock. */
  saved_at: string;
  state: ViewState;
}

const LEVELS: GraphLevel[] = ['entity', 'file', 'module'];

/** A view of nothing in particular — the shape every field falls back to
 *  when a stored view doesn't mention it. Never restored as-is; it exists so
 *  `normalizeState` can be total. */
export function emptyState(): ViewState {
  return {
    scope: [],
    level: 'entity',
    autoLevel: true,
    ringFocus: null,
    ringReach: 1,
    entityTypes: [],
    relTypes: [],
    outgoing: true,
    incoming: true,
    levelOverrides: {},
    hiddenLanguages: [],
    hiddenFiles: [],
    showGhosts: true,
    showBuiltinGhosts: false,
    showTemplateVars: false,
    structureOnly: true,
    spec: [],
    searchTerm: '',
    searchIds: [],
    searchHides: false,
  };
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

function bool(v: unknown, fallback: boolean): boolean {
  return typeof v === 'boolean' ? v : fallback;
}

function rules(v: unknown): ScopeRule[] {
  if (!Array.isArray(v)) return [];
  return v
    .filter(isRecord)
    .filter((r) => typeof r.pattern === 'string')
    .map((r) => ({ pattern: r.pattern as string, negate: r.negate === true }));
}

const TRI = new Set(['general', 'on', 'off']);

function triMap(v: unknown): Record<string, 'general' | 'on' | 'off'> {
  const out: Record<string, 'general' | 'on' | 'off'> = {};
  if (!isRecord(v)) return out;
  for (const [k, val] of Object.entries(v)) {
    if (typeof val === 'string' && TRI.has(val)) out[k] = val as 'general' | 'on' | 'off';
  }
  return out;
}

function tri(v: unknown): 'general' | 'on' | 'off' {
  return typeof v === 'string' && TRI.has(v) ? (v as 'general' | 'on' | 'off') : 'general';
}

function overrides(v: unknown): Record<number, LevelOverrides> {
  const out: Record<number, LevelOverrides> = {};
  if (!isRecord(v)) return out;
  for (const [k, val] of Object.entries(v)) {
    const level = Number(k);
    if (!Number.isInteger(level) || !isRecord(val)) continue;
    out[level] = {
      enabled: val.enabled === true,
      entityTypes: triMap(val.entityTypes),
      relTypes: triMap(val.relTypes),
      outgoing: tri(val.outgoing),
      incoming: tri(val.incoming),
      peerEdges: val.peerEdges !== false,
    };
  }
  return out;
}

/**
 * Turn anything claiming to be a view state into one.
 *
 * Total by construction: `.nao/views.json` is a file a human can open and
 * edit, and a missing or mistyped field has to degrade to a default rather
 * than throw somewhere deep in a restore that has already half-run.
 */
export function normalizeState(raw: unknown): ViewState {
  const base = emptyState();
  if (!isRecord(raw)) return base;
  const level = raw.level;
  return {
    scope: rules(raw.scope),
    level: typeof level === 'string' && LEVELS.includes(level as GraphLevel)
      ? (level as GraphLevel)
      : base.level,
    autoLevel: bool(raw.autoLevel, base.autoLevel),
    // A hand-edited `''` is not a focus on the repo root, it is an empty
    // string someone left behind — and as a ring focus the two are the
    // opposite of each other, since `''` seeds every entity in the repo.
    ringFocus: typeof raw.ringFocus === 'string' && raw.ringFocus !== '' ? raw.ringFocus : base.ringFocus,
    ringReach: typeof raw.ringReach === 'number' && Number.isFinite(raw.ringReach)
      ? Math.max(0, Math.min(4, Math.floor(raw.ringReach)))
      : base.ringReach,
    entityTypes: strings(raw.entityTypes),
    relTypes: strings(raw.relTypes),
    outgoing: bool(raw.outgoing, base.outgoing),
    incoming: bool(raw.incoming, base.incoming),
    levelOverrides: overrides(raw.levelOverrides),
    hiddenLanguages: strings(raw.hiddenLanguages),
    hiddenFiles: strings(raw.hiddenFiles),
    showGhosts: bool(raw.showGhosts, base.showGhosts),
    showBuiltinGhosts: bool(raw.showBuiltinGhosts, base.showBuiltinGhosts),
    showTemplateVars: bool(raw.showTemplateVars, base.showTemplateVars),
    structureOnly: bool(raw.structureOnly, base.structureOnly),
    spec: strings(raw.spec),
    searchTerm: typeof raw.searchTerm === 'string' ? raw.searchTerm : '',
    searchIds: strings(raw.searchIds),
    searchHides: bool(raw.searchHides, base.searchHides),
  };
}

/** Same tolerance for the list itself. An entry with no id or no name is
 *  dropped rather than repaired — the server refuses to store one, so a file
 *  holding one was hand-written and guessing at the author's intent would be
 *  worse than saying it isn't there. */
export function normalizeViews(raw: unknown): SavedView[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter(isRecord)
    .filter((v) => typeof v.id === 'string' && v.id.trim() !== '')
    .filter((v) => typeof v.name === 'string' && v.name.trim() !== '')
    .map((v) => ({
      id: v.id as string,
      name: (v.name as string).trim(),
      saved_at: typeof v.saved_at === 'string' ? v.saved_at : '',
      state: normalizeState(v.state),
    }));
}

/** What the current dataset still has, for pruning a view saved against an
 *  older analysis. */
export interface Present {
  files: ReadonlySet<string>;
  entityIds: ReadonlySet<string>;
  specIds: ReadonlySet<string>;
}

/** What a restore had to drop because the code moved under it. */
export interface Dropped {
  files: number;
  search: number;
  spec: number;
}

export function nothingDropped(d: Dropped): boolean {
  return d.files === 0 && d.search === 0 && d.spec === 0;
}

/**
 * Drop the parts of a saved view that name things this analysis no longer
 * has, and report how many.
 *
 * Staleness is expected rather than exceptional — files get renamed, entities
 * get deleted, a Feature loses its `cr:` — and a view is worth more partially
 * restored than refused. Only the three id-keyed fields are pruned; scope
 * rules are deliberately left alone, because a rule naming a path that does
 * not exist yet is how you save a view for a branch you are about to check
 * out, and `pruneHiddenFiles` already garbage-collects the file exclusions
 * against the whole repo once they are live.
 *
 * `entityIds` is empty until the graph has loaded, so a caller with no
 * dataset yet passes empty sets and gets the view back untouched — pruning
 * against nothing would report every id as gone.
 */
export function pruneState(state: ViewState, present: Present): { state: ViewState; dropped: Dropped } {
  const keep = <T>(xs: T[], has: ReadonlySet<T>) => (has.size === 0 ? xs : xs.filter((x) => has.has(x)));
  const hiddenFiles = keep(state.hiddenFiles, present.files);
  const searchIds = keep(state.searchIds, present.entityIds);
  const spec = keep(state.spec, present.specIds);
  return {
    state: { ...state, hiddenFiles, searchIds, spec },
    dropped: {
      files: state.hiddenFiles.length - hiddenFiles.length,
      search: state.searchIds.length - searchIds.length,
      spec: state.spec.length - spec.length,
    },
  };
}

/** Sentence for what a restore dropped, or `''` when it dropped nothing. */
export function droppedSummary(d: Dropped): string {
  const parts: string[] = [];
  if (d.files) parts.push(`${d.files} hidden file${d.files > 1 ? 's' : ''}`);
  if (d.search) parts.push(`${d.search} search hit${d.search > 1 ? 's' : ''}`);
  if (d.spec) parts.push(`${d.spec} spec entit${d.spec > 1 ? 'ies' : 'y'}`);
  if (parts.length === 0) return '';
  return `${parts.join(', ')} no longer in the graph — skipped.`;
}

/** Key-sorted JSON, so two states that differ only in insertion order
 *  compare equal. Sets reach here as arrays and are sorted for the same
 *  reason: `new Set(['a','b'])` and `new Set(['b','a'])` are the same filter. */
function canonical(v: unknown): unknown {
  if (Array.isArray(v)) {
    const items = v.map(canonical);
    return items.every((x) => typeof x === 'string')
      ? [...(items as string[])].sort()
      : items;
  }
  if (isRecord(v)) {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(v).sort()) out[k] = canonical(v[k]);
    return out;
  }
  return v;
}

/**
 * Is the canvas currently showing this view?
 *
 * Drives the "you are here" marker in the list, which is what makes a
 * restore verifiable — click a view, see it become the active one — and what
 * tells a reader whether the picture in front of them has drifted from the
 * view they restored.
 */
export function sameState(a: ViewState, b: ViewState): boolean {
  return JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
}

/**
 * A name not already taken, by appending a counter.
 *
 * Names are how a reader picks a view out of a list, so two rows reading
 * `Parsers` would make the list unusable exactly when it has grown enough to
 * need one. Saving over an existing name is a separate, explicit gesture.
 */
export function uniqueName(base: string, taken: Iterable<string>): string {
  const used = new Set([...taken].map((n) => n.toLowerCase()));
  const trimmed = base.trim() || 'View';
  if (!used.has(trimmed.toLowerCase())) return trimmed;
  for (let i = 2; ; i++) {
    const candidate = `${trimmed} ${i}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}

/** A one-line description of what a view holds, for the row's subtitle.
 *  Counts rather than names: the scope is the only part a reader can guess
 *  the shape of from a name, and the rest is worth a glance before restoring. */
export function stateSummary(state: ViewState): string {
  const parts: string[] = [];
  const includes = state.scope.filter((r) => !r.negate);
  if (includes.length === 0) parts.push('no scope');
  else if (includes.length === 1) parts.push(includes[0].pattern === '' ? 'whole repo' : includes[0].pattern);
  else parts.push(`${includes.length} paths`);
  // With a focus the level names only the OUTER grain, so printing it alone
  // would describe a picture the view does not hold — the row would read
  // `module` for a view whose whole point is the entities in the middle.
  parts.push(state.autoLevel ? `${state.level} (auto)` : state.level);
  if (state.ringFocus !== null) parts.push(`focus ${state.ringFocus} +${state.ringReach}`);
  if (state.hiddenFiles.length) parts.push(`${state.hiddenFiles.length} files hidden`);
  if (state.hiddenLanguages.length) parts.push(`${state.hiddenLanguages.length} langs hidden`);
  if (state.spec.length) parts.push(`spec ×${state.spec.length}`);
  if (state.searchIds.length) parts.push(`search “${state.searchTerm}”`);
  return parts.join(' · ');
}
