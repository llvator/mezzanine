/**
 * UI-147 — what the marked set can answer besides "narrow to this".
 *
 * Marking two scopes has meant exactly one thing since the gesture existed:
 * `drillIntoMarks` throws away everything else and redraws. That is a good
 * answer to *show me these two closer up* and no answer at all to the question
 * a reader usually marked them to ask — **how do these two relate?** Drilling
 * shows the pair with their edges; it does not say which direction the
 * dependency runs, what they both stand on, who uses both, or — when nothing
 * runs between them — whether they are genuinely unrelated or two hops apart
 * through a module neither of them names.
 *
 * Four readings, computed together because they are one question:
 *
 * 1. **Direct flow** — every edge running between the marked scopes, split by
 *    direction and by kind, down to the entity pairs. One-way, mutual and
 *    absent are three different structural facts and the picture on the canvas
 *    distinguishes none of them at a glance.
 * 2. **Shared dependencies** — scopes *both* sides depend on. Common ground.
 * 3. **Shared dependents** — scopes that depend on both. Two things nothing
 *    connects but everything uses together usually want to be one thing.
 * 4. **Connecting chains** — when no edge runs between them, the shortest
 *    routes that do, bounded to three hops. This is the literal answer to
 *    "how do these relate" for the pair that looks unrelated.
 *
 * ## Scopes, not entities — except where the entity is the point
 *
 * A mark is a *path* (`viewmodels/markSet.ts`), so everything here is keyed by
 * path and the neighbour lists name files, not functions. The one exception is
 * the direct flow, which lists the entity pairs: "these two files are coupled"
 * is a fact you act on by opening `parse` and `emit`, and the pair is what
 * carries you there.
 *
 * **External dependencies count.** A ghost — an import the analysis resolved
 * to a library rather than to a file in this repo — has no `file_path` and so
 * cannot be a marked side, but it is a perfectly good *shared* dependency, and
 * often the most telling one: two files with no edge between them that both
 * reach for the same three crates are the same layer. Ghosts get a scope key
 * of their own and are flagged `external` so a renderer can say which they are.
 *
 * ## This reads the repo, not the canvas
 *
 * The caller is expected to pass the whole-repo entity graph
 * (`ensureFullData`), not the scoped one. "Who depends on both of these" has
 * one true answer and it is not "whoever happens to be drawn right now" — a
 * shared dependent living outside the current scope is exactly the one worth
 * knowing about, and answering from the visible graph would silently report
 * fewer the more the reader had narrowed.
 *
 * Pure and store-free (`npm run test:relate`).
 */

import type { D3Link, D3Node } from '../types/graph';

/** Entity pairs listed per direction before the rest becomes a count. */
const MAX_PAIRS = 12;
/** Rows in each shared list. The counts travel beside it, so a capped list
 *  never makes the verdict understate what was found. */
const MAX_SHARED = 12;
/**
 * External rows either shared list will show, out of `MAX_SHARED`.
 *
 * Measured, not guessed: over this repo's own graph, two marked files in
 * `src/server` share eleven dependencies and ten of them are `String`, `Ok`,
 * `Vec`, `Some` — Rust builtins that every file in every repo shares and that
 * therefore say nothing about these two. The one row that *did* say something
 * was `state.rs`. Externals still earn a place, because two files that both
 * reach for `axum` are a real finding, but not at the price of burying the
 * repo files under the standard library.
 */
const MAX_SHARED_EXTERNAL = 4;
/** Chains listed per ordered pair. */
const MAX_CHAINS = 5;
/** Hops a chain may span. 1 is a direct edge, so this admits two mediators. */
export const MAX_HOPS = 3;
/**
 * Neighbours either end of a chain search contributes to the three-hop cross
 * product. A hub file with a thousand neighbours would otherwise turn one
 * click into a million map lookups, and the thousand-and-first mediator is not
 * the one anybody reads.
 */
const CHAIN_FANOUT = 60;

// --- Scope keys ---------------------------------------------------------
//
// One string space for three kinds of scope: a marked side, a file, and a
// ghost. A file path is the key for a file, so the other two need a prefix no
// path can carry — hence NUL, built with `fromCharCode` rather than typed as a
// literal, so the source stays plain text that `grep` will still read. One
// `Map<string, …>` then holds all three and the graph walk never has to branch
// on which kind of scope it is looking at.

const NUL = String.fromCharCode(0);
const SIDE_TAG = `${NUL}s:`;
const GHOST_TAG = `${NUL}x:`;

const sideKey = (index: number): string => `${SIDE_TAG}${index}`;
const ghostKey = (id: string): string => `${GHOST_TAG}${id}`;

/** Is this key one of the marked sides, rather than something outside them? */
export function isSideKey(key: string): boolean {
  return key.startsWith(SIDE_TAG);
}

function isGhostKey(key: string): boolean {
  return key.startsWith(GHOST_TAG);
}

function sideIndexOf(key: string): number {
  return Number(key.slice(SIDE_TAG.length));
}

// --- The reading --------------------------------------------------------

/** One marked scope, as the reading refers to it. */
export interface RelationSide {
  /** The marked path. */
  path: string;
  /** Last segment — what a heading says when the full path is beside it. */
  label: string;
  grain: 'file' | 'folder';
  /** Non-ghost entities under this path. */
  entities: number;
  /** Distinct files under it. 1 for a file side. */
  files: number;
}

/** How many edges of one kind run one way between two sides. */
export interface KindTally {
  /** `kind_raw` — the stable name, and what a caller should key on. */
  kind: string;
  /** The language-aware label the graph gave it ("calls", "declares"). */
  label: string;
  count: number;
}

/** One edge between the marked sides, named at both ends. */
export interface EntityPair {
  sourceId: string;
  sourceName: string;
  sourceFile: string;
  targetId: string;
  targetName: string;
  targetFile: string;
  /** Display label, as `D3Link.kind`. */
  label: string;
}

/** Everything running from one marked side to another. */
export interface DirectFlow {
  /** Index into `MarkRelation.sides`. */
  from: number;
  to: number;
  /** Edges in this direction. */
  total: number;
  /** Heaviest kind first. */
  kinds: KindTally[];
  /** Up to `MAX_PAIRS` of them, in the order the graph lists them. */
  pairs: EntityPair[];
  /** Pairs beyond the cap — 0 when `pairs` is all of them. */
  more: number;
}

/** A scope every marked side touches, on one side of the arrow. */
export interface SharedScope {
  key: string;
  /** File path, or the ghost's name when `external`. */
  path: string;
  label: string;
  /** True when this is a library the analysis never opened, not a file. */
  external: boolean;
  /** Edges to (or from) this scope, per marked side, by side index. */
  perSide: number[];
  /** Sum of `perSide` — what the list sorts on. */
  total: number;
}

/**
 * One shared list — the rows to show, and what they are a sample of.
 *
 * `files` and `external` are counted apart because they answer different
 * questions and only one of them is about this repository. "These two share
 * four files" is a statement about the structure a reader can change; "these
 * two both use `String`" is a statement about Rust. Folding them into one
 * total produced sentences like *they share 123 dependencies* for a pair whose
 * real overlap was three files, which is the kind of number that stops a
 * reader trusting the panel.
 */
export interface SharedList {
  /** Repo files first, then externals, each heaviest first. Capped. */
  rows: SharedScope[];
  /** Shared scopes that are files in this repo. */
  files: number;
  /** Shared scopes that are libraries the analysis never opened. */
  external: number;
}

/** One step of a connecting chain. */
export interface ChainStep {
  key: string;
  path: string;
  label: string;
  external: boolean;
}

/** A route between two marked sides that no direct edge covers. */
export interface Chain {
  from: number;
  to: number;
  /** The scopes between the two sides, in order. Never empty — a route with
   *  no mediator is a direct edge, which is a `DirectFlow`. */
  via: ChainStep[];
  /** Edges on the thinnest link of the route. A chain is only as real as its
   *  narrowest hop, so this is what the list sorts on. */
  strength: number;
}

/** What shape the marked set turned out to be. */
export type RelationTone =
  | 'mutual'
  | 'one-way'
  | 'siblings'
  | 'indirect'
  | 'independent'
  /** Fewer than two marked scopes — there is no relation to read. */
  | 'none';

export interface MarkRelation {
  sides: RelationSide[];
  /** Only the directions that carry something, heaviest first. */
  flows: DirectFlow[];
  /** What all the marked sides depend on. */
  sharedDeps: SharedList;
  /** What depends on all the marked sides. */
  sharedDependents: SharedList;
  chains: Chain[];
  tone: RelationTone;
  /** One sentence naming the shape, ready to render. */
  verdict: string;
}

const NOTHING_MARKED = 'Mark two scopes to read how they relate.';

// --- Sides --------------------------------------------------------------

/**
 * Drop marks that live inside another mark.
 *
 * A folder and a file inside it are one side, not two: left as two they would
 * report edges "between" themselves and halve their own neighbour counts,
 * because every node under the file belongs to both. `setScopes` compacts for
 * the same reason before it narrows — this is that rule applied to the reading
 * rather than to the scope.
 *
 * Shortest-first is how the containment test is made cheap — a container is
 * always shorter than what it contains, so one pass over the kept list decides
 * it. The answer is re-sorted by path before it leaves, because that working
 * order is an artefact of the algorithm and everything downstream indexes
 * sides by position: which side is "first" would otherwise depend on how many
 * characters its path happens to have.
 */
export function compactSides(paths: readonly string[]): string[] {
  const byLength = [...paths].sort((a, b) => a.length - b.length || a.localeCompare(b));
  const kept: string[] = [];
  for (const p of byLength) {
    if (kept.some((q) => p === q || p.startsWith(`${q}/`))) continue;
    kept.push(p);
  }
  return kept.sort((a, b) => a.localeCompare(b));
}

/** Which marked side holds this file, or null when none does. The sides are
 *  disjoint after `compactSides`, so the first match is the only match. */
function sideOfFile(filePath: string, sides: readonly string[]): number | null {
  for (let i = 0; i < sides.length; i++) {
    if (filePath === sides[i] || filePath.startsWith(`${sides[i]}/`)) return i;
  }
  return null;
}

/**
 * The scope a node belongs to, or null when it belongs to none.
 *
 * Null is for a node with no file and no ghost tag — nothing the reading can
 * place. Keying it on `''` instead would file it under the repo root and make
 * every such node look like a shared dependency on the whole repository.
 */
function scopeKeyOf(n: D3Node, sides: readonly string[]): string | null {
  if (n.tags?.includes('ghost')) return ghostKey(n.id);
  if (!n.file_path) return null;
  const i = sideOfFile(n.file_path, sides);
  return i === null ? n.file_path : sideKey(i);
}

function lastSegment(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? path : path.slice(i + 1);
}

/**
 * File names that name their *folder* rather than themselves.
 *
 * A route printed as `handlers.rs → mod.rs → main.rs → mod.rs` is unreadable
 * and, worse, looks like a loop: the two `mod.rs` are different files in
 * different directories. Rust, TypeScript and Python all have this convention
 * and a repo in any of them has dozens of them, so the last segment alone is
 * the wrong label for exactly the files a chain most often passes through.
 *
 * Only the genuinely repeated names. `main.rs` and `lib.rs` look like they
 * belong here and do not: a crate has one of each, so qualifying them adds a
 * directory to a name that was already unique.
 */
const FOLDER_NAMED = /^(mod|index|__init__)\.[a-z]+$/;

/**
 * What to call a scope in one line.
 *
 * The last segment, except when that segment is the same word in fifty
 * directories — then the directory holding it, which is the part that
 * identifies it. The full path travels alongside in every list that renders
 * one of these, so this only has to be *distinguishing*, not complete.
 */
export function scopeLabel(path: string): string {
  const slash = path.lastIndexOf('/');
  const base = path.slice(slash + 1);
  // No directory to borrow from: a `mod.rs` at the repo root is already the
  // only one. Deriving the parent by arithmetic instead would slice into the
  // name itself and answer `main.r/main.rs`.
  if (slash < 0 || !FOLDER_NAMED.test(base)) return base;
  return `${lastSegment(path.slice(0, slash))}/${base}`;
}

/** Count what is under each side, and decide whether it is one file or many. */
function describeSides(nodes: readonly D3Node[], sides: readonly string[]): RelationSide[] {
  const entities = sides.map(() => 0);
  const files: Set<string>[] = sides.map(() => new Set<string>());
  for (const n of nodes) {
    if (!n.file_path || n.tags?.includes('ghost')) continue;
    const i = sideOfFile(n.file_path, sides);
    if (i === null) continue;
    entities[i]++;
    files[i].add(n.file_path);
  }
  return sides.map((path, i) => ({
    path,
    label: lastSegment(path),
    // A side is a file exactly when the only file under it is itself. A folder
    // holding one file is still a folder, and `files.has(path)` says so.
    grain: files[i].has(path) ? ('file' as const) : ('folder' as const),
    entities: entities[i],
    files: files[i].size,
  }));
}

// --- The walk -----------------------------------------------------------

const sourceIdOf = (l: D3Link): string =>
  typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
const targetIdOf = (l: D3Link): string =>
  typeof l.target === 'object' ? (l.target as D3Node).id : l.target;

type Counted = Map<string, Map<string, number>>;

function bump(m: Counted, from: string, to: string): void {
  let row = m.get(from);
  if (!row) {
    row = new Map();
    m.set(from, row);
  }
  row.set(to, (row.get(to) ?? 0) + 1);
}

interface FlowAcc {
  from: number;
  to: number;
  total: number;
  kinds: Map<string, KindTally>;
  pairs: EntityPair[];
  more: number;
}

interface Walk {
  /** scope → scope → edges. Every scope: sides, files and ghosts alike. */
  adj: Counted;
  /** The same edges reversed, so "who points at this" is one lookup. */
  rev: Counted;
  /** Keyed `from->to` by side index. */
  flows: Map<string, FlowAcc>;
  /** Ghost key → display name, so a chain step can be labelled without
   *  carrying the node it came from. */
  ghostNames: Map<string, string>;
}

function recordPair(walk: Walk, from: number, to: number, s: D3Node, t: D3Node, l: D3Link): void {
  const key = `${from}->${to}`;
  let acc = walk.flows.get(key);
  if (!acc) {
    acc = { from, to, total: 0, kinds: new Map(), pairs: [], more: 0 };
    walk.flows.set(key, acc);
  }
  acc.total++;
  const tally = acc.kinds.get(l.kind_raw);
  if (tally) tally.count++;
  else acc.kinds.set(l.kind_raw, { kind: l.kind_raw, label: l.kind, count: 1 });
  if (acc.pairs.length >= MAX_PAIRS) {
    acc.more++;
    return;
  }
  acc.pairs.push({
    sourceId: s.id,
    sourceName: s.name,
    sourceFile: s.file_path,
    targetId: t.id,
    targetName: t.name,
    targetFile: t.file_path,
    label: l.kind,
  });
}

/**
 * Project every edge onto the scope graph, in one pass.
 *
 * Lifted twins (UI-113) are dropped, for the reason `collapseGraph` drops them
 * above Entity grain: a twin says what its real edge already said, one node
 * further out, and both land on the same scope pair here. Counting both would
 * double every call written inside an `if`.
 */
function walkGraph(
  nodes: readonly D3Node[],
  links: readonly D3Link[],
  sides: readonly string[],
): Walk {
  const walk: Walk = { adj: new Map(), rev: new Map(), flows: new Map(), ghostNames: new Map() };
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const keyCache = new Map<string, string | null>();
  const keyOf = (n: D3Node): string | null => {
    let k = keyCache.get(n.id);
    if (k === undefined) {
      k = scopeKeyOf(n, sides);
      keyCache.set(n.id, k);
      if (k !== null && isGhostKey(k)) walk.ghostNames.set(k, n.name);
    }
    return k;
  };

  for (const l of links) {
    if (l.lifted_from) continue;
    const s = byId.get(sourceIdOf(l));
    const t = byId.get(targetIdOf(l));
    if (!s || !t) continue;
    const sk = keyOf(s);
    const tk = keyOf(t);
    if (sk === null || tk === null || sk === tk) continue;
    bump(walk.adj, sk, tk);
    bump(walk.rev, tk, sk);
    if (isSideKey(sk) && isSideKey(tk)) {
      recordPair(walk, sideIndexOf(sk), sideIndexOf(tk), s, t, l);
    }
  }
  return walk;
}

// --- Shared neighbours --------------------------------------------------

/** Neighbours of one side that are not themselves marked. */
function outsideNeighbours(m: Counted, side: number): Map<string, number> {
  const row = m.get(sideKey(side));
  if (!row) return new Map();
  const out = new Map<string, number>();
  for (const [k, n] of row) if (!isSideKey(k)) out.set(k, n);
  return out;
}

function stepOf(key: string, ghostNames: ReadonlyMap<string, string>): ChainStep {
  if (isGhostKey(key)) {
    const name = ghostNames.get(key) ?? key.slice(GHOST_TAG.length);
    return { key, path: name, label: name, external: true };
  }
  return { key, path: key, label: scopeLabel(key), external: false };
}

/**
 * Scopes every marked side touches, on the given side of the arrow.
 *
 * Intersection, not union: a dependency two of three marked scopes share is a
 * fact about those two, and reporting it here would let a reader conclude the
 * third stands on it as well. The pair case — which is nearly every case — is
 * the same computation with two sets.
 *
 * Repo files sort ahead of externals regardless of weight — see
 * `MAX_SHARED_EXTERNAL` for the measurement that forced it. Within each group
 * the heaviest goes first. The counts are taken before the cap, because a
 * verdict counting only what fits on screen is a wrong number.
 */
function sharedNeighbours(
  m: Counted,
  sideCount: number,
  ghostNames: ReadonlyMap<string, string>,
): SharedList {
  const rows = Array.from({ length: sideCount }, (_, i) => outsideNeighbours(m, i));
  const shared: SharedScope[] = [];
  for (const [key, n] of rows[0]) {
    const perSide = [n];
    for (let i = 1; i < rows.length; i++) {
      const c = rows[i].get(key);
      if (c === undefined) break;
      perSide.push(c);
    }
    if (perSide.length !== rows.length) continue;
    shared.push({
      ...stepOf(key, ghostNames),
      perSide,
      total: perSide.reduce((a, b) => a + b, 0),
    });
  }
  const byWeight = (a: SharedScope, b: SharedScope) =>
    b.total - a.total || a.path.localeCompare(b.path);
  const files = shared.filter((s) => !s.external).sort(byWeight);
  const external = shared.filter((s) => s.external).sort(byWeight);
  return {
    rows: [
      ...files.slice(0, MAX_SHARED),
      ...external.slice(0, Math.min(MAX_SHARED_EXTERNAL, MAX_SHARED - Math.min(files.length, MAX_SHARED))),
    ],
    files: files.length,
    external: external.length,
  };
}

// --- Chains -------------------------------------------------------------

/** The heaviest `CHAIN_FANOUT` neighbours, which is as far as the cross
 *  product below is allowed to look. */
function topNeighbours(m: Counted, side: number): Array<[string, number]> {
  return [...outsideNeighbours(m, side)]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, CHAIN_FANOUT);
}

function rankChains(chains: Chain[]): Chain[] {
  chains.sort((a, b) => b.strength - a.strength || a.via[0].path.localeCompare(b.via[0].path));
  return chains.slice(0, MAX_CHAINS);
}

/** Routes of exactly two hops: one mediator the source reaches and the target
 *  is reached from. */
function twoHop(walk: Walk, from: number, to: number, left: Array<[string, number]>, backBy: ReadonlyMap<string, number>): Chain[] {
  const found: Chain[] = [];
  for (const [key, n] of left) {
    const back = backBy.get(key);
    if (back === undefined) continue;
    found.push({ from, to, via: [stepOf(key, walk.ghostNames)], strength: Math.min(n, back) });
  }
  return found;
}

/** Routes of exactly three hops: an edge joining one of the source's
 *  neighbours to one of the target's. */
function threeHop(walk: Walk, from: number, to: number, left: Array<[string, number]>, right: Array<[string, number]>): Chain[] {
  const found: Chain[] = [];
  for (const [a, na] of left) {
    const row = walk.adj.get(a);
    if (!row) continue;
    for (const [b, nb] of right) {
      const mid = a === b ? undefined : row.get(b);
      if (mid === undefined) continue;
      found.push({
        from,
        to,
        via: [stepOf(a, walk.ghostNames), stepOf(b, walk.ghostNames)],
        strength: Math.min(na, mid, nb),
      });
    }
  }
  return found;
}

/**
 * Routes from one marked side to another, when no edge runs between them.
 *
 * Two levels, expanded from both ends rather than searched from one: the
 * mediators the source reaches, the mediators that reach the target, and then
 * the two ways those meet — the same scope in both lists (two hops), or an
 * edge from one list to the other (three). A breadth-first search would find
 * the same routes and would also have to be told when to stop; this is bounded
 * by construction, which is what lets it run on a click.
 *
 * Two hops win outright when there are any. A three-hop route is a longer way
 * to say what a two-hop route already said, and listing both would push the
 * short answer off the bottom of the panel.
 */
function chainsBetween(walk: Walk, from: number, to: number): Chain[] {
  const left = topNeighbours(walk.adj, from);
  const right = topNeighbours(walk.rev, to);
  const two = twoHop(walk, from, to, left, new Map(right));
  return rankChains(two.length > 0 ? two : threeHop(walk, from, to, left, right));
}

// --- Verdict ------------------------------------------------------------

type Partial = Omit<MarkRelation, 'tone' | 'verdict'>;

function toneOf(rel: Partial): RelationTone {
  if (rel.sides.length < 2) return 'none';
  if (rel.flows.length > 0) {
    const both = rel.flows.some((f) => rel.flows.some((g) => g.from === f.to && g.to === f.from));
    return both ? 'mutual' : 'one-way';
  }
  if (sharedSize(rel.sharedDeps) > 0 || sharedSize(rel.sharedDependents) > 0) return 'siblings';
  return rel.chains.length > 0 ? 'indirect' : 'independent';
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

const sharedSize = (l: SharedList): number => l.files + l.external;

/**
 * How the verdict names one shared list.
 *
 * Files are the finding, so they are what the sentence counts. Externals are
 * named only when there are no files at all — for a pair whose entire overlap
 * is the standard library, "they share nothing" would be false and "they share
 * 10 dependencies" would be worse.
 */
function sharedPart(l: SharedList, one: string, many: string): string | null {
  if (l.files > 0) return plural(l.files, one, many);
  if (l.external > 0) return `${l.external} external ${l.external === 1 ? one : many}`;
  return null;
}

function sharedPhrase(rel: Partial): string {
  return [
    sharedPart(rel.sharedDeps, 'dependency', 'dependencies'),
    sharedPart(rel.sharedDependents, 'dependent', 'dependents'),
  ].filter((p) => p !== null).join(' and ');
}

/**
 * The sentence at the top of the panel.
 *
 * Written for two sides, because two is what the gesture is for and a sentence
 * that hedges for N reads as a sentence about nothing. Three or more get the
 * arithmetic instead, which is the honest general statement.
 */
function verdictOf(tone: RelationTone, rel: Partial): string {
  if (tone === 'none') return NOTHING_MARKED;
  if (rel.sides.length > 2) {
    const linked = new Set(rel.flows.map((f) => [f.from, f.to].sort().join('-'))).size;
    const pairs = (rel.sides.length * (rel.sides.length - 1)) / 2;
    return `${rel.sides.length} marked · ${linked} of ${pairs} pairs directly linked`;
  }
  const [a, b] = rel.sides;
  if (tone === 'mutual') {
    return `${a.label} and ${b.label} depend on each other — the pair is a cycle.`;
  }
  if (tone === 'one-way') {
    const f = rel.flows[0];
    return `${rel.sides[f.from].label} depends on ${rel.sides[f.to].label}. Nothing comes back.`;
  }
  if (tone === 'siblings') {
    return `No edge runs between them — but they share ${sharedPhrase(rel)}.`;
  }
  if (tone === 'indirect') {
    const via = rel.chains[0].via;
    return `No edge runs between them. The shortest route is ${plural(via.length + 1, 'hop', 'hops')}, through ${via.map((v) => v.label).join(' → ')}.`;
  }
  return `Independent — no edge, nothing shared, and no route within ${MAX_HOPS} hops.`;
}

// --- Entry point --------------------------------------------------------

const EMPTY_SHARED: SharedList = { rows: [], files: 0, external: 0 };

function emptyRelation(sides: RelationSide[]): MarkRelation {
  return {
    sides,
    flows: [],
    sharedDeps: EMPTY_SHARED,
    sharedDependents: EMPTY_SHARED,
    chains: [],
    tone: 'none',
    verdict: NOTHING_MARKED,
  };
}

function orderedFlows(walk: Walk): DirectFlow[] {
  return [...walk.flows.values()]
    .map((f) => ({
      from: f.from,
      to: f.to,
      total: f.total,
      kinds: [...f.kinds.values()].sort((x, y) => y.count - x.count || x.kind.localeCompare(y.kind)),
      pairs: f.pairs,
      more: f.more,
    }))
    .sort((x, y) => y.total - x.total || x.from - y.from);
}

/** Chains for every ordered pair no direct edge already answers for. */
function allChains(walk: Walk, sideCount: number): Chain[] {
  const chains: Chain[] = [];
  for (let i = 0; i < sideCount; i++) {
    for (let j = 0; j < sideCount; j++) {
      if (i === j || walk.flows.has(`${i}->${j}`)) continue;
      chains.push(...chainsBetween(walk, i, j));
    }
  }
  return chains;
}

/**
 * Read the marked set as a relationship.
 *
 * `nodes`/`links` are the entity-level graph after `liftBodies` — what
 * `rawEntityGraph` holds and what `ensureFullData` returns for the whole repo.
 * Pass the latter: see the header on why the visible graph is the wrong input.
 *
 * Fewer than two marks *after compacting* is `tone: 'none'` rather than an
 * error. Marking a folder and one file inside it is a legitimate gesture that
 * happens to name one scope, and the panel rendering this should say so
 * instead of showing a relationship between a thing and itself.
 */
export function markRelation(
  nodes: readonly D3Node[],
  links: readonly D3Link[],
  marked: Iterable<string>,
): MarkRelation {
  const sidePaths = compactSides([...marked]);
  if (sidePaths.length < 2) return emptyRelation(describeSides(nodes, sidePaths));

  const walk = walkGraph(nodes, links, sidePaths);
  const partial: Partial = {
    sides: describeSides(nodes, sidePaths),
    flows: orderedFlows(walk),
    sharedDeps: sharedNeighbours(walk.adj, sidePaths.length, walk.ghostNames),
    sharedDependents: sharedNeighbours(walk.rev, sidePaths.length, walk.ghostNames),
    chains: allChains(walk, sidePaths.length),
  };
  const tone = toneOf(partial);
  return { ...partial, tone, verdict: verdictOf(tone, partial) };
}
