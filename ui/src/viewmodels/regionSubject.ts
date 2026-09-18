/**
 * UI-141 — the region under the pointer, as a subject the panels can read.
 *
 * At File level the canvas draws folders as *regions*, not as nodes: an
 * outline and a name (UI-055). Every panel that answers "what am I looking
 * at" keys off a node, so a reader hovering the one thing on the canvas that
 * names a folder got the cursor-anchored card and nothing else — the Details
 * column and the Description pane both went on describing whatever was last
 * under the pointer, which is a file inside the folder being asked about.
 *
 * The card cannot be the answer to this. It is a hover surface, deliberately
 * capped in height (`clampDescription`, the 300px budget in GraphView), and
 * it disappears the moment the pointer moves toward it. The columns are where
 * a reader already looks for metrics and prose, and they have the room.
 *
 * Pure and store-free (`npm run test:region-subject`) so the two rules worth
 * arguing with — which rollup row describes a region, and which descriptions
 * are honestly *about* it — can be pinned in a test rather than through a
 * browser.
 */

import type { ScopeMetrics } from '../types/graph.ts';
import type { DescriptionEntry } from './descriptionChain.ts';
import type { RegionSpecClaim } from './regionSpec.ts';
import type { RegionTraffic } from './regionTraffic.ts';
import { ancestorDirs } from './qualityPopulation.ts';

/** What a region groups by, at the grain now in force (UI-103). A folder at
 *  every level; a file too, once the reader groups entities by their file. */
export type RegionGrain = 'folder' | 'file';

/**
 * The region whose *name* the pointer is on, as GraphView publishes it.
 *
 * The name and not the area, which is the whole of what makes this safe to
 * put in a column: a hull covers most of the canvas, so an area hover would
 * retarget both panels on the way to anything else. The name is a few dozen
 * pixels of text that has advertised itself as a target since UI-055.
 */
export interface HoveredRegion {
  /** Full group key — a directory path, or a file path at file grain. */
  path: string;
  /** What the outline is labelled with: the last segment. */
  label: string;
  grain: RegionGrain;
  /** Members the hull was built from. The fallback for `membership` when the
   *  traffic count has not been recomputed for this region yet. */
  size: number;
  /** What the region holds and what its relationships do, or null before the
   *  first count. Snapshotted rather than looked up on demand: the map it
   *  comes from is rebuilt per draw and lives inside the canvas component. */
  traffic: RegionTraffic | null;
}

/** The part of `stores/quality`'s `ScopeRow` this module needs. Structural on
 *  purpose — a viewmodel that imported the store could not be tested without
 *  dragging the whole panel behind it. */
export interface ScopeRowLike {
  scope: ScopeMetrics;
}

/**
 * What the Details column is about, once a region can be pinned there as well
 * as pointed at (UI-148).
 *
 * Generic over the node for the reason `ScopeRowLike` is structural: this
 * module is tested under bare Node, and `D3Node` drags d3's simulation types
 * behind it.
 */
export type DetailSubject<N> =
  | { kind: 'node'; node: N; pinned: boolean }
  | { kind: 'region'; region: HoveredRegion; pinned: boolean };

/**
 * Which of the four candidates the Details column shows.
 *
 * Two tiers, and the boundary between them is the whole rule: anything the
 * reader *asked* to hold outranks anything the pointer merely happens to be
 * over. Within each tier a node outranks a region, which is the older rule —
 * a region is drawn behind the nodes in it, so the pointer is over both
 * whenever it is over one, and the finer subject is the one that was aimed at.
 *
 * The two pins are mutually exclusive at the store (`pinRegion` clears the
 * node, selecting a node clears the region), so their order here is a
 * formality rather than a decision. It is written down anyway: a precedence
 * that only holds because of an invariant elsewhere is one refactor away from
 * being wrong silently.
 */
export function detailSubject<N>(state: {
  selectedNode: N | null;
  selectedRegion: HoveredRegion | null;
  hoveredNode: N | null;
  hoveredRegion: HoveredRegion | null;
}): DetailSubject<N> | null {
  if (state.selectedNode) return { kind: 'node', node: state.selectedNode, pinned: true };
  if (state.selectedRegion) return { kind: 'region', region: state.selectedRegion, pinned: true };
  if (state.hoveredNode) return { kind: 'node', node: state.hoveredNode, pinned: false };
  if (state.hoveredRegion) return { kind: 'region', region: state.hoveredRegion, pinned: false };
  return null;
}

/**
 * Trailing slashes and a `./` prefix off, so a group key and a rollup path
 * that name the same directory compare equal.
 *
 * The engine emits `ui/src/stores`, the hulls key on the same string, and
 * neither is normalized on the way here — but a saved view, a scope rule or
 * a hand-typed path reaches the same lookup wearing a slash. Failing that
 * comparison shows up as a folder with no metrics at all, which reads as
 * "the engine measured nothing here" rather than as a spelling difference.
 */
export function normalizeRegionPath(path: string): string {
  let p = path.trim();
  while (p.startsWith('./')) p = p.slice(2);
  while (p.endsWith('/')) p = p.slice(0, -1);
  return p;
}

/**
 * The rollup row describing `path`, or null when the population holds none.
 *
 * Null is a real answer and not a lookup failure: the Quality panel's rows
 * follow its own population (`analysisGraph`), so a folder outside the
 * analysis scope — or one whose files were all filtered away — genuinely has
 * no row, and the panel says so rather than showing zeroes that would read as
 * a measured emptiness.
 */
export function scopeRowFor<T extends ScopeRowLike>(
  rows: readonly T[],
  path: string,
): T | null {
  const subject = normalizeRegionPath(path);
  for (const row of rows) {
    if (normalizeRegionPath(row.scope.path) === subject) return row;
  }
  return null;
}

/** How many of this region's members reached the canvas, and how it should be
 *  read. Deliberately the same two numbers the card shows — the column and the
 *  card have to agree about a region a filter has thinned out. */
export function regionMembership(region: HoveredRegion): {
  drawn: number;
  total: number;
  hidden: number;
} {
  const t = region.traffic;
  if (!t) return { drawn: region.size, total: region.size, hidden: 0 };
  return { drawn: t.drawn, total: t.total, hidden: Math.max(0, t.total - t.drawn) };
}

/** `ui/src/stores` → `stores`. Local rather than `folderHulls.basename` so
 *  this module stays free of d3 and testable under bare Node. */
function lastSegment(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? path : path.slice(i + 1);
}

/** Same stop the entity chain uses, for the same reason: a chain deep enough
 *  to need a scrollbar has stopped being an explanation. */
const MAX_RUNGS = 8;

/**
 * The region read as prose: itself, then every folder above it that a spec
 * entity actually describes.
 *
 * Two rules, and both are about not putting words in the author's mouth:
 *
 * - A rung carries a description only when the claim on it is **exact**. A
 *   `cr: "ui/"` genuinely covers `ui/src/stores`, but it is a sentence about
 *   `ui` — printing it under `stores` would assert something the author never
 *   wrote. The claim still reaches the reader, on the rung for `ui`, where it
 *   is true.
 * - A described rung says **who** describes it (`attribution`). "Feature
 *   Region grouping" and "Concept Region grouping" read very differently, and
 *   a folder has no prose of its own for the reader to weigh it against.
 *
 * Ancestors with no exact claim are dropped rather than rendered empty: a
 * chain of five "No description." rungs hides the one rung that says
 * something. The subject itself is always the first rung, described or not —
 * the pane has to name what the pointer is on.
 *
 * `claimOf` is injected for the reason `regionSpecClaim` takes its lookup that
 * way: the descriptions live in the `/api/details` sidecar, so a version of
 * this that fetched them could not be tested without a server.
 */
export function regionChainEntries(
  region: HoveredRegion,
  claimOf: (path: string) => RegionSpecClaim | null,
): DescriptionEntry[] {
  const path = normalizeRegionPath(region.path);
  const entries: DescriptionEntry[] = [rungFor(path, region.label, region.grain, 0, claimOf(path))];

  // Innermost first, and the root (`''`) dropped: `regionSpecClaim` answers
  // null for an empty subject anyway, and "(root)" as the last rung of every
  // chain is a heading nobody wrote.
  const ancestors = ancestorDirs(path).filter((d) => d !== '').reverse();
  for (const dir of ancestors) {
    if (entries.length >= MAX_RUNGS) break;
    const claim = claimOf(dir);
    if (!claim?.exact) continue;
    entries.push(rungFor(dir, lastSegment(dir), 'folder', entries.length, claim));
  }
  return entries;
}

/** The id prefix a region rung carries. Owned here, with the code that
 *  writes it, so the panel asking "is this rung selectable?" does not have to
 *  know the spelling. */
const REGION_ID_PREFIX = 'region:';

/** Whether a chain rung stands for a region rather than an entity — which is
 *  to say, whether clicking it could select anything. Nothing in the graph
 *  answers to a folder, so the panel renders these as plain text instead of
 *  as a button that promises a selection it cannot make. */
export function isRegionEntry(entry: Pick<DescriptionEntry, 'entityId'>): boolean {
  return entry.entityId.startsWith(REGION_ID_PREFIX);
}

/**
 * One rung.
 *
 * `filePath` is left empty on purpose. The panel renders it as a source
 * location and the VS Code host turns a click on it into `goToDefinition` —
 * handed a directory, that host would try to open a folder as a file. The
 * path is still carried, as `qualifiedName`, which is where the panel's
 * tooltip reads from.
 */
function rungFor(
  path: string,
  label: string,
  grain: RegionGrain,
  depth: number,
  claim: RegionSpecClaim | null,
): DescriptionEntry {
  const described = claim?.exact ? claim : null;
  return {
    // Prefixed so it cannot collide with an entity id: the panel looks this
    // up in the graph to decide what a click selects, and a folder is not
    // selectable — a bare path could match a File node and silently pin one.
    entityId: `${REGION_ID_PREFIX}${path}`,
    name: label || '(root)',
    qualifiedName: path,
    kind: grain === 'file' ? 'File' : 'Folder',
    filePath: '',
    line: 0,
    documentation: described?.description ?? null,
    attribution: described ? `${described.kind} ${described.name}` : undefined,
    depth,
  };
}
