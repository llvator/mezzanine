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
