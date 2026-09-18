/**
 * The Description mode: read the graph as prose instead of as topology.
 *
 * Sits alongside "follow selection to editor" as the other way of answering
 * "what am I looking at". Where that mode jumps the editor to a node's
 * source, this one shows the node's description followed by its ancestors' —
 * so skimming the canvas with the pointer narrates the graph.
 *
 * Hover wins while the pointer is over a node; when it leaves, the pane
 * falls back to the selection rather than blanking, so a pinned node stays
 * readable. `describeOnHover` off makes it selection-only.
 *
 * Since UI-141 a *region's name* is a subject too. A folder has no
 * description of its own — what describes it is a spec entity's `d:`
 * reaching it through a `cr:` — so the chain climbs directories and each
 * rung says who wrote what is under it.
 */
import { derived, writable } from 'svelte/store';
import type { D3Node } from '../types/graph';
import { hoveredNode, selectedNode, rawEntityGraph } from './graph';
import { hoveredRegion, selectedRegion, regionClaimOf } from './region';
import { ensureDetailsLoaded } from './details';
import { buildChildEntries, buildDescriptionChain, type DescriptionEntry } from '../viewmodels/descriptionChain';
import { regionChainEntries, type HoveredRegion } from '../viewmodels/regionSubject';
import type { RegionSpecClaim } from '../viewmodels/regionSpec';

/** Whether hovering a node retargets the pane. Off ⇒ selection only.
 *
 *  Deliberately *not* persisted: in VS Code this is driven by a checkbox in
 *  the native Controls view, which is stateless chrome rendered from a fixed
 *  default. A remembered `false` here would leave that checkbox ticked while
 *  hover did nothing, and the desync is worse than re-defaulting to on. */
export const describeOnHover = writable(true);

// Whether the pane itself is expanded lives in `stores/panes.ts`, with the
// other columns — the layout has to weigh all three widths against the
// canvas at once.

export interface DescriptionState {
  /** What put this chain on screen — the pane labels it so a moving pane
   *  and a pinned one are never confused. */
  source: 'hover' | 'selection';
  chain: DescriptionEntry[];
  /** Direct children of the subject — the chain's head — in declaration
   *  order. The chain only ever climbs, so without this a Feature read as
   *  its own description plus its parents' and never mentioned the
   *  Functionalities that are most of what it means. */
  children: DescriptionEntry[];
}

export const description = writable<DescriptionState | null>(null);

/**
 * What the pane is about: an entity, or a region's name (UI-141).
 *
 * A folder is the one subject on the canvas with no entity behind it, so it
 * cannot be squeezed into the node case — its prose is a spec entity's,
 * reaching it through a `cr:`, and the chain that explains it climbs
 * directories rather than `parent_id`.
 */
type Subject =
  | { kind: 'node'; node: D3Node; source: 'hover' | 'selection'; nodes: D3Node[] }
  | {
      kind: 'region';
      region: HoveredRegion;
      source: 'hover' | 'selection';
      claimOf: (path: string) => RegionSpecClaim | null;
    };

const subject = derived(
  [
    hoveredNode, selectedNode, describeOnHover, rawEntityGraph,
    hoveredRegion, selectedRegion, regionClaimOf,
  ],
  ([$hovered, $selected, $onHover, $graph, $region, $pinnedRegion, $claimOf]): Subject | null => {
    // Hover before selection, which is this pane's rule and the opposite of
    // the Details column's — reading here happens *while* skimming, and a
    // pinned node that outranked the pointer would stop the narration. A
    // region ranks with the node hover for the same reason: it is something
    // the pointer is on right now.
    if ($onHover && $hovered) {
      return { kind: 'node', node: $hovered, source: 'hover', nodes: $graph.nodes };
    }
    if ($onHover && $region) {
      return { kind: 'region', region: $region, source: 'hover', claimOf: $claimOf };
    }
    if ($selected) {
      return { kind: 'node', node: $selected, source: 'selection', nodes: $graph.nodes };
    }
    // A pinned region falls back with the pinned node and for the same
    // reason (UI-148): the pointer has left the canvas — typically for this
    // very pane — and the prose a reader clicked a folder's name to read
    // should still be here when they arrive.
    if ($pinnedRegion) {
      return { kind: 'region', region: $pinnedRegion, source: 'selection', claimOf: $claimOf };
    }
    return null;
  },
);

/** Guards against an out-of-order details fetch overwriting a newer hover. */
let generation = 0;

subject.subscribe((current) => {
  const mine = ++generation;
  if (!current) {
    description.set(null);
    return;
  }
  if (current.kind === 'region') {
    // No await: the claims already carry whatever the sidecar has loaded, and
    // `regionClaimOf` is derived from it — so a chain built before the
    // descriptions arrived is rebuilt with them the moment they do, rather
    // than resolving late against a region the pointer has left.
    description.set({
      source: current.source,
      chain: regionChainEntries(current.region, current.claimOf),
      // A folder's contents are the scope tree, and that pane already exists.
      // What this one adds is the prose, which the chain carries.
      children: [],
    });
    return;
  }
  void ensureDetailsLoaded().then((docs) => {
    if (mine !== generation) return;
    description.set({
      source: current.source,
      chain: buildDescriptionChain(current.node, current.nodes, docs),
      children: buildChildEntries(current.node, current.nodes, docs),
    });
  });
});
