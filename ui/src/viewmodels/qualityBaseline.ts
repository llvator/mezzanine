/**
 * The repo's own middle, so a single score can be read as high or low.
 *
 * A composite score printed alone is a number without a frame: 0.42 is only
 * "fine" or "bad" against what the rest of the repo scores, and that varies
 * enormously between a parser crate and a Svelte UI. This module computes the
 * mean score of every file, every folder, and every entity in the repo, so the
 * Details pane can put the selected thing's score beside the mean of its own
 * kind.
 *
 * Of its own kind, deliberately. `compositeScore` and `scopeCompositeScore`
 * are different formulas over different inputs — a function's 0.4 and a file's
 * 0.4 are not the same measurement — so a file is compared against files, a
 * folder against folders, and everything else against entities. Crossing them
 * would produce a confident sentence about nothing.
 *
 * Pure and store-free (the two scorers are passed in) so it can be unit-tested
 * with `npm run test:quality-baseline` without dragging `stores/quality` and
 * its threshold cache behind it.
 */

import type { D3Node, EntityMetrics, GraphData, ScopeMetrics } from '../types/graph';

/** Which population a score belongs to — one per scoring formula. */
export type ScoreGrain = 'file' | 'folder' | 'entity';

export interface Baseline {
  grain: ScoreGrain;
  /** Arithmetic mean of every score in the population. */
  mean: number;
  /** How many things went into it — a mean over three files is not a repo. */
  count: number;
}

export type ScopeScorer = (s: ScopeMetrics, isFolder: boolean) => number;
export type EntityScorer = (m: EntityMetrics, kindRaw: string) => number;

/** All three baselines, `null` where the population was empty. */
export type Baselines = Record<ScoreGrain, Baseline | null>;

export const NO_BASELINES: Baselines = { file: null, folder: null, entity: null };

/** Which population this node's score should be read against. */
export function grainOfKind(kindRaw: string): ScoreGrain {
  if (kindRaw === 'File') return 'file';
  if (kindRaw === 'Folder') return 'folder';
  return 'entity';
}

/**
 * Nodes that carry a score but do not belong in the entity population.
 *
 * `Parameter` is synthetic and would drown the mean in near-zero rows — the
 * same reason `qualityRows` skips it. `File` and `Folder` entities are scope
 * rollups scored by the other formula; they are counted through `g.files` /
 * `g.folders` instead, and counting them here too would mix the two scales.
 */
function inEntityPopulation(n: D3Node): boolean {
  return n.kind_raw !== 'Parameter' && n.kind_raw !== 'File' && n.kind_raw !== 'Folder';
}

/**
 * A node's own composite score, or `undefined` when it has none.
 *
 * Mirrors `nodeEncoding.severityScore`: prefer the backend's number, and fall
 * back to the front-end formula that matches the node's grain. A File or
 * Folder node is scored off its raw `scope_metrics` rollup, never off the
 * fields `collapseGraph` promotes into `metrics` (entity_count posing as
 * field_count, callable_count as method_count) — running the per-entity
 * formula on those returns a confident number that means nothing.
 */
export function nodeScore(
  d: D3Node,
  scoreScope: ScopeScorer,
  scoreEntity: EntityScorer,
): number | undefined {
  const m = d.metrics;
  if (!m) return undefined;
  if (m.composite_score != null) return m.composite_score;
  if (d.kind_raw === 'File' || d.kind_raw === 'Folder') {
    return d.scope_metrics
      ? scoreScope(d.scope_metrics, d.kind_raw === 'Folder')
      : undefined;
  }
  return scoreEntity(m, d.kind_raw);
}

function baselineOf(grain: ScoreGrain, scores: number[]): Baseline | null {
  if (scores.length === 0) return null;
  let sum = 0;
  for (const s of scores) sum += s;
  return { grain, mean: sum / scores.length, count: scores.length };
}

/**
 * Mean score per grain over `g`.
 *
 * Files and folders come from the engine's own rollups, scored exactly as
 * `fileRows` / `folderRows` score them, so the mean and the row a reader can
 * click through to are the same number.
 */
export function baselinesFor(
  g: GraphData | null,
  scoreScope: ScopeScorer,
  scoreEntity: EntityScorer,
): Baselines {
  if (!g) return NO_BASELINES;
  const files = (g.files ?? []).map((s) => s.composite_score ?? scoreScope(s, false));
  const folders = (g.folders ?? []).map((s) => s.composite_score ?? scoreScope(s, true));
  const entities: number[] = [];
  for (const n of g.nodes) {
    if (!n.metrics || !inEntityPopulation(n)) continue;
    entities.push(n.metrics.composite_score ?? scoreEntity(n.metrics, n.kind_raw));
  }
  return {
    file: baselineOf('file', files),
    folder: baselineOf('folder', folders),
    entity: baselineOf('entity', entities),
  };
}

export interface ScoreComparison {
  /** Score minus mean. Positive is *worse* — the scale runs low-is-good. */
  delta: number;
  verdict: 'above' | 'below' | 'at';
  /** What the verdict means, in the reader's terms rather than the scale's. */
  text: string;
}

/**
 * Rounded to the two decimals the panel prints, so the sentence never
 * contradicts the digits beside it — a score and a mean that both render as
 * `0.42` must not be described as "above average".
 */
const DISPLAY_EPSILON = 0.005;

export function compareToBaseline(score: number, b: Baseline): ScoreComparison {
  const delta = score - b.mean;
  if (Math.abs(delta) < DISPLAY_EPSILON) {
    return { delta: 0, verdict: 'at', text: 'about average' };
  }
  return delta > 0
    ? { delta, verdict: 'above', text: 'worse than average' }
    : { delta, verdict: 'below', text: 'better than average' };
}

/** Noun for the population a baseline covers, for the label beside it. */
export function baselineNoun(grain: ScoreGrain): string {
  return grain === 'file' ? 'file' : grain === 'folder' ? 'folder' : 'entity';
}
