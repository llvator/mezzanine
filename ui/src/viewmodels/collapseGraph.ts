/**
 * Graph aggregation: collapse entity-level nodes down to one node per file
 * or one node per folder (directory), re-routing edges between scopes and
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

/** Shared empty set, so the default argument allocates nothing per call. */
const EMPTY_EXPANSION: ReadonlySet<string> = new Set<string>();

/** The directory holding a node's file. Exported so `mixedGrain` resolves a
 *  folder the same way this does — the two deciding a scope differently is
 *  the one bug a ring plan cannot survive, since the grain it picks and the
 *  scope this collapses into would name different things. */
export function folderOf(node: D3Node): string {
  const i = node.file_path.lastIndexOf('/');
  return i >= 0 ? node.file_path.slice(0, i) : '';
}

/**
 * UI-090 — does every file in this graph hold exactly one entity?
 *
 * When it does, collapsing entities into files removes nothing: Entity level
 * and File level are the same picture, and a reader who presses Entity and
 * watches the canvas not move has learned only that a control looks broken.
 * That is the normal case for a document graph — the Markdown parser emits
 * one Note per file, because a link addresses a whole document — but nothing
 * here asks what language anything is written in. A future language with the
 * same shape inherits the answer, and a Rust file that happens to hold one
 * struct does not, because the question is about the graph on screen.
 *
 * Takes the *entity-level* nodes. Asking a collapsed graph is meaningless:
 * every file already holds one node there by construction.
 */
export function everyFileIsOneEntity(nodes: readonly D3Node[]): boolean {
  if (nodes.length === 0) return false;
  const files = new Set<string>();
  for (const n of nodes) files.add(n.file_path);
  return files.size === nodes.length;
}

/**
 * The grain ONE node is drawn at (UI-104).
 *
 * The level used to be a property of the picture: ask for Entity and every
 * circle is an entity, the eight you came to read and the four hundred you
 * did not. It was never quite that, though — UI-057's expansion already made
 * the drawn graph mixed, one clicked scope at a time. This type is what that
 * was underneath all along, said once: a function from a node to the grain it
 * is drawn at, with `grainFromLevel` recovering the uniform case exactly.
 *
 * Everything downstream of `collapseGraph` already copes with a mixed result
 * — the display plan, the filters and the selection cannot tell a rollup from
 * an entity — so generalising here is where the whole feature is spent.
 */
export type GrainOf = (node: D3Node) => GraphLevel;

/**
 * Today's rule as a resolver: one level everywhere, with `expanded` opening
 * exactly one level under it (UI-057).
 *
 * `expanded` holds *paths*, never ids. A path survives a level change and a
 * re-analysis; `sanitizeId` rewrites ids and `collapseGraph` builds fresh
 * node objects on every level change, so an id-keyed expansion set would
 * silently stop matching the moment the view moved.
 *
 * Expansion opens exactly one level: an expanded folder renders as its
 * files, an expanded file as its entities. Anything more would make a single
 * gesture unpredictable — the reader would not know how much they were about
 * to add to the canvas. A ring plan (`mixedGrain.ts`) is the deliberate
 * exception: it is not a gesture on one scope, so the reader is told what it
 * will cost before it runs rather than after.
 *
 * A node with no file_path — a ghost — is never expanded, and takes the bare
 * level so `scopeIdFor` drops it below Entity. That is not an optimisation:
 * the root folder's path is `''`, so an expanded root would otherwise test
 * `expanded.has('')` against a ghost's empty file_path and promote thousands
 * of external symbols onto the canvas.
 */
export function grainFromLevel(level: GraphLevel, expanded: ReadonlySet<string>): GrainOf {
  return (node) => {
    if (level === 'entity') return 'entity';
    if (!node.file_path) return level;
    if (level === 'file') return expanded.has(node.file_path) ? 'entity' : 'file';
    return expanded.has(folderOf(node)) ? 'file' : 'folder';
  };
}

/**
 * Which scope a node collapses into — the answer `scopeIdFor` gives by
 * default, made replaceable (UI-108).
 *
 * `GrainOf` says how *coarse* a node is drawn and the scope follows from it,
 * which is enough for every uniform level and for a ring plan. It is not
 * enough for a picture rooted at one folder: a file three directories down
 * has to collapse into the immediate child of that folder holding it, and
 * that is an ancestor `folderOf` never names — it always answers the file's
 * own parent.
 *
 * `null` drops the node from the canvas entirely, which is what makes this a
 * filter as well as an aggregation: a resolver answering `null` for
 * everything unrelated to a folder is how the shape view shows one folder
 * and its neighbours and nothing else.
 */
export type ScopeOf = (node: D3Node, grain: GraphLevel) => string | null;

/**
 * The scope a node collapses into at its grain.
 *
 * `null` means the node belongs to no scope at this grain, which is only ever
 * a ghost: an external or stdlib reference has no file, and a real graph
 * carries thousands of them (6 598 against 13 615 real entities on this
 * repo), so they cannot each become a circle at a collapsed level. They used
 * to land on the `''` scope — the same key the repo ROOT directory has at
 * folder level — and the edge merge below dropped every edge touching it,
 * testing `!srcScope` where it meant "no scope at all". A root-level file's
 * relationships disappeared with them: in a doc graph the root holds the hub
 * documents (README, CLAUDE.md, CONTEXT.md), so the reader who selected one
 * got a Details pane with no Relationships section at all.
 *
 * Entity grain is answered before the file_path guard, because a ghost IS a
 * circle at Entity level — it is only below Entity that it has nowhere to go.
 */
function scopeIdFor(node: D3Node, grain: GraphLevel): string | null {
  if (grain === 'entity') return node.id;
  if (!node.file_path) return null;
  if (grain === 'file') return node.file_path;
  return folderOf(node);
}

/** True when this scope id is a single entity rather than a rollup — i.e.
 *  the node was expanded all the way. Used to decide whether an edge keeps
 *  its real relationship kind (UI-058). */
function isEntityScope(node: D3Node, scopeId: string): boolean {
  return scopeId === node.id;
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

/** Promote a file/folder `ScopeMetrics` entry to the shape expected by D3Node
 * (`EntityMetrics`). Only the fields the UI already renders are populated. */
function scopeToEntityMetrics(s: ScopeMetrics): EntityMetrics {
  return {
    loc: s.loc,
    fan_in: s.fan_in,
    fan_out: s.fan_out,
    in_cycle: s.in_cycle,
    method_count: s.callable_count,
    // CC / nesting / params don't map to a file or folder aggregate.
    cyclomatic: undefined,
    max_nesting: undefined,
    param_count: undefined,
    field_count: s.entity_count,
    public_field_ratio: undefined,
    // The scope's OWN composite score, matching `fileRows` / `folderRows` —
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
 * from `raw.files` / `raw.folders` are attached to the corresponding node
 * so the per-entity info panel can display meaningful numbers on a collapsed
 * node.
 */
export function collapseGraph(
  raw: GraphData,
  level: GraphLevel,
  expanded: ReadonlySet<string> = EMPTY_EXPANSION,
  grainOf?: GrainOf,
  scopeOf?: ScopeOf,
): GraphData {
  // The uniform fast path, kept exactly: Entity level with no ring plan is
  // the input graph, links and all. A ring plan has to be honoured even when
  // it happens to answer Entity everywhere, because the caller — not this
  // function — is the one that knows whether it does. A scope resolver
  // disables it for the same reason, and additionally because such a
  // resolver may be dropping nodes, which the fast path would not do.
  if (!grainOf && !scopeOf && level === 'entity') return raw;
  const grainFor = grainOf ?? grainFromLevel(level, expanded);
  const scopeFor = scopeOf ?? scopeIdFor;

  const fileIndex = new Map((raw.files ?? []).map((f) => [f.path, f]));
  const folderIndex = new Map((raw.folders ?? []).map((m) => [m.path, m]));

  // Build one D3Node per unique scope id encountered. We preserve the set of
  // file paths per scope (mostly 1 for file level, N for folder level) so
  // the detail panel can still show the underlying file list.
  const nodes = new Map<string, D3Node>();
  const entityToScope = new Map<string, string>();
  /** Scope ids that are a single entity rather than a rollup. */
  const entityScopes = new Set<string>();

  for (const n of raw.nodes) {
    const grain = grainFor(n);
    const scopeId = scopeFor(n, grain);
    if (scopeId === null) continue;
    entityToScope.set(n.id, scopeId);
    if (isEntityScope(n, scopeId)) entityScopes.add(scopeId);
    if (nodes.has(scopeId)) continue;

    // An expanded scope contributes the entity itself, untouched — same id,
    // same metrics, same kind. Only the rollups below are synthesised.
    if (isEntityScope(n, scopeId)) {
      nodes.set(scopeId, n);
      continue;
    }

    // A folder expanded to files yields File nodes even though the requested
    // level is Folder; a file expanded to entities is handled above. This is
    // what makes the view *mixed* rather than uniform — and with a ring plan
    // the grain is simply read off the node instead of being reconstructed
    // from the level and the expansion set.
    const isFileNode = grain === 'file';
    const { name, dir } = splitPath(scopeId);
    const id = sanitizeId(scopeId) || '_root_';
    // 'Folder', not 'Module': `EntityKind::Module` reaches the UI as
    // `kind_raw: 'Module'` too, and every `kind_raw === 'Module'` test in
    // the codebase means "is this a directory rollup" — so a `mod`
    // declaration answered yes to all of them.
    const kindRaw = isFileNode ? 'File' : 'Folder';
    const metricsSource = isFileNode ? fileIndex.get(scopeId) : folderIndex.get(scopeId);

    nodes.set(scopeId, {
      id,
      original_id: scopeId,
      name: name || '(root)',
      qualified_name: scopeId || '(root)',
      kind: kindRaw.toLowerCase(),
      kind_raw: kindRaw,
      file_path: isFileNode ? scopeId : (dir ? `${dir}/${name || ''}` : name),
      // A rollup is not a span in a file: it stands for every entity inside
      // it, and the first line of the first one is not a fact about the
      // scope. Both ends sit at 1 so anything reading a range gets an empty
      // one rather than a confident wrong number.
      line: 1,
      end_line: 1,
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
  /** Edges kept at full fidelity because both ends are expanded entities. */
  const passthrough: D3Link[] = [];
  for (const l of raw.links) {
    const srcEntity = typeof l.source === 'object' ? l.source.id : l.source;
    const tgtEntity = typeof l.target === 'object' ? l.target.id : l.target;
    const srcScope = entityToScope.get(srcEntity);
    const tgtScope = entityToScope.get(tgtEntity);
    // `undefined`, not falsy: `''` is the repo root, a scope like any other.
    if (srcScope === undefined || tgtScope === undefined || srcScope === tgtScope) continue;
    const srcNode = nodes.get(srcScope)!;
    const tgtNode = nodes.get(tgtScope)!;

    // A lifted twin (UI-113) says the same thing as the edge it was routed
    // off, one node further out. That is new information at Entity grain,
    // where the body it bypasses can be hidden — and double-counting at every
    // grain above it, where the branch and its callable collapse into the
    // same circle and the real edge is already in this rollup's weight.
    if (l.lifted_from && !(entityScopes.has(srcScope) && entityScopes.has(tgtScope))) continue;

    // UI-058. Both ends expanded to entities means this is a real
    // entity-to-entity relationship that happens to be drawn on a mixed
    // canvas — it keeps its own kind, its order badge and its label. Only an
    // edge with a *rollup* on at least one end becomes `DependsOn`, because
    // that is the only case where the specific kind no longer describes the
    // pair on screen.
    if (entityScopes.has(srcScope) && entityScopes.has(tgtScope)) {
      passthrough.push({ ...l, source: srcNode.id, target: tgtNode.id });
      continue;
    }

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
  const linkList = [...passthrough, ...edgeMap.values()];

  return {
    nodes: nodeList,
    links: linkList,
    files: raw.files,
    folders: raw.folders,
    thresholds: raw.thresholds,
  };
}
