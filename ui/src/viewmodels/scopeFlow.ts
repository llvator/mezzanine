/**
 * UI-146 — a scope's insides, projected onto the grain below it.
 *
 * `flowLayers` orders a set of ids. This is the part that decides *which* set,
 * and it is the whole of what makes the reading mean two different things at
 * two different scales:
 *
 * - a **folder** is read through the relationships between the things
 *   immediately inside it — its own files, and each subfolder as one unit;
 * - a **file** is read through the relationships between the entities it
 *   declares.
 *
 * Immediate children rather than the whole subtree, for the folder, and that
 * is the choice worth arguing with. `ui/src/viewmodels` holds fifty files, and
 * fifty rows in a hover panel is a directory listing, not a hierarchy. It is
 * also the reading `shapeView` already gives a folder — one circle per child,
 * subfolders included — so a reader who opens the Shape view after seeing this
 * ladder sees the same population, laid out differently.
 *
 * ## This reads the repo, not the canvas
 *
 * Its input is the entity-level graph as analysed, before any filter or
 * aggregation. That is deliberate and it is the opposite of `regionTraffic`,
 * which counts what is drawn and says so. Both are defensible; what would not
 * be is a *hierarchy* that rearranges itself when someone unticks a
 * relationship kind. Whether `stores` sits above `viewmodels` is a fact about
 * the code, and the panel presents it as one.
 *
 * Pure and store-free (`npm run test:scopeflow`).
 */

// Extensionless imports resolve under Vite and not under bare Node, and this
// module is unit tested (`npm run test:scopeflow`) — see the same `.ts` on
// every sibling that carries a test.
import type { D3Link, D3Node } from '../types/graph.ts';
import { flowLayers, emptyFlow, type FlowEdge, type FlowReading } from './flowLayers.ts';

/** The grain a subject is read AT — what the ladder's rows are made of. */
export type MemberGrain = 'folder' | 'file' | 'entity';

/** What the panel is asking about. */
export interface FlowSubject {
  grain: 'folder' | 'file';
  /** Directory path, or file path. `''` is the repo root, a folder like any
   *  other — never conflate it with "no subject". */
  path: string;
}

/** One row of the ladder. */
export interface FlowMember {
  /** The id `flowLayers` ordered: a path at folder grain, an entity id at
   *  file grain. */
  key: string;
  /** What the row is labelled with — the last path segment, or the entity
   *  name. */
  label: string;
  grain: MemberGrain;
  /** Entities rolled up into this row. 1 for an entity; the subtree's count
   *  for a subfolder, which is what makes a heavy child visible as one. */
  weight: number;
}

export interface ScopeFlow {
  subject: FlowSubject;
  /** 'file' when the subject is a folder, 'entity' when it is a file — the
   *  noun the panel uses so a sentence about a folder never says "entities". */
  memberNoun: 'file' | 'entity';
  /** Keyed by `FlowMember.key`. */
  members: Map<string, FlowMember>;
  reading: FlowReading;
}

export function emptyScopeFlow(subject: FlowSubject): ScopeFlow {
  return {
    subject,
    memberNoun: subject.grain === 'folder' ? 'file' : 'entity',
    members: new Map(),
    reading: emptyFlow(),
  };
}

const sourceIdOf = (l: D3Link): string =>
  typeof l.source === 'object' ? (l.source as D3Node).id : l.source;
const targetIdOf = (l: D3Link): string =>
  typeof l.target === 'object' ? (l.target as D3Node).id : l.target;

/**
 * Relationship kinds that say nothing about which way the flux runs.
 *
 * Containment is not dependency. A class does not *stand on* its methods and a
 * method does not stand on its parameters — they are the same thing described
 * at two grains, and counting them would put every class one layer above the
 * code it is made of, burying the call graph that the reading is actually
 * about. Everything else is kept, including `UsesType` and `Returns`: naming a
 * type in a signature is a real reason this cannot compile without that.
 *
 * `DependsOn` is deliberately absent from the exclusions — it is what
 * `collapseGraph` mints for every cross-scope edge, containment included, and
 * a folder that declares a submodule genuinely does sit above it.
 */
export const CONTAINMENT_KINDS: ReadonlySet<string> = new Set(['Contains', 'TakesParam']);

/** Does this edge carry the flux, or is it structure? */
export function carriesFlux(kindRaw: string): boolean {
  return !CONTAINMENT_KINDS.has(kindRaw);
}

/** Trailing slashes off, so a hull key and a hand-typed path compare equal —
 *  the same normalisation `regionSubject` applies, restated here rather than
 *  imported so this module stays free of the description types. */
export function normalizeFolder(path: string): string {
  let p = path.trim();
  while (p.startsWith('./')) p = p.slice(2);
  while (p.endsWith('/')) p = p.slice(0, -1);
  return p;
}

function lastSegment(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? path : path.slice(i + 1);
}

/**
 * The child of `folder` that holds `filePath`, or null when it holds none.
 *
 * Answers a *path*, so a file directly inside comes back as itself and a file
 * three directories down comes back as the subfolder at the top of that
 * descent. The one thing it must never answer is the folder itself: a
 * self-keyed member would collapse the whole ladder into one row.
 */
export function childUnder(folder: string, filePath: string): string | null {
  const dir = normalizeFolder(folder);
  const prefix = dir === '' ? '' : `${dir}/`;
  if (!filePath.startsWith(prefix)) return null;
  const rest = filePath.slice(prefix.length);
  if (rest === '') return null;
  const slash = rest.indexOf('/');
  return slash < 0 ? `${prefix}${rest}` : `${prefix}${rest.slice(0, slash)}`;
}

/** Is this node one of the file's own declarations, rather than something
 *  inside a function body or a synthetic ghost? Same population the canvas
 *  opens with (`structureOnly`), so the ladder and the default canvas agree
 *  about what a file contains. */
function isDeclaration(n: D3Node): boolean {
  if (n.body_of !== undefined) return false;
  if (n.tags?.includes('ghost')) return false;
  if (n.tags?.includes('template_var')) return false;
  return n.kind_raw !== 'Parameter';
}

/**
 * Is this a field of the type above it, rather than a peer of it?
 *
 * A field is a declaration — `structureOnly` keeps it, and rightly, because it
 * is part of what the file says. It is not a *rung*, though: the only edge it
 * has is the `Contains` from its type, and containment carries no flux, so
 * every field in the file lands isolated at layer 0. Measured on this repo's
 * own `flowLayers.ts`, that was thirteen interface properties against eight
 * things a reader would call an entity — the ladder read as a field list with
 * the functions buried in it.
 *
 * So a field is folded into its type, exactly as a branch is folded into its
 * function. Nothing is dropped: the type's weight counts it.
 */
function isField(n: D3Node): boolean {
  return n.kind_raw === 'Property' || (n.tags?.includes('class_field') ?? false);
}

/**
 * A rung of a file's ladder: a declaration that is neither a field nor a
 * synthetic scope, and that is not the file standing in for itself.
 *
 * Two exclusions past `isField`, both found by running this over the repo's
 * own graph rather than by reasoning:
 *
 * - **`Branch` and `Loop`.** They are internals by construction, and most of
 *   them are excluded already by carrying a `body_of`. Not all: a branch
 *   written inside `derived(…, () => { … })` has a *Variable* for a parent,
 *   and `bodyOwners` stops climbing at anything outside `BODY_KINDS` — so it
 *   arrives here unstamped. On `ui/src/stores/graph.ts` that was fourteen rows
 *   named `c1`, `c2`, `l1`.
 * - **the file's own module entity.** Several parsers emit one `Module` named
 *   after the file, and it is the file, not a thing inside it. Matched on the
 *   name because that is what makes it recognisable without a second index;
 *   a Rust `mod foo;` declaration inside `bar.rs` is named `foo` and stays.
 */
function isRung(n: D3Node, fileLabel: string): boolean {
  if (!isDeclaration(n) || isField(n)) return false;
  if (n.kind_raw === 'Branch' || n.kind_raw === 'Loop') return false;
  if (n.kind_raw === 'Module' && n.name === fileLabel) return false;
  return true;
}

/**
 * Read one scope's insides.
 *
 * `nodes`/`links` are the entity-level graph after `liftBodies` — which is
 * what `rawEntityGraph` holds. The lift is load-bearing at file grain: a call
 * written inside an `if` hangs off a `Branch`, not off the function, so
 * without the twin edge a file's functions would look almost unrelated to each
 * other. Both the real edge and its twin project onto the same pair here and
 * the dedup in `flowLayers` merges them, so nothing needs to know which is
 * which.
 */
export function scopeFlow(
  nodes: readonly D3Node[],
  links: readonly D3Link[],
  subject: FlowSubject,
): ScopeFlow {
  const rows = subject.grain === 'folder'
    ? folderRows(nodes, normalizeFolder(subject.path))
    : fileRows(nodes, subject.path);
  if (rows.members.size === 0) return emptyScopeFlow(subject);
  return {
    subject,
    memberNoun: subject.grain === 'folder' ? 'file' : 'entity',
    members: rows.members,
    reading: flowLayers([...rows.members.keys()], fluxEdges(links, rows.rowOf)),
  };
}

/** The ladder's rows, and the routing every edge is projected through. */
interface ScopeRows {
  members: Map<string, FlowMember>;
  /** node id → the row it belongs to. */
  rowOf: Map<string, string>;
}

/**
 * A folder's immediate children, one row each.
 *
 * Membership is by FILE, so every entity a file holds is routed — internals
 * included. A branch written at module level carries no `body_of` and gets no
 * lifted twin, so filtering to declarations here would silently drop the only
 * edge some top-level code has.
 *
 * The `weight` still counts declarations only: "14 files" beside a number that
 * also counted every branch arm would not be a size a reader could check
 * against the Quality panel.
 */
function folderRows(nodes: readonly D3Node[], folder: string): ScopeRows {
  const members = new Map<string, FlowMember>();
  const rowOf = new Map<string, string>();
  for (const n of nodes) {
    if (!n.file_path) continue;
    if (n.tags?.includes('ghost') || n.tags?.includes('template_var')) continue;
    const child = childUnder(folder, n.file_path);
    if (child === null) continue;
    rowOf.set(n.id, child);
    const existing = members.get(child);
    if (existing) existing.weight += isDeclaration(n) ? 1 : 0;
    else members.set(child, {
      key: child,
      label: lastSegment(child),
      // A child is a file exactly when the path that produced it is the whole
      // file path; anything shorter is a directory on the way down.
      grain: child === n.file_path ? 'file' : 'folder',
      weight: isDeclaration(n) ? 1 : 0,
    });
  }
  return { members, rowOf };
}

/** A file's declarations, one row each, with everything else folded in. */
function fileRows(nodes: readonly D3Node[], file: string): ScopeRows {
  const fileLabel = lastSegment(file);
  const inFile = nodes.filter((n) => n.file_path === file);
  const members = new Map<string, FlowMember>();
  const rowOf = new Map<string, string>();
  for (const n of inFile) {
    if (!isRung(n, fileLabel)) continue;
    rowOf.set(n.id, n.id);
    members.set(n.id, { key: n.id, label: n.name, grain: 'entity', weight: 1 });
  }
  // `body_of` and `parent_id` both speak `original_id`; links speak `id`.
  const byOriginal = new Map(inFile.map((n) => [n.original_id, n]));
  for (const n of inFile) {
    if (rowOf.has(n.id)) continue;
    foldInto(n, { members, rowOf }, byOriginal, fileLabel);
  }
  return { members, rowOf };
}

/** How far a `parent_id` chain is followed before it is assumed to loop. The
 *  same bargain `bodyOwners` makes: cheaper to cap than to prove impossible. */
const MAX_CLIMB = 32;

/**
 * Route `n` to the rung that owns it — a branch to its function through
 * `body_of`, a field to its type through `parent_id`.
 *
 * One walk for both, because a field of a class nested in a function has to
 * pass through each in turn. A node that reaches the top without meeting a
 * rung gets no row, and its edges are dropped: that is code belonging to the
 * file itself rather than to anything inside it.
 */
function foldInto(
  n: D3Node,
  rows: ScopeRows,
  byOriginal: Map<string, D3Node>,
  fileLabel: string,
): void {
  let current: D3Node | undefined = n;
  for (let step = 0; step < MAX_CLIMB && current; step++) {
    const upId: string | null = current.body_of ?? current.parent_id;
    const up: D3Node | undefined = upId ? byOriginal.get(upId) : undefined;
    if (!up || up.original_id === current.original_id) return;
    if (isRung(up, fileLabel)) {
      rows.rowOf.set(n.id, up.id);
      const row = rows.members.get(up.id);
      if (row) row.weight++;
      return;
    }
    current = up;
  }
}

/** Every relationship that carries flux, projected onto the rows. */
function fluxEdges(links: readonly D3Link[], rowOf: Map<string, string>): FlowEdge[] {
  const edges: FlowEdge[] = [];
  for (const l of links) {
    if (!carriesFlux(l.kind_raw)) continue;
    const s = rowOf.get(sourceIdOf(l));
    const t = rowOf.get(targetIdOf(l));
    if (s === undefined || t === undefined || s === t) continue;
    edges.push({ source: s, target: t });
  }
  return edges;
}

/**
 * Where the subject itself stands among its siblings.
 *
 * The ladder says what is inside; this says what the thing is inside OF, which
 * is the half a reader needs to place a folder in the repo rather than only to
 * read it internally. It is the same computation one level up — run
 * `scopeFlow` on the parent and look the subject up — so the two numbers a
 * panel prints are guaranteed to be the same kind of number.
 *
 * Null at the repo root, which has no parent and therefore no siblings. Null
 * too when the parent holds only one child, because "layer 1 of 1" is a fact
 * about arithmetic rather than about the code.
 */
export function siblingStanding(
  nodes: readonly D3Node[],
  links: readonly D3Link[],
  subject: FlowSubject,
): { parent: string; flow: ScopeFlow; key: string } | null {
  const path = subject.grain === 'folder' ? normalizeFolder(subject.path) : subject.path;
  if (path === '') return null;
  const slash = path.lastIndexOf('/');
  const parent = slash < 0 ? '' : path.slice(0, slash);
  const flow = scopeFlow(nodes, links, { grain: 'folder', path: parent });
  if (flow.members.size < 2) return null;
  if (!flow.members.has(path)) return null;
  return { parent, flow, key: path };
}
