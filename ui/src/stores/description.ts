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
 */
import { derived, writable } from 'svelte/store';
import { hoveredNode, selectedNode, rawEntityGraph } from './graph';
import { ensureDetailsLoaded } from './details';
import { buildDescriptionChain, type DescriptionEntry } from '../viewmodels/descriptionChain';

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
}

export const description = writable<DescriptionState | null>(null);

const subject = derived(
  [hoveredNode, selectedNode, describeOnHover, rawEntityGraph],
  ([$hovered, $selected, $onHover, $graph]) => {
    const hovering = $onHover && $hovered !== null;
    const node = hovering ? $hovered : $selected;
    if (!node) return null;
    return { node, source: (hovering ? 'hover' : 'selection') as 'hover' | 'selection', nodes: $graph.nodes };
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
  void ensureDetailsLoaded().then((docs) => {
    if (mine !== generation) return;
    description.set({
      source: current.source,
      chain: buildDescriptionChain(current.node, current.nodes, docs),
    });
  });
});
