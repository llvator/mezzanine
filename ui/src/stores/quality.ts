import { derived, writable } from 'svelte/store';
import type { D3Node, GraphData, EntityMetrics, ScopeMetrics, BackendThresholds } from '../types/graph';
import { graphData, rawEntityGraph } from './graph';
import { diffData } from './diff';
import { analysisGraphData } from './scope';
import { displayPlan } from '../viewmodels/displayPlan';

/** Where the Quality view pulls entities from:
 *    'scope'            — entities in the Analysis Scope tree (default)
 *    'visualScope'      — entities in the Visual Scopes tree (pre-filter)
 *    'visualSelection'  — entities actually drawn on screen right now
 *                         (post-filter, post-search, post-level-aggregation)
 *    'currentFile'      — only entities in the file focused in the editor
 *    'changedFiles'     — only entities in files that differ from HEAD (diff).
 *  When the required signal is missing (no file focus / no diff loaded /
 *  empty visual scope / empty visual selection), the filter falls back to
 *  'scope'. */
export type QualityAnalysisScope =
  | 'scope'
  | 'visualScope'
  | 'visualSelection'
  | 'currentFile'
  | 'changedFiles';

/** Which metric the Quality row list is ranked by (always descending —
 *  biggest/worst/most-central at the top). */
export type QualitySortKey =
  | 'score'       // Composite refactor pressure (default)
  | 'pagerank'
  | 'cc'
  | 'cognitive'
  | 'nesting'
  | 'loc'
  | 'params'
  | 'fanIn'
  | 'fanOut'
  | 'wmc'
  | 'chainDepth'
  | 'methodCount'
  | 'fieldCount';
export const qualitySortBy = writable<QualitySortKey>('score');
export const qualityAnalysisScope = writable<QualityAnalysisScope>('scope');

/** The file currently focused in the editor (workspace-relative path).
 *  Updated by the VS Code extension on editor / cursor changes. Null in
 *  standalone mode and before the first focus event. */
export const currentEditorFile = writable<string | null>(null);

/** Graph data filtered to the currently chosen analysis scope. Starts
 *  from `analysisGraphData` (the full graph sliced by the *analysis*
 *  scope — which can be wider than the visual scope — NOT from
 *  `graphData`, which is the visually-scoped subset). The per-file /
 *  changed-files narrowing is then layered on top.
 *
 *  Quality metrics, the aggregate summary, and the row list all derive
 *  from here so the user can analyze a scope bigger than the one drawn. */
export const analysisGraph = derived(
  [analysisGraphData, rawEntityGraph, graphData, displayPlan, qualityAnalysisScope, currentEditorFile, diffData],
  ([$g, $visual, $shown, $plan, $scope, $file, $diff]): GraphData => {
    // Visual scope — reuse rawEntityGraph which applySelection already
    // populates with the visual-scope-filtered entities (no graphLevel
    // collapsing applied yet, so we have raw entities).
    if ($scope === 'visualScope') {
      return $visual.nodes.length > 0 ? $visual : $g;
    }
    // Visual selection — only the nodes actually drawn on the canvas
    // right now. Respects every filter, search, level aggregation, and
    // selection-distance gating applied by displayPlan.
    if ($scope === 'visualSelection') {
      const ids = $plan?.visibleNodeIds;
      if (ids && ids.size > 0) {
        return { ...$shown, nodes: $shown.nodes.filter((n) => ids.has(n.id)) };
      }
      return $g;
    }
    if ($scope === 'currentFile' && $file) {
      const f = $file;
      return { ...$g, nodes: $g.nodes.filter((n) => n.file_path === f) };
    }
    if ($scope === 'changedFiles' && $diff) {
      // "Core" changes only — mirrors what `scopeToChangedFiles` uses.
      const changed = new Set(
        $diff.entities
          .filter((e) =>
            e.status === 'added' || e.status === 'removed'
            || (e.status === 'modified' && e.source_changed === true)
          )
          .map((e) => e.file_path)
          .filter((p): p is string => typeof p === 'string' && p.length > 0),
      );
      return { ...$g, nodes: $g.nodes.filter((n) => changed.has(n.file_path)) };
    }
    return $g;
  },
);

/** Severity tier for a single metric value. */
export type Tier = 'ok' | 'warn' | 'bad' | 'na';

/** Fallback thresholds used when the backend hasn't provided them (e.g.,
 * loading older data). The backend's `Thresholds::default()` produces
 * identical values — this is the safety net, not the source of truth. */
const FALLBACK_THRESHOLDS = {
  cc: { warn: 10, bad: 20 },
  cognitive: { warn: 8, bad: 15 },
  nest: { warn: 3, bad: 5 },
  locCallable: { warn: 30, bad: 60 },
  locContainer: { warn: 100, bad: 200 },
  params: { warn: 4, bad: 6 },
  fanOut: { warn: 7, bad: 15 },
  fields: { warn: 8, bad: 15 },
  variants: { warn: 6, bad: 12 },
  methodCount: { warn: 15, bad: 25 },
  publicFieldRatio: { warn: 0.5, bad: 0.8 },
  fileEntityCount: { warn: 15, bad: 30 },
  moduleEntityCount: { warn: 60, bad: 150 },
  fileLoc: { warn: 400, bad: 800 },
  moduleLoc: { warn: 2000, bad: 5000 },
  fileFanOut: { warn: 10, bad: 20 },
  moduleFanOut: { warn: 15, bad: 30 },
  cohesion: { warn: 0.6, bad: 0.3 },
};

/** Map backend snake_case threshold keys to frontend camelCase. */
function resolveThresholds(bt?: BackendThresholds): typeof FALLBACK_THRESHOLDS {
  if (!bt) return FALLBACK_THRESHOLDS;
  return {
    cc: bt.cc ?? FALLBACK_THRESHOLDS.cc,
    cognitive: bt.cognitive ?? FALLBACK_THRESHOLDS.cognitive,
    nest: bt.nest ?? FALLBACK_THRESHOLDS.nest,
    locCallable: bt.loc_callable ?? FALLBACK_THRESHOLDS.locCallable,
    locContainer: bt.loc_container ?? FALLBACK_THRESHOLDS.locContainer,
    params: bt.params ?? FALLBACK_THRESHOLDS.params,
    fanOut: bt.fan_out ?? FALLBACK_THRESHOLDS.fanOut,
    fields: bt.fields ?? FALLBACK_THRESHOLDS.fields,
    variants: bt.variants ?? FALLBACK_THRESHOLDS.variants,
    methodCount: bt.method_count ?? FALLBACK_THRESHOLDS.methodCount,
    publicFieldRatio: bt.public_field_ratio ?? FALLBACK_THRESHOLDS.publicFieldRatio,
    fileEntityCount: bt.file_entity_count ?? FALLBACK_THRESHOLDS.fileEntityCount,
    moduleEntityCount: bt.module_entity_count ?? FALLBACK_THRESHOLDS.moduleEntityCount,
    fileLoc: bt.file_loc ?? FALLBACK_THRESHOLDS.fileLoc,
    moduleLoc: bt.module_loc ?? FALLBACK_THRESHOLDS.moduleLoc,
    fileFanOut: bt.file_fan_out ?? FALLBACK_THRESHOLDS.fileFanOut,
    moduleFanOut: bt.module_fan_out ?? FALLBACK_THRESHOLDS.moduleFanOut,
    cohesion: bt.cohesion ?? FALLBACK_THRESHOLDS.cohesion,
  };
}

/** Resolved thresholds — reads from backend when available, falls back to
 * hardcoded defaults for older data. All tier functions read from this. */
let _cachedThresholds = FALLBACK_THRESHOLDS;
export const thresholdStore = derived(graphData, ($g) => {
  _cachedThresholds = resolveThresholds($g.thresholds);
  return _cachedThresholds;
});

// Eagerly subscribe so _cachedThresholds stays current.
thresholdStore.subscribe(() => {});

/** Current thresholds. Use this in non-reactive contexts (tier functions).
 * In reactive contexts, subscribe to `thresholdStore` instead. */
export const THRESHOLDS = new Proxy(FALLBACK_THRESHOLDS, {
  get(_target, prop) {
    return (_cachedThresholds as any)[prop];
  },
});

/**
 * Which threshold key backs each summary-table row, and the unit its numbers
 * are in. Drives the per-metric threshold display: the table showed
 * ok/warn/bad *counts* with the boundaries that produced them never stated,
 * so "132 warn, 141 bad" could not be acted on (UI-018).
 *
 * `loc` and `fields` have separate callable/container variants; the summary
 * row aggregates both, so both are shown.
 */
export const SUMMARY_METRIC_THRESHOLDS: Record<string, { keys: string[]; unit: string }> = {
  cc:               { keys: ['cc'], unit: 'paths' },
  cognitive:        { keys: ['cognitive'], unit: 'points' },
  nest:             { keys: ['nest'], unit: 'levels' },
  loc:              { keys: ['locCallable', 'locContainer'], unit: 'lines' },
  params:           { keys: ['params'], unit: 'params' },
  fanOut:           { keys: ['fanOut'], unit: 'deps' },
  fieldCount:       { keys: ['fields', 'variants'], unit: 'fields' },
  methodCount:      { keys: ['methodCount'], unit: 'methods' },
  publicFieldRatio: { keys: ['publicFieldRatio'], unit: 'ratio' },
};

/** Short plain-English explanation of each metric for the UI tooltip.
 * Kept terse: this is the elevator pitch, not the full docs. The full
 * guidance lives in the Quality section of the top-level README. */
export const METRIC_EXPLANATIONS: Record<string, { title: string; body: string }> = {
  score: {
    title: 'Composite score',
    body:
      'Weighted sum of the other metrics (CC, Nest, Fan-out, LOC) plus a bonus when the entity is in a cycle. ' +
      'Roughly 0 = healthy, 1 = at red threshold, >1 = over the line. Use it as a "refactor-pressure" ranking, not a verdict.',
  },
  cognitive: {
    title: 'Cognitive complexity',
    body:
      'Like cyclomatic complexity but weights each branch by its nesting depth. A flat guard clause costs +1; '
      + 'the same branch nested inside a for-loop inside a match costs +4. Better at measuring "how hard is this to understand." '
      + '≤8 healthy, ≤15 worth a look, >15 usually means flatten with early returns or extract helpers.',
  },
  cc: {
    title: 'Cyclomatic complexity',
    body:
      'Number of independent paths through the function (branches + 1). Every if/match-arm/loop/?/&&/|| adds one. ' +
      '≤10 healthy, ≤20 worth a look, >20 usually means split the function into smaller pieces.',
  },
  nest: {
    title: 'Max nesting depth',
    body:
      'Deepest level of nested control flow. Deep nesting forces the reader to hold many conditions in mind. ' +
      '≤3 healthy, ≤5 amber, >5 usually means early-returns or extracted helpers would help.',
  },
  loc: {
    title: 'Lines of code',
    body:
      'Inclusive line span of the entity. Weakest signal on its own — a long but flat function can be fine. ' +
      'Pair with CC and Fan-out before reacting. Callable threshold: green ≤30, amber ≤60, red >60.',
  },
  params: {
    title: 'Parameter count',
    body:
      'Number of parameters (self excluded). Long lists usually mean a missing abstraction (group related params into a struct) ' +
      'or a function doing too much. Green ≤4, amber ≤6, red >6.',
  },
  fan_in: {
    title: 'Fan-in',
    body:
      'Number of distinct entities that depend on this one. High fan-in is healthy for stable utilities — it means they are reused. ' +
      'It becomes risky only when the code also changes frequently, because every change ripples through all callers.',
  },
  fan_out: {
    title: 'Fan-out',
    body:
      'Number of distinct entities this one depends on. The classic god-object / orchestrator smell. ' +
      'Green ≤7, amber ≤15, red >15. Remedy: introduce a narrow seam (trait/facade) so the caller depends on one port instead of many.',
  },
  cycle: {
    title: 'In cycle',
    body:
      'True if this entity participates in a dependency cycle (strongly connected component). Cycles make modules impossible to ' +
      'understand in isolation and break layering. Typical fix: dependency inversion — both sides depend on an interface instead of each other.',
  },
  field_count: {
    title: 'Field / variant count',
    body:
      'Structs: number of fields (≤8 healthy, ≤15 amber, >15 red). A struct with 25 fields is usually several types pretending to be one. ' +
      'Enums: number of variants (≤6 / ≤12 / >12). Very wide enums often should be traits or split by concern.',
  },
  method_count: {
    title: 'Method count',
    body:
      'Methods directly contained in the type or module. Surface-area proxy. High method count combined with high field count is the ' +
      'classic "blob class" pattern. ≤15 healthy, ≤25 amber, >25 red — consider splitting by responsibility.',
  },
  public_field_ratio: {
    title: 'Public field ratio',
    body:
      'Share of fields marked `pub`. Only scored on structs that also have methods — a pure data record with all-public fields is ' +
      'intentional and fine. When the struct has behavior, high ratios (>50% amber, >80% red) indicate weak encapsulation: callers can bypass the API.',
  },
  entity_count: {
    title: 'Entity count',
    body:
      'Number of entities (structs, functions, etc.) declared in this file or module. High counts indicate a grab-bag file that '
      + "probably wants splitting. File thresholds: ≤15 healthy, ≤30 amber, >30 red. Module thresholds: ≤60 / ≤150.",
  },
  scope_loc: {
    title: 'Lines of code (scope)',
    body:
      'Total LOC in the file or module. File thresholds: ≤400 / ≤800. Module thresholds: ≤2000 / ≤5000. Weak signal alone — pair '
      + 'with entity count and cohesion.',
  },
  cohesion: {
    title: 'Cohesion',
    body:
      'Fraction of dependency edges that stay inside this file/module (internal / total). High cohesion = a tight module; low cohesion '
      + '= a grab-bag. ≥60% healthy, ≥30% amber, <30% red. Very low values usually mean the scope should be split or re-grouped.',
  },
  scope_fan_in: {
    title: 'Scope fan-in',
    body:
      'Distinct other files/modules depending on this one. High fan-in is healthy for stable core scopes; risky only when the scope '
      + 'also changes frequently.',
  },
  scope_fan_out: {
    title: 'Scope fan-out',
    body:
      'Distinct other files/modules this one depends on. Classic architectural smell when very high — indicates a leaky or '
      + 'orchestrating scope. File thresholds: ≤10 / ≤20. Module thresholds: ≤15 / ≤30.',
  },
  scope_cycle: {
    title: 'In cycle (scope)',
    body:
      'True when this file or module participates in a cross-scope dependency cycle. Usually a layering violation — a strong signal '
      + 'to apply dependency inversion at the architecture boundary.',
  },
  aggregated_quality: {
    title: 'Aggregated entity quality',
    body:
      'Average composite score of all entities living inside this scope (file or folder). '
      + 'Surfaces entity-level quality problems at a higher granularity. '
      + 'Green ≤0.5 (most entities healthy), amber ≤1 (some trouble), red >1 (widespread issues). '
      + 'The ok/warn/bad counts show how many entities fall into each tier.',
  },
  instability: {
    title: 'Instability index',
    body:
      'fan_out / (fan_in + fan_out). 0 = maximally stable (only depended on), 1 = maximally unstable (only depends on others). '
      + 'Not inherently good or bad — it depends on the entity\'s role. Stable entities should have stable interfaces; '
      + 'unstable ones can change freely since nothing depends on them.',
  },
  return_complexity: {
    title: 'Return type complexity',
    body:
      'Number of elements in the return type\'s outermost tuple. A function returning (A, B, C, D, E) scores 5 — '
      + 'a strong signal that a named struct should replace the tuple. ≤2 fine, 3 worth a look, >3 usually means extract a struct.',
  },
  smells: {
    title: 'Code smells',
    body:
      'Anti-pattern signals detected by combining multiple metrics. Each smell points to a '
      + 'specific design problem and suggests which pattern would fix it.',
  },
};

/** Metadata for each smell kind. Keyed by the snake_case string the backend emits. */
export const SMELL_META: Record<string, { label: string; hint: string }> = {
  god_class: {
    label: 'God Class',
    hint: 'Too many fields, methods, and dependencies — split by responsibility (Facade, Decorator).',
  },
  dispatcher: {
    label: 'Dispatcher',
    hint: 'High branching that just routes to other functions — replace with Strategy, Command, or Chain of Responsibility.',
  },
  feature_envy: {
    label: 'Feature Envy',
    hint: 'Most outgoing dependencies target one foreign type — move this method there or extract shared logic.',
  },
  shotgun_surgery: {
    label: 'Shotgun Surgery',
    hint: 'Very high fan-in — any change here ripples widely. Stabilise the interface or apply dependency inversion.',
  },
};

const CALLABLE_KINDS = new Set(['Function', 'Method']);

export function isCallable(kindRaw: string): boolean {
  return CALLABLE_KINDS.has(kindRaw);
}

export function tierCC(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v <= THRESHOLDS.cc.warn) return 'ok';
  if (v <= THRESHOLDS.cc.bad) return 'warn';
  return 'bad';
}
export function tierCognitive(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v <= THRESHOLDS.cognitive.warn) return 'ok';
  if (v <= THRESHOLDS.cognitive.bad) return 'warn';
  return 'bad';
}
export function tierNest(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v <= THRESHOLDS.nest.warn) return 'ok';
  if (v <= THRESHOLDS.nest.bad) return 'warn';
  return 'bad';
}
export function tierLoc(v: number, callable: boolean): Tier {
  const t = callable ? THRESHOLDS.locCallable : THRESHOLDS.locContainer;
  if (v <= t.warn) return 'ok';
  if (v <= t.bad) return 'warn';
  return 'bad';
}
export function tierParams(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v <= THRESHOLDS.params.warn) return 'ok';
  if (v <= THRESHOLDS.params.bad) return 'warn';
  return 'bad';
}
export function tierFanOut(v: number): Tier {
  if (v <= THRESHOLDS.fanOut.warn) return 'ok';
  if (v <= THRESHOLDS.fanOut.bad) return 'warn';
  return 'bad';
}
/** Field / variant count tier. `isEnum` flips to the enum-specific bounds,
 * which are stricter since enums usually want fewer arms than structs want fields. */
export function tierFieldCount(v: number | undefined, isEnum: boolean): Tier {
  if (v == null) return 'na';
  const t = isEnum ? THRESHOLDS.variants : THRESHOLDS.fields;
  if (v <= t.warn) return 'ok';
  if (v <= t.bad) return 'warn';
  return 'bad';
}
export function tierMethodCount(v: number): Tier {
  if (v <= THRESHOLDS.methodCount.warn) return 'ok';
  if (v <= THRESHOLDS.methodCount.bad) return 'warn';
  return 'bad';
}
/** Public-field ratio tier. Only meaningful when the struct has behavior —
 * a pure data record with 100% pub fields is fine. Callers should skip the
 * tier (render as informational) when `method_count === 0`. */
export function tierPublicFieldRatio(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v <= THRESHOLDS.publicFieldRatio.warn) return 'ok';
  if (v <= THRESHOLDS.publicFieldRatio.bad) return 'warn';
  return 'bad';
}

// --- Scope-level tiers ---

export function tierEntityCount(v: number, isModule: boolean): Tier {
  const t = isModule ? THRESHOLDS.moduleEntityCount : THRESHOLDS.fileEntityCount;
  if (v <= t.warn) return 'ok';
  if (v <= t.bad) return 'warn';
  return 'bad';
}
export function tierScopeLoc(v: number, isModule: boolean): Tier {
  const t = isModule ? THRESHOLDS.moduleLoc : THRESHOLDS.fileLoc;
  if (v <= t.warn) return 'ok';
  if (v <= t.bad) return 'warn';
  return 'bad';
}
export function tierScopeFanOut(v: number, isModule: boolean): Tier {
  const t = isModule ? THRESHOLDS.moduleFanOut : THRESHOLDS.fileFanOut;
  if (v <= t.warn) return 'ok';
  if (v <= t.bad) return 'warn';
  return 'bad';
}
/** Cohesion tier — LOWER is worse, so comparisons are inverted. A scope
 * with no dependency edges at all has no meaningful cohesion and returns 'na'. */
export function tierCohesion(v: number | undefined): Tier {
  if (v == null) return 'na';
  if (v >= THRESHOLDS.cohesion.warn) return 'ok';
  if (v >= THRESHOLDS.cohesion.bad) return 'warn';
  return 'bad';
}

/** Normalize a metric against its red threshold. Values above 1 indicate
 * "over the red line"; clamped at 2 so a single runaway metric can't
 * dominate the composite score. */
function norm(v: number | undefined, red: number): number {
  if (v == null) return 0;
  return Math.min(v / red, 2);
}

/** Composite "refactor pressure" score in roughly [0, 2]. Weighted sum of
 * normalized metrics + cycle bonus. Weights chosen from the composition
 * table in the Quality section of the top-level README (fan-out and CC are
 * the strongest refactor signals). Tunable — the numbers aren't load-bearing. */
export function compositeScore(m: EntityMetrics, kindRaw: string): number {
  const callable = isCallable(kindRaw);
  const cycle = m.in_cycle ? 0.5 : 0;

  if (callable) {
    const cc = norm(m.cyclomatic, THRESHOLDS.cc.bad);
    const cog = norm(m.cognitive_complexity, THRESHOLDS.cognitive.bad);
    const nest = norm(m.max_nesting, THRESHOLDS.nest.bad);
    const loc = norm(m.loc, THRESHOLDS.locCallable.bad);
    const fo = norm(m.fan_out, THRESHOLDS.fanOut.bad);
    const params = norm(m.param_count, THRESHOLDS.params.bad);
    return 0.25 * cc + 0.15 * cog + 0.25 * fo + 0.15 * loc + 0.1 * params + 0.1 * nest + cycle;
  }

  // For containers (structs/enums/traits/modules): LOC is a weak signal,
  // so we lean on fields/variants, methods, fan-out, and — for structs
  // with behavior — the public-field ratio (leaky encapsulation).
  const isEnum = kindRaw === 'Enum';
  const fieldRed = isEnum ? THRESHOLDS.variants.bad : THRESHOLDS.fields.bad;
  const fields = norm(m.field_count, fieldRed);
  const methods = norm(m.method_count, THRESHOLDS.methodCount.bad);
  const loc = norm(m.loc, THRESHOLDS.locContainer.bad);
  const fo = norm(m.fan_out, THRESHOLDS.fanOut.bad);
  // Public-field ratio only contributes when the struct has methods
  // (behavior). A data-bag record with no methods is intentional and
  // should not be penalised.
  let encaps = 0;
  if (m.method_count > 3 && m.public_field_ratio != null) {
    encaps = norm(m.public_field_ratio, THRESHOLDS.publicFieldRatio.bad);
  }
  return 0.3 * fields + 0.2 * methods + 0.15 * loc + 0.15 * fo + 0.2 * encaps + cycle;
}

export interface QualityRow {
  node: D3Node;
  score: number;
  tiers: {
    cc: Tier;
    cognitive: Tier;
    nest: Tier;
    loc: Tier;
    params: Tier;
    fanOut: Tier;
    fieldCount: Tier;
    methodCount: Tier;
    publicFieldRatio: Tier;
  };
}

export interface TierCounts {
  ok: number;
  warn: number;
  bad: number;
  na: number;
}

export interface QualitySummary {
  total: number;
  cc: TierCounts;
  cognitive: TierCounts;
  nest: TierCounts;
  loc: TierCounts;
  params: TierCounts;
  fanOut: TierCounts;
  fieldCount: TierCounts;
  methodCount: TierCounts;
  publicFieldRatio: TierCounts;
  inCycle: number;
}

function emptyCounts(): TierCounts {
  return { ok: 0, warn: 0, bad: 0, na: 0 };
}

/** Rows derived from the current graph scope. Computed once per graphData
 * change — sorting/filtering in the UI reads this snapshot without
 * recomputing per interaction. */
export const qualityRows = derived(analysisGraph, ($g): QualityRow[] => {
  const rows: QualityRow[] = [];
  for (const node of $g.nodes) {
    const m = node.metrics;
    if (!m) continue;
    // Skip synthetic parameter nodes — they'd drown the report in noise.
    if (node.kind_raw === 'Parameter') continue;
    const callable = isCallable(node.kind_raw);
    const isEnum = node.kind_raw === 'Enum';
    // Public-field ratio is informational (not scored/tiered) when the
    // struct has no behavior — a plain data record intentionally exposes
    // its fields. Only emit a tier when method_count > 3.
    const pfrTier =
      !callable && m.method_count > 3
        ? tierPublicFieldRatio(m.public_field_ratio)
        : 'na';
    rows.push({
      node,
      score: m.composite_score ?? compositeScore(m, node.kind_raw),
      tiers: {
        cc: tierCC(m.cyclomatic),
        cognitive: tierCognitive(m.cognitive_complexity),
        nest: tierNest(m.max_nesting),
        loc: tierLoc(m.loc, callable),
        params: tierParams(m.param_count),
        fanOut: tierFanOut(m.fan_out),
        fieldCount: callable ? 'na' : tierFieldCount(m.field_count, isEnum),
        methodCount: callable ? 'na' : tierMethodCount(m.method_count),
        publicFieldRatio: pfrTier,
      },
    });
  }
  return rows;
});

// --- Scope-level rows (files & modules) ---

export interface ScopeRow {
  scope: ScopeMetrics;
  score: number;
  tiers: {
    entity: Tier;
    loc: Tier;
    cohesion: Tier;
    fanOut: Tier;
  };
}

/** Minimum entity count for cohesion to be meaningful. Files with fewer
 * entities have too small a sample for cohesion to signal a real problem. */
const MIN_ENTITIES_FOR_COHESION = 5;

/** File names that are wiring / entry-point by nature — their low cohesion
 * is structural (connecting modules), not a grab-bag smell. */
const WIRING_FILES = new Set(['mod.rs', 'main.rs', 'lib.rs', 'index.ts', 'index.js', 'mod.ts', '__init__.py']);

function isWiringFile(path: string): boolean {
  const name = path.includes('/') ? path.slice(path.lastIndexOf('/') + 1) : path;
  return WIRING_FILES.has(name);
}

/** Detect stable data-model files: high fan-in, near-zero fan-out,
 * mostly containers (structs/enums). These legitimately have 0% cohesion
 * because their types are consumed elsewhere, not by each other. */
function isStableDataModel(s: ScopeMetrics): boolean {
  const inst = s.instability ?? 1;
  return inst <= 0.15
    && s.fan_in >= 3
    && s.container_count >= 2
    && s.container_count >= s.callable_count * 0.3;
}

/** Determine whether cohesion is a meaningful signal for this scope and,
 * if so, what weight it should carry. Returns `{ meaningful, weight }`. */
function cohesionContext(s: ScopeMetrics): { meaningful: boolean; weight: number } {
  if (s.cohesion == null || s.entity_count < MIN_ENTITIES_FOR_COHESION) {
    return { meaningful: false, weight: 0 };
  }
  if (isStableDataModel(s)) return { meaningful: false, weight: 0 };
  if (isWiringFile(s.path)) return { meaningful: true, weight: 0.12 };
  return { meaningful: true, weight: 0.25 };
}

/** Compute the raw cohesion penalty from the cohesion ratio. */
function cohesionPenalty(cohesionValue: number): number {
  const red = THRESHOLDS.cohesion.bad;
  if (cohesionValue < red) return Math.min((red - cohesionValue) / red, 1) * 2;
  if (cohesionValue < THRESHOLDS.cohesion.warn) return 0.5;
  return 0;
}

/** Composite "refactor pressure" for a file or module. Balanced across
 * bloat (entity_count + LOC), coupling (fan-out), cohesion (inverted),
 * and cycle membership.
 *
 * Context-aware adjustments to reduce false positives:
 * - Cohesion is suppressed when entity_count < 5 (sample too small).
 * - Cohesion weight is halved for entry-point / wiring files (mod.rs,
 *   main.rs, etc.) whose job is connecting modules.
 * - Cohesion is suppressed for stable data-model files (high fan-in,
 *   near-zero fan-out, mostly containers) — their 0% cohesion is by
 *   design since their types are consumed elsewhere, not by each other.
 * - LOC penalty is halved when cohesion > 80% — a large focused module
 *   is not the same problem as a large fragmented one.
 *
 * Tunable — not load-bearing. */
export function scopeCompositeScore(s: ScopeMetrics, isModule: boolean): number {
  const entityRed = isModule ? THRESHOLDS.moduleEntityCount.bad : THRESHOLDS.fileEntityCount.bad;
  const locRed = isModule ? THRESHOLDS.moduleLoc.bad : THRESHOLDS.fileLoc.bad;
  const foRed = isModule ? THRESHOLDS.moduleFanOut.bad : THRESHOLDS.fileFanOut.bad;
  const entity = Math.min(s.entity_count / entityRed, 2);
  let loc = Math.min(s.loc / locRed, 2);
  const fo = Math.min(s.fan_out / foRed, 2);

  const ctx = cohesionContext(s);
  const coh = ctx.meaningful ? cohesionPenalty(s.cohesion!) : 0;

  // LOC penalty halved when cohesion is high (large focused module ≠ large grab-bag).
  if (ctx.meaningful && s.cohesion! >= 0.8) {
    loc *= 0.5;
  }

  const cycle = s.in_cycle ? 0.5 : 0;
  const baseWeight = 0.25 + 0.2 + 0.25 + ctx.weight;
  const norm = baseWeight > 0 ? 1 / baseWeight : 1;
  return (0.25 * entity + 0.2 * loc + 0.25 * fo + ctx.weight * coh) * norm + cycle;
}

function scopeTiers(s: ScopeMetrics, isModule: boolean): ScopeRow['tiers'] {
  const ctx = cohesionContext(s);
  return {
    entity: tierEntityCount(s.entity_count, isModule),
    loc: tierScopeLoc(s.loc, isModule),
    cohesion: ctx.meaningful ? tierCohesion(s.cohesion) : 'na',
    fanOut: tierScopeFanOut(s.fan_out, isModule),
  };
}

export const fileRows = derived(graphData, ($g): ScopeRow[] =>
  ($g.files ?? []).map((f) => ({
    scope: f,
    score: f.composite_score ?? scopeCompositeScore(f, false),
    tiers: scopeTiers(f, false),
  })),
);

export const moduleRows = derived(graphData, ($g): ScopeRow[] =>
  ($g.modules ?? []).map((m) => ({
    scope: m,
    score: m.composite_score ?? scopeCompositeScore(m, true),
    tiers: scopeTiers(m, true),
  })),
);

// --- Aggregated quality scores (repo / folder / file level) ---

/** Rolled-up quality picture for a scope (repo, folder, or file). Built by
 * aggregating the per-entity composite scores of every entity living inside
 * the scope's files. */
export interface AggregatedScore {
  /** Mean composite score across all entities in scope. */
  avgScore: number;
  /** Highest (worst) composite score in scope. */
  maxScore: number;
  /** Total entities contributing. */
  entityCount: number;
  /** Entities whose composite score is ≤ 0.5 (roughly "green"). */
  okCount: number;
  /** Entities whose composite score is in (0.5, 1.0] ("amber zone"). */
  warnCount: number;
  /** Entities whose composite score is > 1.0 ("red zone"). */
  badCount: number;
  /** Entities participating in a dependency cycle. */
  cycleCount: number;
  /** Fraction of entities in the red zone (badCount / entityCount). */
  badRatio: number;
  /** Overall tier derived from avgScore. */
  tier: Tier;
}

/** Thresholds for mapping an average composite score to a tier. The numbers
 * align with the entity-level scale: 0–0.5 green, 0.5–1.0 amber, >1.0 red. */
/**
 * Composite-score tier boundaries — the one definition.
 *
 * These were also written out longhand at the editor-host broadcast site as
 * `score > 1.0 ? 'bad' : score > 0.5 ? 'warn' : 'ok'`, so the panel and the
 * native views each carried their own copy of the same two numbers with
 * nothing keeping them in step (UI-018). Exported so both read from here, and
 * so the UI can state the scale instead of leaving the reader to guess
 * whether higher is better.
 */
export const SCORE_TIERS = { ok: 0.5, warn: 1.0 } as const;

/** Human-readable statement of the scale, for display next to the score. */
export const SCORE_SCALE_LABEL = `0 – 1+ · lower is better`;

export function tierFromScore(score: number): Tier {
  if (score <= SCORE_TIERS.ok) return 'ok';
  if (score <= SCORE_TIERS.warn) return 'warn';
  return 'bad';
}

function tierFromAvg(avg: number): Tier {
  return tierFromScore(avg);
}

/** Aggregate a set of quality rows into a single `AggregatedScore`.
 * Uses LOC-weighted averaging so larger entities contribute proportionally
 * more — small healthy getters don't mask large complex functions. */
export function aggregateRows(rows: QualityRow[]): AggregatedScore {
  if (rows.length === 0) {
    return {
      avgScore: 0, maxScore: 0, entityCount: 0,
      okCount: 0, warnCount: 0, badCount: 0, cycleCount: 0,
      badRatio: 0, tier: 'na',
    };
  }
  let weightedSum = 0;
  let totalLoc = 0;
  let max = 0;
  let ok = 0;
  let warn = 0;
  let bad = 0;
  let cycles = 0;
  for (const r of rows) {
    const loc = Math.max(r.node.metrics?.loc ?? 1, 1);
    weightedSum += r.score * loc;
    totalLoc += loc;
    if (r.score > max) max = r.score;
    if (r.score <= 0.5) ok++;
    else if (r.score <= 1) warn++;
    else bad++;
    if (r.node.metrics?.in_cycle) cycles++;
  }
  const avg = totalLoc > 0 ? weightedSum / totalLoc : 0;
  return {
    avgScore: avg,
    maxScore: max,
    entityCount: rows.length,
    okCount: ok,
    warnCount: warn,
    badCount: bad,
    cycleCount: cycles,
    badRatio: bad / rows.length,
    tier: tierFromAvg(avg),
  };
}

/** Repo-level aggregated quality: every entity in the current scope. */
export const repoQuality = derived(qualityRows, ($rows): AggregatedScore =>
  aggregateRows($rows),
);

/** Convert a backend `ScopeMetrics` entry (which already carries aggregated
 * quality fields computed in Rust) into the frontend `AggregatedScore` shape. */
export function scopeToAggregated(s: ScopeMetrics): AggregatedScore {
  const qOk = s.quality_ok ?? 0;
  const qWarn = s.quality_warn ?? 0;
  const qBad = s.quality_bad ?? 0;
  const avg = s.avg_quality ?? 0;
  const count = qOk + qWarn + qBad;
  return {
    avgScore: avg,
    maxScore: s.max_quality ?? 0,
    entityCount: count,
    okCount: qOk,
    warnCount: qWarn,
    badCount: qBad,
    cycleCount: 0, // cycle count per entity not tracked at scope level
    badRatio: count > 0 ? qBad / count : 0,
    tier: count === 0 ? 'na' : tierFromAvg(avg),
  };
}

export const qualitySummary = derived(qualityRows, ($rows): QualitySummary => {
  const s: QualitySummary = {
    total: $rows.length,
    cc: emptyCounts(),
    cognitive: emptyCounts(),
    nest: emptyCounts(),
    loc: emptyCounts(),
    params: emptyCounts(),
    fanOut: emptyCounts(),
    fieldCount: emptyCounts(),
    methodCount: emptyCounts(),
    publicFieldRatio: emptyCounts(),
    inCycle: 0,
  };
  for (const r of $rows) {
    s.cc[r.tiers.cc]++;
    s.cognitive[r.tiers.cognitive]++;
    s.nest[r.tiers.nest]++;
    s.loc[r.tiers.loc]++;
    s.params[r.tiers.params]++;
    s.fanOut[r.tiers.fanOut]++;
    s.fieldCount[r.tiers.fieldCount]++;
    s.methodCount[r.tiers.methodCount]++;
    s.publicFieldRatio[r.tiers.publicFieldRatio]++;
    if (r.node.metrics?.in_cycle) s.inCycle++;
  }
  return s;
});
