/**
 * The region whose name the pointer is on — UI-141.
 *
 * Its own store rather than a field on `hoveredNode`, because a region is not
 * a node and every consumer of that store would have to start asking which it
 * had. The two are hover state all the same, so the rules that govern one
 * govern this: a frozen preview (`L`) outranks it, and it clears whenever the
 * canvas stops drawing the thing it names — see `hoveredNode`'s own guard in
 * `stores/graph`, which exists for the same reason.
 *
 * Written only by GraphView, which is the only place that knows what the
 * pointer is over. Read by the Details column (`FolderInfo`) and by the
 * Description pane, so both answer "what am I looking at" for a folder with
 * one definition of what the folder is.
 */
import { derived, get, writable, type Readable } from 'svelte/store';
import { hoverLocked, selectedNode } from './graph';
import { specGraph } from './crossFilter';
import { detailsMap, ensureDetailsLoaded } from './details';
import {
  documentationLookup,
  regionSpecClaim,
  type RegionSpecClaim,
} from '../viewmodels/regionSpec';
import type { HoveredRegion } from '../viewmodels/regionSubject';

export const hoveredRegion = writable<HoveredRegion | null>(null);

/**
 * The region a click has pinned into the Details column — UI-148.
 *
 * `selectedNode`'s counterpart for the one subject on the canvas that is not
 * a node, and it exists for the same reason that store does: a hover ends the
 * moment the pointer moves, and reading a file's rollup means moving the
 * pointer off the name and into the column that holds it. Until this, the one
 * gesture that gave a region a durable subject was the drill — and the drill
 * *cleared* the hover on its way out, so clicking a file's name emptied the
 * very pane a reader was clicking it to fill.
 *
 * Survives a redraw where the hover does not (`keepHoveredRegionAmong`). That
 * is the difference between the two: the hover is a claim about where the
 * pointer is, and the canvas can invalidate it; a pin is a claim about what
 * the reader is reading, and the focus it rides along with is *expected* to
 * replace the picture underneath. `selectedNode` outlives a rebuild for the
 * same reason.
 */
export const selectedRegion = writable<HoveredRegion | null>(null);

/**
 * Pin a region into the Details column, releasing any pinned entity.
 *
 * Mutually exclusive on purpose. Both are "the thing I am reading", the
 * column shows one at a time, and a precedence between two live pins would be
 * a rule the reader has to learn in order to predict what a click does. One
 * pin, whichever was asked for last.
 */
export function pinRegion(region: HoveredRegion): void {
  selectedNode.set(null);
  selectedRegion.set(region);
}

/** Drop the pin and hand the column back to the pointer. */
export function unpinRegion(): void {
  selectedRegion.set(null);
}

/**
 * The other half of the exclusion, kept here rather than in `focusNode` so
 * every route to a selection is covered — the canvas click, the search hit,
 * the relationship row, a Description rung. `stores/graph` knows nothing of
 * regions, and teaching it would put the dependency the wrong way round.
 */
selectedNode.subscribe((node) => {
  if (node) selectedRegion.set(null);
});

/**
 * Point at a region's name, or at nothing.
 *
 * The lock is honoured here rather than at each call site for the reason
 * `focusNode` honours it: `L` is the one hover a reader asked to keep, and a
 * region name is a target the pointer crosses on its way to the panel that
 * froze it.
 *
 * `ensureDetailsLoaded` is kicked off on the way past. The descriptions live
 * in the sidecar, and asking for them here means the first folder a reader
 * hovers in a session shows its prose on that hover rather than the next one.
 */
export function setHoveredRegion(region: HoveredRegion | null): void {
  if (get(hoverLocked)) return;
  if (region) void ensureDetailsLoaded();
  hoveredRegion.set(region);
}

/**
 * Drop the hover unless the canvas still draws a region at `path`.
 *
 * `mouseleave` never fires when a re-render removes the element the pointer
 * was over, so without this a level change, a filter, or a scope drill would
 * leave both panels describing a folder that is no longer outlined anywhere —
 * exactly the failure `graphData.subscribe` guards `hoveredNode` against.
 */
export function keepHoveredRegionAmong(paths: ReadonlySet<string>): void {
  const current = get(hoveredRegion);
  if (current && !paths.has(current.path)) hoveredRegion.set(null);
}

/**
 * What the spec says about any path, given the graph and sidecar loaded now.
 *
 * A function rather than a resolved claim because the Description pane asks
 * about its region *and every folder above it* — an inherited claim is only
 * honest on the rung that owns it (`regionChainEntries`), so one answer would
 * not do. Derived, so a chain built before the sidecar arrived is rebuilt
 * with its descriptions once it does.
 *
 * `FolderInfo` asks it about the region it was handed rather than about the
 * hovered one, which is what lets the same panel serve a pinned region
 * (UI-148) — a claim derived from the hover would have gone on answering for
 * whatever the pointer wandered over next.
 */
export const regionClaimOf: Readable<(path: string) => RegionSpecClaim | null> = derived(
  [specGraph, detailsMap],
  ([$graph, $docs]) => {
    const documentationOf = documentationLookup($docs);
    return (path: string) => regionSpecClaim($graph, path, documentationOf);
  },
);

