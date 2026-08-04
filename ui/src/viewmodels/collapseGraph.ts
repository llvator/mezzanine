/**
 * Graph aggregation: collapse entity-level nodes down to one node per file
 * or one node per module (directory), re-routing edges between scopes and
 * merging weights. Keeps the output shape identical to `GraphData` so the
 * downstream D3 pipeline, filters, and selection flow are unchanged.
 *
 * This is a pure function — no store access, no side effects. Callers own
 * when to run it (today: `publishGraph` + on-level-change re-publish).
 *
 * It must STAY store-free: `stores/graph.ts` imports this module, so any
 * import of a store here closes a cycle (graph -> collapseGraph -> store ->
 * graph) that leaves `graphData` undefined at module-eval time and takes the
 * whole app down. Scoring policy therefore lives in `nodeEncoding.ts`, which
 * reads the `scope_metrics` passed through below.
 */

import type { D3Node, D3Link, GraphData, GraphLevel, EntityMetrics, ScopeMetrics } from '../types/graph';

/** Return the scope id (file path or directory path) for a given entity. */
function scopeIdFor(node: D3Node, level: GraphLevel): string {
  if (level === 'file') return node.file_path;
  if (level === 'module') {
    const i = node.file_path.lastIndexOf('/');
    return i >= 0 ? node.file_path.slice(0, i) : '';
  }
  return node.id;
}

function sanitizeId(s: string): string {
  return s.replace(/[^a-zA-Z0-9_]/g, '_');
}

/** Derive basename and directory from a scope path. */
function splitPath(p: string): { name: string; dir: string } {
  const i = p.lastIndexOf('/');
  if (i < 0) return { name: p, dir: '' };
  return { name: p.slice(i + 1), dir: p.slice(0, i) };
}

/** Promote a file/module `ScopeMetrics` entry to the shape expected by D3Node
 * (`EntityMetrics`). Only the fields the UI already renders are populated. */
function scopeToEntityMetrics(s: ScopeMetrics): EntityMetrics {
  return {
    loc: s.loc,
    fan_in: s.fan_in,
    fan_out: s.fan_out,
    in_cycle: s.in_cycle,
    method_count: s.callable_count,
    // CC / nesting / params don't map to a file or module aggregate.
    cyclomatic: undefined,
    max_nesting: undefined,
    param_count: undefined,
    field_count: s.entity_count,
    public_field_ratio: undefined,
    // The scope's OWN composite score, matching `fileRows` / `moduleRows` —
    // so a node's colour on the canvas and its row in the Quality panel are
    // the same number.
    //
    // Deliberately NOT `avg_quality`. That is the *mean* score of the
    // entities inside the scope, and a mean is the wrong statistic for
    // "should I worry about this file": a thousand-line file with three
    // trivial helpers per hotspot averages down to ~0.01. Measured on this
    // repo, every one of the 47 `ui/` files scored under 0.02 on
    // `avg_quality` and landed on a single ramp step, versus a 0.15-1.38
    // spread on `composite_score`. Left undefined when the backend omits it —
    // 0 is the *best* score, so defaulting would paint unknown as pristine,
    // and `nodeEncoding` recomputes it from `scope_metrics` instead.
    composite_score: s.composite_score,
  };
}

/**
 * Collapse `raw` to one node per scope at the requested level. When
 * `level === 'entity'` the input is returned as-is. Scope rollup metrics
 * from `raw.files` / `raw.modules` are attached to the corresponding node
 * so the per-entity info panel can display meaningful numbers on a collapsed
 * node.
 */
export function collapseGraph(raw: GraphData, level: GraphLevel): GraphData {
  if (level === 'entity') return raw;

  const scopeIndex = level === 'file'
    ? new Map((raw.files ?? []).map((f) => [f.path, f]))
    : new Map((raw.modules ?? []).map((m) => [m.path, m]));

  // Build one D3Node per unique scope id encountered. We preserve the set of
  // file paths per scope (mostly 1 for file level, N for module level) so
  // the detail panel can still show the underlying file list.
  const nodes = new Map<string, D3Node>();
  const entityToScope = new Map<string, string>();

  for (const n of raw.nodes) {
    const scopeId = scopeIdFor(n, level);
    entityToScope.set(n.id, scopeId);
    if (nodes.has(scopeId)) continue;

    const { name, dir } = splitPath(scopeId);
    const id = sanitizeId(scopeId) || '_root_';
    const kindRaw = level === 'file' ? 'File' : 'Module';
    const metricsSource = scopeIndex.get(scopeId);

    nodes.set(scopeId, {
      id,
      original_id: scopeId,
      name: name || '(root)',
      qualified_name: scopeId || '(root)',
      kind: kindRaw.toLowerCase(),
      kind_raw: kindRaw,
      file_path: level === 'file' ? scopeId : (dir ? `${dir}/${name || ''}` : name),
      line: 1,
      visibility: 'Public',
      parent_id: null,
      parameters: [],
      return_type: null,
      extends: [],
      implements: [],
      tags: [],
      source_code: null,
      fields: [],
      impl_blocks: [],
      language: n.language,
      metrics: metricsSource ? scopeToEntityMetrics(metricsSource) : undefined,
      // Carried through so severity scoring can fall back to the same
      // `scopeCompositeScore` the Quality panel uses without this module
      // importing a store. See the cycle note in the header.
      scope_metrics: metricsSource,
    });
  }

  // Merge edges: skip intra-scope. Collapse every cross-scope edge kind
  // (Calls / Inherits / Implements / …) into a single `DependsOn` link per
  // ordered (src, tgt) pair. Entity-level vocabulary doesn't lift cleanly
  // to the scope level — "file A calls file B" is nonsense; "file A depends
  // on file B" is what readers actually want. We keep a per-kind `breakdown`
  // so the original fidelity is available on hover.
  type EdgeKey = string;
  const edgeMap = new Map<EdgeKey, D3Link>();
  for (const l of raw.links) {
    const srcEntity = typeof l.source === 'object' ? l.source.id : l.source;
    const tgtEntity = typeof l.target === 'object' ? l.target.id : l.target;
    const srcScope = entityToScope.get(srcEntity);
    const tgtScope = entityToScope.get(tgtEntity);
    if (!srcScope || !tgtScope || srcScope === tgtScope) continue;
    const srcNode = nodes.get(srcScope)!;
    const tgtNode = nodes.get(tgtScope)!;
    const key: EdgeKey = `${srcNode.id}->${tgtNode.id}`;
    const existing = edgeMap.get(key);
    if (existing) {
      existing.weight = (existing.weight ?? 0) + 1;
      const b = existing.breakdown!;
      b[l.kind_raw] = (b[l.kind_raw] ?? 0) + 1;
    } else {
      edgeMap.set(key, {
        source: srcNode.id,
        target: tgtNode.id,
        kind: 'depends on',
        kind_raw: 'DependsOn',
        incoming_kind: 'depended on by',
        order: null,
        weight: 1,
        breakdown: { [l.kind_raw]: 1 },
      });
    }
  }

  const nodeList = Array.from(nodes.values());
  const linkList = Array.from(edgeMap.values());

  return {
    nodes: nodeList,
    links: linkList,
    files: raw.files,
    modules: raw.modules,
    thresholds: raw.thresholds,
  };
}
