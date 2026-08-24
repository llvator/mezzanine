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
import { sizeChannel, colorChannel, sizeCurve, sizeBoost, sizeBins, activeTheme } from './settings';
import { diffChurnAt, diffChurnAvailable } from './diff';
import { NODE_COLORS } from '../types/graph';
import { buildNodeEncoding, sizeChannelsFor, type NodeEncoding } from '../viewmodels/nodeEncoding';

export const nodeEncoding = derived(
  [graphData, graphLevel, sizeChannel, colorChannel, sizeCurve, sizeBoost, sizeBins, activeTheme, diffChurnAvailable, diffChurnAt],
  ([$graphData, $graphLevel, $sizeChannel, $colorChannel, $sizeCurve, $sizeBoost, $sizeBins, $activeTheme, $churnAvailable, $churnAt]): NodeEncoding =>
    buildNodeEncoding($graphData.nodes, {
      sizeChannel: $sizeChannel,
      colorChannel: $colorChannel,
      // UI-106 — the shape and the width of the size ramp. Derived here with
      // the channels rather than read inside GraphView, so the canvas and the
      // legend cannot disagree about how big a circle should be; that
      // agreement is the whole reason this store exists.
      sizeCurve: $sizeCurve,
      sizeBoost: $sizeBoost,
      // UI-110 — how many size classes the ramp collapses to, continuous by
      // default. Same reason as the two above for living here: the legend has
      // to name the groups the canvas actually draws.
      sizeBins: $sizeBins,
      // Passed only while a diff is loaded, which is also what makes the
      // channel selectable — so the two can't disagree about whether "lines
      // changed" is a thing this canvas can say.
      churn: $churnAvailable ? $churnAt : undefined,
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

/** Size channels worth offering right now. Metrics with no meaningful rollup
 *  above entity level are withheld rather than silently reading zero, and so
 *  is `Lines changed` when there is no diff for it to measure. */
export const availableSizeChannels = derived(
  [graphLevel, diffChurnAvailable],
  ([$level, $diffLoaded]) => sizeChannelsFor($level, { diffLoaded: $diffLoaded }),
);
