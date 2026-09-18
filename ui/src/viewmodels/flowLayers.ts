/**
 * UI-146 — which way the dependencies run.
 *
 * A folder with nine files and thirty edges between them has a *direction*,
 * and the canvas has never said what it is. The force layout answers "what is
 * near what"; the hulls answer "what is filed together"; `regionTraffic`
 * answers "does this boundary hold". None of them answers the question a
 * reader asks first about an unfamiliar folder: **what sits on top of what.**
 *
 * That question is a property of the edge set alone, so it can be computed
 * rather than eyeballed — which matters most in exactly the case a human
 * cannot do it by hand, when the graph has cycles.
 *
 * ## Which end is which
 *
 * An edge in this graph points from the dependant at the dependency: `A calls
 * B`, `A imports B`, so `A → B` and A is the one that would break. This module
 * follows the convention the `impact` tool already uses — dependents are the
 * blast radius, so:
 *
 * - **upstream** = what a node depends on, transitively. The code it stands
 *   on. Reached by walking edges *forwards*.
 * - **downstream** = what depends on it, transitively. What a change here
 *   would reach. Reached by walking edges *backwards*.
 *
 * `layer` counts from the upstream end: layer 0 depends on nothing else in
 * the set, and each layer above it is one hop further from the foundation. So
 * change flows from low layers to high ones — the direction of the flux — and
 * the arrows on the canvas point the other way, at what each node needs. That
 * is not a contradiction, it is what a dependency arrow means, and the Flow
 * view labels its axis rather than leaving the reader to infer it.
 *
 * ## Cycles are the reason this is not a topological sort
 *
 * A dependency cycle has no upstream end, so a plain longest-path walk either
 * loops forever or silently picks a winner. Every node in a cycle is genuinely
 * at the same depth — that is what makes it a cycle — so the layering runs
 * over the graph's **strongly-connected components**, and the members of one
 * share a layer and are reported as a group. A reader gets "these four are
 * tangled together" instead of an arbitrary order presented as a hierarchy.
 *
 * Tarjan emits components in reverse topological order (sinks first), which is
 * the whole reason it is used here rather than a plain DFS: every derived
 * quantity below — the layer, and both reachability sets — is a single pass in
 * that order or its reverse, with no second sort and no revisiting.
 *
 * Pure and store-free (`npm run test:flow`), so the rules that are worth
 * arguing with — what a cycle's layer is, whether a node's own cycle mates
 * count as its upstream — can be pinned in a test rather than through a
 * browser.
 */

/** A directed relationship, reduced to the only thing this needs from it. */
export interface FlowEdge {
  /** The dependant. */
  source: string;
  /** The dependency. */
  target: string;
}

/**
 * Where a node sits at the ends of the flux.
 *
 * Named for what the reader can check, not for the degree that produced it: a
 * `foundation` is a node the rest of this set stands on, and the claim is
 * falsified by finding one outgoing edge.
 */
export type FlowRole =
  /** Depends on nothing else here, and something here depends on it. */
  | 'foundation'
  /** Nothing here depends on it, and it depends on something here. */
  | 'entry'
  /** Both. */
  | 'relay'
  /** Neither — no relationship with anything else in the set. */
  | 'isolated';

export interface FlowStanding {
  /** Hops from the upstream end. 0 depends on nothing else in the set. */
  layer: number;
  /** How many members it transitively depends on. */
  upstream: number;
  /** How many members transitively depend on it. */
  downstream: number;
  role: FlowRole;
  /**
   * The other members of its dependency cycle, or null when it is in none.
   *
   * Cycle mates are counted in BOTH `upstream` and `downstream`, because both
   * statements are true of them: each reaches the other by definition. A
   * reader who finds a node whose two counts overlap has found a cycle, which
   * is the correct conclusion.
   */
  cycle: readonly string[] | null;
}

export interface FlowReading {
  /** One entry per input id — a node with no edges is still in the reading,
   *  at layer 0 and `isolated`. */
  standing: Map<string, FlowStanding>;
  /** Ids by layer, upstream end first. Sorted within a layer for determinism;
   *  the canvas re-orders them to reduce crossings, the panel does not. */
  layers: string[][];
  /** Every cycle of two or more, upstream-most first. */
  cycles: string[][];
}

export function emptyFlow(): FlowReading {
  return { standing: new Map(), layers: [], cycles: [] };
}

/**
 * Layer the set, condensing cycles first.
 *
 * `nodes` is the population and `edges` is filtered against it — an edge with
 * an end outside the set says nothing about the order *within* it, and
 * counting it would let a folder's internal hierarchy be rearranged by a call
 * into the standard library. Self-loops go the same way, for the same reason:
 * recursion is not a hierarchy of one.
 *
 * Everything is O(V + E) except the two reachability passes, which are
 * O(V·E/32) on bitsets over the condensation. The populations this runs on are
 * a folder's children, a file's declarations, or a canvas under the 400-node
 * render budget, so that is comfortably affordable per keystroke.
 */
export function flowLayers(
  nodes: readonly string[],
  edges: readonly FlowEdge[],
): FlowReading {
  const ids = [...new Set(nodes)];
  if (ids.length === 0) return emptyFlow();
  const adj = adjacencyOf(ids, edges);
  return assemble(ids, adj, condense(adj.succ));
}

/** Who depends on whom, by index into `ids`. */
interface Adjacency {
  /** `succ[v]` — what `v` depends on. */
  succ: number[][];
  /** `pred[v]` — what depends on `v`. */
  pred: number[][];
}

/**
 * The condensation: cycles collapsed to one vertex each.
 *
 * `members` is **in reverse topological order** — every component appears
 * after everything it can reach — which is what makes `layersOf` and both
 * reachability passes single sweeps rather than sorted traversals.
 */
interface Condensation extends Adjacency {
  /** `comp[v]` — which component holds node `v`. */
  comp: Int32Array;
  /** Node indices per component. */
  members: number[][];
}

/**
 * Deduped adjacency, both ways.
 *
 * A folder pair joined by forty calls is one edge here. Everything downstream
 * counts *members*, never relationships, so a repeated pair would inflate
 * nothing and cost a traversal each.
 */
function adjacencyOf(ids: readonly string[], edges: readonly FlowEdge[]): Adjacency {
  const indexOf = new Map(ids.map((id, i) => [id, i]));
  const succ: number[][] = Array.from({ length: ids.length }, () => []);
  const pred: number[][] = Array.from({ length: ids.length }, () => []);
  const seen = new Set<number>();
  for (const e of edges) {
    const s = indexOf.get(e.source);
    const t = indexOf.get(e.target);
    if (s === undefined || t === undefined || s === t) continue;
    const pair = s * ids.length + t;
    if (seen.has(pair)) continue;
    seen.add(pair);
    succ[s].push(t);
    pred[t].push(s);
  }
  return { succ, pred };
}

/** The condensation graph, deduped per component pair. */
function condense(succ: readonly number[][]): Condensation {
  const { comp, members } = stronglyConnected(succ);
  const cSucc: number[][] = Array.from({ length: members.length }, () => []);
  const cPred: number[][] = Array.from({ length: members.length }, () => []);
  const seen = new Set<number>();
  for (let v = 0; v < succ.length; v++) {
    for (const w of succ[v]) {
      const pair = comp[v] * members.length + comp[w];
      if (comp[v] === comp[w] || seen.has(pair)) continue;
      seen.add(pair);
      cSucc[comp[v]].push(comp[w]);
      cPred[comp[w]].push(comp[v]);
    }
  }
  return { comp, members, succ: cSucc, pred: cPred };
}

/** One more than the deepest thing a component depends on. A forward sweep is
 *  valid because every successor was emitted earlier. */
function layersOf(cSucc: readonly number[][]): Int32Array {
  const layerOf = new Int32Array(cSucc.length);
  for (let c = 0; c < cSucc.length; c++) {
    let deepest = -1;
    for (const d of cSucc[c]) if (layerOf[d] > deepest) deepest = layerOf[d];
    layerOf[c] = deepest + 1;
  }
  return layerOf;
}

/** How many members each component transitively reaches, each way. Cycle
 *  mates are added on BOTH sides — see `FlowStanding.cycle`. */
function componentCounts(cond: Condensation, sizeOf: readonly number[]): {
  up: number[];
  down: number[];
} {
  const n = sizeOf.length;
  const upReach = reachability(cond.succ, n, false);
  const downReach = reachability(cond.pred, n, true);
  const up: number[] = [];
  const down: number[] = [];
  for (let c = 0; c < n; c++) {
    up.push(countMembers(upReach, c, n, sizeOf) + sizeOf[c] - 1);
    down.push(countMembers(downReach, c, n, sizeOf) + sizeOf[c] - 1);
  }
  return { up, down };
}

/** The reading, from the condensation and the counts over it. */
function assemble(ids: readonly string[], adj: Adjacency, cond: Condensation): FlowReading {
  const sizeOf = cond.members.map((m) => m.length);
  const layerOf = layersOf(cond.succ);
  const counts = componentCounts(cond, sizeOf);
  const mates = cond.members.map((m) => m.map((v) => ids[v]).sort());
  const standing = new Map<string, FlowStanding>();
  const layers: string[][] = Array.from({ length: maxOf(layerOf) + 1 }, () => []);
  for (let v = 0; v < ids.length; v++) {
    const c = cond.comp[v];
    standing.set(ids[v], {
      layer: layerOf[c],
      upstream: counts.up[c],
      downstream: counts.down[c],
      role: roleOf(adj.succ[v].length, adj.pred[v].length),
      cycle: mates[c].length > 1 ? mates[c].filter((m) => m !== ids[v]) : null,
    });
    layers[layerOf[c]].push(ids[v]);
  }
  for (const layer of layers) layer.sort();
  return { standing, layers, cycles: cyclesOf(mates, layerOf) };
}

/** Every group of two or more, upstream-most first. */
function cyclesOf(mates: readonly string[][], layerOf: Int32Array): string[][] {
  const out: { layer: number; ids: string[] }[] = [];
  for (let c = 0; c < mates.length; c++) {
    if (mates[c].length > 1) out.push({ layer: layerOf[c], ids: mates[c] });
  }
  out.sort((a, b) => a.layer - b.layer || a.ids[0].localeCompare(b.ids[0]));
  return out.map((e) => e.ids);
}

function roleOf(outDegree: number, inDegree: number): FlowRole {
  if (outDegree === 0 && inDegree === 0) return 'isolated';
  if (outDegree === 0) return 'foundation';
  if (inDegree === 0) return 'entry';
  return 'relay';
}

function maxOf(a: Int32Array): number {
  let m = 0;
  for (const v of a) if (v > m) m = v;
  return m;
}

/**
 * Transitive reachability over the condensation, as one bitset per component.
 *
 * `reverse` says which sweep direction makes each component's answer complete
 * before it is read. Following `cSucc`, successors are emitted first, so a
 * forward sweep works; following `cPred`, predecessors are emitted later, so
 * the sweep runs backwards. Getting this wrong does not crash — it silently
 * under-counts, which is why the two callers pass it explicitly rather than
 * sharing a default.
 */
function reachability(adj: number[][], nComp: number, reverse: boolean): Uint32Array {
  const words = (nComp + 31) >> 5;
  const bits = new Uint32Array(nComp * words);
  for (let k = 0; k < nComp; k++) {
    const c = reverse ? nComp - 1 - k : k;
    const base = c * words;
    for (const d of adj[c]) {
      bits[base + (d >> 5)] |= 1 << (d & 31);
      const other = d * words;
      for (let w = 0; w < words; w++) bits[base + w] |= bits[other + w];
    }
  }
  return bits;
}

/** Members held by every component in `c`'s reachability set. */
function countMembers(
  bits: Uint32Array,
  c: number,
  nComp: number,
  sizeOf: readonly number[],
): number {
  const words = (nComp + 31) >> 5;
  const base = c * words;
  let total = 0;
  for (let w = 0; w < words; w++) {
    let word = bits[base + w];
    while (word !== 0) {
      // Lowest set bit, cleared each turn. `clz32` rather than `log2` because
      // the top bit reads back as a negative int32 once `&` has coerced it,
      // and `log2` of that is NaN — a silent zero in the middle of a count.
      const bit = word & -word;
      total += sizeOf[(w << 5) + (31 - Math.clz32(bit))];
      word ^= bit;
    }
  }
  return total;
}

/**
 * Tarjan's SCC, iteratively.
 *
 * Iterative rather than recursive because the depth is the length of the
 * longest dependency chain, and `bodyScope` records a 138-deep branch nest on
 * this repo alone — a recursive version is one pathological input away from a
 * stack overflow that takes the whole canvas down.
 *
 * Returns the component index per node, and the members per component **in
 * reverse topological order**: every component appears after everything it can
 * reach. Three passes above depend on that guarantee.
 */
function stronglyConnected(succ: readonly number[][]): {
  comp: Int32Array;
  members: number[][];
} {
  const st = newTarjanState(succ.length);
  for (let root = 0; root < succ.length; root++) {
    if (st.index[root] === -1) exploreFrom(root, succ, st);
  }
  return { comp: st.comp, members: st.members };
}

/**
 * Everything the walk mutates, in one object.
 *
 * Bundled rather than passed as eight parameters, and the four helpers below
 * read through `st.` rather than destructuring: the state is genuinely one
 * thing, and spreading it across signatures makes every step read as if it
 * could be called with a different combination.
 */
interface TarjanState {
  /** Discovery order, `-1` until visited. */
  index: Int32Array;
  /** Lowest discovery order reachable without leaving the stack. */
  low: Int32Array;
  onStack: Uint8Array;
  comp: Int32Array;
  stack: number[];
  members: number[][];
  counter: number;
}

function newTarjanState(n: number): TarjanState {
  return {
    index: new Int32Array(n).fill(-1),
    low: new Int32Array(n),
    onStack: new Uint8Array(n),
    comp: new Int32Array(n).fill(-1),
    stack: [],
    members: [],
    counter: 0,
  };
}

/** First visit: stamp the discovery order and put it on the stack. */
function discover(v: number, st: TarjanState): void {
  st.index[v] = st.low[v] = st.counter++;
  st.stack.push(v);
  st.onStack[v] = 1;
}

/** A finished child's reach belongs to its parent too. */
function relaxParent(parent: number, child: number, st: TarjanState): void {
  if (st.low[child] < st.low[parent]) st.low[parent] = st.low[child];
}

/** `v` is a component root: pop everything above it into one component. */
function popComponent(v: number, st: TarjanState): void {
  const group: number[] = [];
  for (;;) {
    const w = st.stack.pop()!;
    st.onStack[w] = 0;
    st.comp[w] = st.members.length;
    group.push(w);
    if (w === v) break;
  }
  st.members.push(group);
}

/** The DFS itself, over an explicit frame stack. */
function exploreFrom(root: number, succ: readonly number[][], st: TarjanState): void {
  discover(root, st);
  const work: { v: number; i: number }[] = [{ v: root, i: 0 }];
  while (work.length > 0) {
    const frame = work[work.length - 1];
    const v = frame.v;
    if (frame.i < succ[v].length) {
      const w = succ[v][frame.i++];
      if (st.index[w] === -1) {
        discover(w, st);
        work.push({ v: w, i: 0 });
      } else if (st.onStack[w] === 1 && st.index[w] < st.low[v]) {
        st.low[v] = st.index[w];
      }
      continue;
    }
    work.pop();
    if (work.length > 0) relaxParent(work[work.length - 1].v, v, st);
    if (st.low[v] === st.index[v]) popComponent(v, st);
  }
}
