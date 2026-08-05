/**
 * UI-014 — the active node encoding, as one derived store.
 *
 * The canvas and the legend have to agree about what a circle's size and
 * colour mean, and the surest way to get that is for them to read the same
 * object rather than each rebuild it from the same inputs. GraphView renders
 * from this; FilterPanel's Legend describes this.
 *
 * The size domain comes from the nodes in play, so the encoding is a function
 * of the graph as well as of the chosen channels — hence a derived store
 * rather than a constant.
 */

import { derived } from 'svelte/store';
import { graphData, graphLevel } from './graph';
import { sizeChannel, colorChannel, activeTheme } from './settings';
import { NODE_COLORS } from '../types/graph';
import { buildNodeEncoding, sizeChannelsFor, type NodeEncoding } from '../viewmodels/nodeEncoding';

export const nodeEncoding = derived(
  [graphData, graphLevel, sizeChannel, colorChannel, activeTheme],
  ([$graphData, $graphLevel, $sizeChannel, $colorChannel, $activeTheme]): NodeEncoding =>
    buildNodeEncoding($graphData.nodes, {
      sizeChannel: $sizeChannel,
      colorChannel: $colorChannel,
      // The domain of the `degree` channel: the collapsed graph's links —
      // current level, currently-open scopes. Deliberately the same source
      // as `$graphData.nodes` above, one line up, so size is measured over
      // exactly the population it is scaled against. That means a link the
      // relationship filters hide still counts, which is how every other
      // channel behaves too: `loc` does not shrink when a node is dimmed.
      links: $graphData.links,
      level: $graphLevel,
      theme: $activeTheme,
      kindColors: NODE_COLORS,
      kindColorFallback: NODE_COLORS.Unknown,
    }),
);

/** Size channels worth offering at the current aggregation level. Metrics
 *  with no meaningful rollup above entity level are withheld rather than
 *  silently reading zero. */
export const availableSizeChannels = derived(graphLevel, ($level) => sizeChannelsFor($level));
