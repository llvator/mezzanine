/**
 * UI-146 — the flux reading, cached per dataset.
 *
 * `scopeFlow` is a sweep over every node in the graph, and the panels ask for
 * one on every hover — a folder outline, the file inside it, the entity inside
 * that. Recomputing per pointer move would be a full pass over twenty thousand
 * entities several times a second.
 *
 * So this exposes a *function* rather than a value, the same shape
 * `stores/region.ts` gives the spec claims, with a memo behind it. The closure
 * is rebuilt whenever `rawEntityGraph` changes, which drops the cache with it —
 * there is no invalidation to get wrong, because the cache cannot outlive the
 * graph it was computed from.
 *
 * It reads `rawEntityGraph` and not `graphData` deliberately: the hierarchy is
 * a fact about the code, not about the picture, and a ladder that reordered
 * itself when someone changed the aggregation level would be claiming
 * otherwise. See the header of `viewmodels/scopeFlow.ts`.
 */

import { derived, type Readable } from 'svelte/store';
import { rawEntityGraph } from './graph';
import {
  emptyScopeFlow,
  scopeFlow,
  siblingStanding,
  type FlowSubject,
  type ScopeFlow,
} from '../viewmodels/scopeFlow';

const keyOf = (s: FlowSubject): string => `${s.grain}:${s.path}`;

/** The insides of any scope, memoised for as long as the graph stands. */
export const scopeFlowOf: Readable<(subject: FlowSubject) => ScopeFlow> = derived(
  rawEntityGraph,
  ($graph) => {
    const cache = new Map<string, ScopeFlow>();
    return (subject: FlowSubject): ScopeFlow => {
      const key = keyOf(subject);
      const hit = cache.get(key);
      if (hit) return hit;
      const value = $graph.nodes.length === 0
        ? emptyScopeFlow(subject)
        : scopeFlow($graph.nodes, $graph.links, subject);
      cache.set(key, value);
      return value;
    };
  },
);

/** Where a scope stands among its siblings, or null when the question does not
 *  apply — the repo root, or an only child. Cached alongside the ladders,
 *  because it is the same computation one level up. */
export const siblingStandingOf: Readable<
  (subject: FlowSubject) => ReturnType<typeof siblingStanding>
> = derived(rawEntityGraph, ($graph) => {
  const cache = new Map<string, ReturnType<typeof siblingStanding>>();
  return (subject: FlowSubject) => {
    const key = keyOf(subject);
    if (cache.has(key)) return cache.get(key)!;
    const value = $graph.nodes.length === 0
      ? null
      : siblingStanding($graph.nodes, $graph.links, subject);
    cache.set(key, value);
    return value;
  };
});
