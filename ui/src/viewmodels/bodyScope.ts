/**
 * What a file declares, as against what a function does (UI-113).
 *
 * Every entity a parser emits lands on the same canvas: the twelve functions
 * a file declares, and the four hundred parameters, branch arms and loop
 * bodies inside them. Both are true, and only one of them is what a reader
 * opening a scope is looking at. The kind checkboxes cannot separate them —
 * `Function` is one kind whether it is a module's entry point or a closure
 * three levels down a callback — so the only way to quiet the canvas was to
 * untick kinds wholesale and lose the thing you unticked them to find.
 *
 * The separating question is not the kind and not the nesting depth. It is
 * **whether an entity lives inside a body**: the entities enclosed by a
 * callable are that callable's internals, and the entities enclosed by a
 * *type* — a struct's fields, an interface's properties, a class's methods —
 * are what the file declares. A Rust `impl` method has a parent and is
 * surface; a `Branch` inside a free function has a parent and is not. That is
 * why this is an ancestry walk rather than `parent_id === null`, which would
 * erase every method in the repo (measured on nao's own graph: 1 689 of
 * 10 831 callables carry a parent, and every one of them is a declaration).
 *
 * ## The half that is not filtering
 *
 * Hiding body entities alone would make the graph lie. A call written inside
 * an `if` does not hang off the function — the analyzer reattaches it to the
 * `Branch` node, so the edge on the wire is `branch → callee`, and dropping
 * the branch drops the call with it. On this repo that is 1 100 calls that
 * would silently vanish, most of a `match`-heavy function's fan-out.
 *
 * So `liftBodies` also **re-routes**: every edge with an end inside a body
 * gains a twin anchored at the enclosing callable, tagged `lifted_from` with
 * the ids it bypassed. Nothing is removed from the dataset — the branch nodes
 * and their edges stay exactly where they were — which is what lets the
 * filter be a view decision (`displayPlan`) rather than a load-time one, and
 * what lets a selected callable open its own body back up without the graph
 * being rebuilt underneath the selection.
 *
 * Pure — nodes and links in, nodes and links out, no stores — for the same
 * reason `collapseGraph` is: the interesting failures (a method classified as
 * an internal, a call lost with its branch, a lifted twin drawn on top of the
 * real edge) are all properties of a graph, testable without a browser.
 *
 *   npm run test:bodies
 */

import type { D3Node, D3Link, GraphData } from '../types/graph';

/**
 * Kinds that own a body — the enclosures whose contents are internals.
 *
 * `Branch` and `Loop` are synthetic scopes the analyzer mints inside a
 * callable, so they are always themselves internals; they are listed anyway
 * because they enclose in turn (`branch → branch` nests 138 deep on this
 * repo) and the walk has to keep climbing through them to find the callable.
 *
 * Containers are deliberately absent. A `Struct`, `Interface`, `Class`,
 * `Module` or `Trait` encloses declarations, not internals, and its children
 * are exactly what this filter exists to keep.
 */
export const BODY_KINDS: ReadonlySet<string> = new Set([
  'Function',
  'Method',
  'Macro',
  'Branch',
  'Loop',
]);

/** Guards a `parent_id` chain that points at itself. Nothing legitimate
 *  nests this far; the cap is cheaper than proving the parser never emits a
 *  cycle. */
const MAX_DEPTH = 64;

/**
 * Resolve a `parent_id` the way the rest of the UI does.
 *
 * `parent_id` normally holds the parent's `original_id`, except for Rust
 * `impl` blocks, where it can be the bare type name — the same quirk
 * `descriptionChain` and `qualityPopulation` allow for. The name index is
 * consulted only for names no entity owns as an id, so a type whose name
 * collides with another entity's id cannot adopt its children.
 */
function parentIndex(nodes: readonly D3Node[]): Map<string, D3Node> {
  const byId = new Map<string, D3Node>();
  for (const n of nodes) byId.set(n.original_id, n);
  for (const n of nodes) {
    if (!byId.has(n.name)) byId.set(n.name, n);
  }
  return byId;
}

/**
 * For every entity inside a body, the `original_id` of the callable whose
 * body it is in — walking *out* through any number of branches and loops to
 * the outermost enclosure that is not itself an internal.
 *
 * Entities at the surface are absent from the map rather than present with a
 * null: "is this an internal" and "whose" are the same lookup, and a map with
 * an entry for all 21 000 entities to say "no" about 14 000 of them is a
 * worse answer to both.
 *
 * The owner is always a callable. The walk stops at the first ancestor that
 * is not itself inside a body, and an ancestor reached through `BODY_KINDS`
 * is by construction one of them — so a parameter's owner is its method, and
 * a call nested five branches deep in that method has the same owner. That
 * is what makes the exemption in `displayPlan` a single equality test: one
 * selected callable, one `body_of` value, its whole body back on screen.
 */
export function bodyOwners(nodes: readonly D3Node[]): Map<string, string> {
  const byId = parentIndex(nodes);
  const owners = new Map<string, string>();
  /** Memo over the *answer*, including "surface" — the chains share long
   *  tails and a deep branch nest would otherwise re-walk them per node. */
  const resolved = new Map<string, string | null>();

  const ownerOf = (node: D3Node): string | null => {
    // Climb once, recording the nodes passed through, so a single walk
    // answers for every entity on the chain rather than re-walking the
    // shared tail per node.
    const path: D3Node[] = [];
    let current: D3Node = node;
    let answer: string | null = null;

    for (let depth = 0; depth < MAX_DEPTH; depth++) {
      const cached = resolved.get(current.original_id);
      if (cached !== undefined) {
        // `current` has been settled before. If it is itself an internal its
        // owner is the outermost enclosure for everything below it too; if it
        // is at the surface, the enclosure is `current`, which the previous
        // step already put in `answer`.
        answer = cached ?? answer;
        break;
      }
      const parentId = current.parent_id;
      const parent = parentId ? byId.get(parentId) : undefined;
      // Three ways to be at the surface: no parent, a parent outside the
      // loaded graph, or a parent that encloses declarations rather than
      // internals. The middle one is a scope boundary, not evidence of a
      // body — hiding an entity on the strength of an ancestor the reader
      // cannot see is a claim this module has no grounds for.
      if (!parent || parent.original_id === current.original_id
          || !BODY_KINDS.has(parent.kind_raw)) {
        resolved.set(current.original_id, null);
        break;
      }
      path.push(current);
      answer = parent.original_id;
      current = parent;
    }

    // Everything on the path is inside the same outermost enclosure — that
    // is what makes the owner a callable rather than the immediate parent,
    // and what lets `displayPlan` reopen a whole body with one equality.
    for (const n of path) resolved.set(n.original_id, answer);
    return resolved.get(node.original_id) ?? null;
  };

  for (const n of nodes) {
    const owner = ownerOf(n);
    if (owner !== null) owners.set(n.original_id, owner);
  }
  return owners;
}

/** Same key `displayPlan.linkKey` builds, so a lifted twin can be tested
 *  against the real edges it might duplicate. */
const linkKey = (src: string, tgt: string, kind: string, order?: number | null) =>
  order == null ? `${src}->${tgt}|${kind}` : `${src}->${tgt}|${kind}#${order}`;

const sourceIdOf = (l: D3Link): string =>
  typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
const targetIdOf = (l: D3Link): string =>
  typeof l.target === 'object' ? (l.target as D3Node).id : l.target;

/**
 * Stamp `body_of` on every internal and add the lifted twin of every edge
 * that would be lost with them.
 *
 * Run once per dataset, on the entity-level graph, before any aggregation —
 * `parent_id` is an entity-level fact and a collapsed node has no ancestry to
 * read. Nothing is dropped: this only ever *adds*, so a graph that passes
 * through it with the filter off is the graph as it was.
 *
 * Three edges earn no twin:
 *
 *  - one whose ends lift to the same callable (a parameter's `TakesParam`,
 *    a branch's `Contains`, a call from one branch of a function into
 *    another). It is a fact about the inside of one function, and its twin
 *    would be a self-loop — 7 179 of them on this repo, every one drawn as a
 *    dot on top of a node.
 *  - one that already exists between the same pair, at the same call-site
 *    order. The lift is meant to recover a relationship, not to double the
 *    stroke on one that survived on its own.
 *  - one with no end inside a body at all, which is most of the graph.
 */
export function liftBodies(graph: GraphData): GraphData {
  const owners = bodyOwners(graph.nodes);
  if (owners.size === 0) return graph;

  // `owners` speaks `original_id` (that is what `parent_id` holds); links
  // speak the sanitized `id`. One index bridges them, and the *sanitized*
  // owner id is what a re-routed endpoint needs.
  const idByOriginal = new Map(graph.nodes.map((n) => [n.original_id, n.id]));
  const surfaceOf = new Map<string, string>();
  for (const n of graph.nodes) {
    const owner = owners.get(n.original_id);
    if (owner === undefined) continue;
    const ownerId = idByOriginal.get(owner);
    if (ownerId !== undefined) surfaceOf.set(n.id, ownerId);
  }

  const nodes = graph.nodes.map((n) => {
    const owner = owners.get(n.original_id);
    return owner === undefined ? n : { ...n, body_of: owner };
  });

  const seen = new Set<string>();
  for (const l of graph.links) {
    seen.add(linkKey(sourceIdOf(l), targetIdOf(l), l.kind_raw, l.order));
  }

  const lifted: D3Link[] = [];
  for (const l of graph.links) {
    const src = sourceIdOf(l);
    const tgt = targetIdOf(l);
    const liftedSrc = surfaceOf.get(src);
    const liftedTgt = surfaceOf.get(tgt);
    if (liftedSrc === undefined && liftedTgt === undefined) continue;
    const s = liftedSrc ?? src;
    const t = liftedTgt ?? tgt;
    if (s === t) continue;
    const key = linkKey(s, t, l.kind_raw, l.order);
    if (seen.has(key)) continue;
    seen.add(key);
    // Which ends were re-routed, so `displayPlan` can drop the twin the
    // moment the reader opens the body it bypasses and the real edge is
    // drawable again.
    const from: string[] = [];
    if (liftedSrc !== undefined) from.push(src);
    if (liftedTgt !== undefined) from.push(tgt);
    lifted.push({ ...l, source: s, target: t, lifted_from: from });
  }

  if (lifted.length === 0) return { ...graph, nodes };
  return { ...graph, nodes, links: [...graph.links, ...lifted] };
}

// --- Reading the stamps back ---
//
// The three questions a canvas asks of this module once `liftBodies` has run.
// They live here rather than in `displayPlan` because they are the definition
// of the filter, and `displayPlan` consults them from five places — the force
// filter, both tree walks, the BFS and two link loops. One of those drifting
// from the others is the failure mode: a body walked into but not drawn, or a
// twin drawn beside the edge it stands in for.

/** The two settings these predicates read. `ComputeArgs` satisfies it
 *  structurally, so `displayPlan` passes its own args straight through. */
export interface BodyFilterState {
  /** Draw only what a file declares. */
  structureOnly: boolean;
  /** `original_id` of the one callable whose body is exempt — the selection. */
  exemptBody: string | null;
}

/**
 * Is this entity someone else's internals, on a canvas drawing declarations?
 *
 * `body_of` is absent on every surface entity, so the common answer costs one
 * field read. The exemption is a single equality because `body_of` names the
 * *outermost* enclosure: one selected callable, its whole body, no walk.
 */
export function bodyHidden(n: D3Node, state: BodyFilterState): boolean {
  if (!state.structureOnly) return false;
  if (n.body_of === undefined) return false;
  return n.body_of !== state.exemptBody;
}

/**
 * May this edge be drawn, given what is on screen?
 *
 * Only ever says no to a *lifted twin* — a real edge is decided by its
 * endpoints, as it always was. A twin exists to stand in for an edge whose end
 * is inside a hidden body, so it has exactly two ways to be wrong: drawn
 * beside the edge it duplicates when bodies are shown at all, or drawn beside
 * it when the reader has opened the one body it bypasses. Both are the same
 * question — is the real edge drawable? — and both answers are here.
 */
export function linkDrawable(
  l: D3Link,
  visible: ReadonlySet<string>,
  state: { structureOnly: boolean },
): boolean {
  if (!l.lifted_from) return true;
  if (!state.structureOnly) return false;
  return !l.lifted_from.every((id) => visible.has(id));
}

/**
 * May a walk follow this edge?
 *
 * A BFS runs before anything is known to be visible, so it cannot ask
 * `linkDrawable`'s question — but it can refuse to count a twin at all while
 * every body is on screen and the real path is walkable. Where a body is
 * exempt both routes lead to the same node and the walk's own visited set
 * settles it.
 */
export function linkTraversable(l: D3Link, state: { structureOnly: boolean }): boolean {
  return !l.lifted_from || state.structureOnly;
}
