<script lang="ts">
  /**
   * The view controls above the canvas.
   *
   * Extracted from App.svelte, which carried ~110 lines of markup and ~60 of
   * CSS for this in a 1400-line file (UI-013).
   *
   * Every control sits in a named cluster. The name is the point: `H1 H2 H3`
   * and `L1 L2 L3` were two-character codes whose meaning lived only in a
   * `title` tooltip. Captions sit *above* their segments rather than beside
   * them — at 1280px the toolbar has ~125px of slack, and inline captions
   * would have cost ~200px and pushed it to a third row, which is the
   * problem this ticket started from.
   *
   * Zoom and Fit are separate clusters for the same reason: a cluster cannot
   * split across rows, so one six-button Viewport cluster wastes more space
   * at the wrap point than it saves.
   */
  import type GraphView from './GraphView.svelte';
  import StatsBar from './StatsBar.svelte';
  import {
    selectedNode, viewMode, graphLevel,
    showLabels, showKindLabels, showLinkLabels,
    treeDensity, treeMaxDepth, hoverDepth, hoverMode,
    setTreeDepth, cycleTreeDensity, DENSITY_LABELS,
  } from '../stores/graph';
  import { autoLevel, drillIntoMarks, markedStats } from '../stores/scope';
  import { clearMarks, markCount } from '../stores/marks';
  import { autoFitView } from '../stores/settings';
  import type { GraphLevel } from '../types/graph';
  import { HOVER_MODES, HOVER_MODE_LABELS, HOVER_MODE_TITLES } from '../viewmodels/hoverHighlight';
  import { toolbarCollapsed, splitViewOpen } from '../stores/panes';
  import { focusedPane } from '../stores/keymap';
  import { specGraph } from '../stores/crossFilter';

  /** Bound instance of the graph, for the viewport actions. Undefined until
   *  App's `bind:this` lands, which is after this component's first render. */
  export let graphView: GraphView | undefined = undefined;

  /** Collapse state, persisted. Expanded by default so the controls stay
   *  discoverable; the point of collapsing is reclaiming canvas on a small
   *  window, which is a choice the user makes, not one to make for them.
   *
   *  A store since UI-075, under the same localStorage key: focusing this pane
   *  with `2` has to unfold it, and `c` has to fold it back. */
  $: collapsed = $toolbarCollapsed;
</script>

<header class="canvas-toolbar" class:collapsed
  class:pane-focused={$focusedPane === 'view'}
  data-pane="view"
  data-probe="canvas-toolbar">
  <div class="canvas-toolbar-bar">
    <button
      type="button"
      class="canvas-toolbar-handle"
      aria-expanded={!collapsed}
      title={collapsed ? 'Show view controls' : 'Hide view controls'}
      on:click={() => toolbarCollapsed.set(!collapsed)}
    >
      <span class="toolbar-chev">{collapsed ? '▶' : '▼'}</span>
      View
      {#if collapsed}
        <!-- Collapsed, the bar still has to say what it is hiding, or the
             active level and view mode become invisible state. -->
        <span class="toolbar-summary">
          {$viewMode === 'graph' ? 'Graph' : 'Tree'} · {$graphLevel}
          {$autoFitView ? ' · auto-fit' : ''}
          <!-- A marked set has a ring on the canvas but its only *control* is
               inside this bar, so a collapsed toolbar would leave the reader
               with a gesture they could make and not spend. -->
          {$markCount > 0 ? ` · ${$markCount} marked` : ''}
        </span>
      {/if}
    </button>
    <!-- Counts live here rather than floating bottom-left. Down there they
         covered nodes, and UI-010's fuller wording wrapped to three lines on
         a narrow canvas — the same complaint the toolbar move fixed. -->
    <span class="canvas-toolbar-stats" data-probe="stats-bar"><StatsBar /></span>
  </div>

  {#if !collapsed}
    <div class="toolbar-rows" data-probe="toolbar">
      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-zoom">Zoom</span>
        <!-- Glyphs, not "+ Zoom In" / "- Zoom Out": under a caption reading
             ZOOM the words were pure repetition, and they cost ~115px of a
             two-row budget that is ~125px wide at 1280. The accessible name
             is on aria-label, so nothing is lost to a screen reader. -->
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-zoom">
          <button class="control-btn icon-btn" title="Zoom in" aria-label="Zoom in"
            on:click={() => graphView?.zoomIn()}>+</button>
          <button class="control-btn icon-btn" title="Zoom out" aria-label="Zoom out"
            on:click={() => graphView?.zoomOut()}>−</button>
          <button class="control-btn" on:click={() => graphView?.resetZoom()}>Reset</button>
        </div>
      </div>

      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-fit">Fit</span>
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-fit">
          <button class="control-btn" on:click={() => graphView?.fitView()}>Fit View</button>
          <button class="control-btn" on:click={() => graphView?.fitWidth()}>Fit Width</button>
          <button
            class="control-btn"
            class:active={$autoFitView}
            aria-pressed={$autoFitView}
            on:click={() => autoFitView.update((v) => !v)}
            title="Automatically fit viewport after layout transitions"
          >Auto-Fit</button>
        </div>
      </div>

      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-show">Show</span>
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-show">
          <button class="control-btn" on:click={() => graphView?.toggleViewMode()}>
            {$viewMode === 'graph' ? 'Tree View' : 'Graph View'}
          </button>
          <!-- Only offered when there is a spec to draw. A button that opens a
               pane reading "no Elevator spec in this project" is a promise the
               project cannot keep, and every project without `.elv` files
               would carry it. -->
          {#if !$specGraph.empty}
            <button
              class="control-btn"
              class:active={$splitViewOpen}
              aria-pressed={$splitViewOpen}
              on:click={() => splitViewOpen.update((v) => !v)}
              title="Draw the Elevator spec beside the code, and filter the code by clicking it"
            >Spec Pane</button>
          {/if}
          <!-- Level toggle: aggregates the graph to one node per file or
               module. Drives both Graph and Tree views, which share data. -->
          <div class="level-toggle" role="group" aria-label="Aggregation level">
            {#each ['entity', 'file', 'module'] as lvl}
              <button
                class="control-btn level-btn"
                class:active={$graphLevel === lvl}
                on:click={() => { autoLevel.set(false); graphLevel.set(lvl as GraphLevel); }}
              >{lvl[0].toUpperCase() + lvl.slice(1)}</button>
            {/each}
          </div>
        </div>
      </div>


      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-selection">Selection</span>
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-selection">
          <button class="control-btn" on:click={() => selectedNode.set(null)}>Clear Selection</button>
          <!-- Offered only once something is marked. With nothing marked the
               button can only explain a gesture, and a permanently disabled
               control that says "⌘-click some nodes first" is a worse teacher
               than the ring that appears the moment you do it. -->
          {#if $markCount > 0}
            <button
              class="control-btn primary"
              data-probe="drill-marks"
              on:click={() => void drillIntoMarks()}
              title={$markedStats.fitsEntityLevel
                ? `Narrow the scope to the ${$markCount} marked, and re-open at the finest level that fits — ${$markedStats.entities} entities`
                : `Narrow the scope to the ${$markCount} marked. ${$markedStats.entities} entities is above the render budget, so it opens at file level; mark fewer, or drill again from there`}
            >Drill into {$markCount} marked ↓</button>
            <!-- The level the drill will land at, said before the click. The
                 whole point of marking two files is to see the entities
                 inside them, so a set too big to draw that way has to admit
                 it here rather than silently return another file view. -->
            {#if !$markedStats.fitsEntityLevel}
              <span class="mark-note">{$markedStats.entities} entities · opens at file level</span>
            {/if}
            <button class="control-btn" on:click={() => clearMarks()}>Clear Marks</button>
          {/if}
        </div>
      </div>

      <!-- Label toggles. `active` + `aria-pressed` mirror Auto-Fit above:
           without them these read identically on and off, and the only way to
           know the current setting is to click and watch the canvas. -->
      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-labels">Labels</span>
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-labels">
          <button
            class="control-btn"
            class:active={$showLabels}
            aria-pressed={$showLabels}
            on:click={() => ($showLabels = !$showLabels)}
          >Node Labels</button>
          <button
            class="control-btn"
            class:active={$showKindLabels}
            aria-pressed={$showKindLabels}
            on:click={() => ($showKindLabels = !$showKindLabels)}
          >Kind Labels</button>
          <button
            class="control-btn"
            class:active={$showLinkLabels}
            aria-pressed={$showLinkLabels}
            on:click={() => ($showLinkLabels = !$showLinkLabels)}
          >Link Labels</button>
        </div>
      </div>

      <!-- UI-054. The depth buttons only mean something in Links mode — in
           Folder mode membership has no distance — so the two clusters sit
           together and depth greys out rather than silently doing nothing. -->
      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-hover-mode">On hover</span>
        <div class="level-toggle" role="group" aria-labelledby="tb-hover-mode">
          {#each HOVER_MODES as mode}
            <button
              class="control-btn"
              class:active={$hoverMode === mode}
              aria-pressed={$hoverMode === mode}
              data-probe="hover-mode-{mode}"
              disabled={mode === 'group' && $graphLevel === 'module'}
              on:click={() => hoverMode.set(mode)}
              title={mode === 'group' && $graphLevel === 'module'
                ? 'No folder to highlight at Module level — each node is already one'
                : HOVER_MODE_TITLES[mode]}
            >{HOVER_MODE_LABELS[mode]}</button>
          {/each}
        </div>
      </div>

      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-highlight">Highlight depth</span>
        <div class="level-toggle" role="group" aria-labelledby="tb-highlight">
          {#each [1, 2, 3] as depth}
            <button
              class="control-btn level-btn"
              class:active={$hoverDepth === depth && $hoverMode === 'connections'}
              disabled={$hoverMode !== 'connections'}
              on:click={() => hoverDepth.set(depth)}
              title={$hoverMode === 'connections'
                ? `Highlight ${depth} degree${depth > 1 ? 's' : ''} of relationships on hover`
                : 'Only applies when hover highlights links'}
            >{depth}</button>
          {/each}
        </div>
      </div>

      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-tree-depth">Tree depth</span>
        <div class="level-toggle" role="group" aria-labelledby="tb-tree-depth">
          {#each [1, 2, 3] as depth}
            <button
              class="control-btn level-btn"
              class:active={$treeMaxDepth === depth}
              on:click={() => setTreeDepth(depth)}
              title="Show {depth} level{depth > 1 ? 's' : ''} of relationships"
            >{depth}</button>
          {/each}
        </div>
      </div>

      <div class="toolbar-group">
        <span class="toolbar-group-label" id="tb-spacing">Tree spacing</span>
        <div class="toolbar-cluster" role="group" aria-labelledby="tb-spacing">
          <button
            class="control-btn"
            on:click={cycleTreeDensity}
            title="Cycle tree compaction: Compact / Normal / Spacious"
          >{DENSITY_LABELS[$treeDensity]}</button>
        </div>
      </div>
    </div>
  {/if}
</header>

<style>
  .canvas-toolbar {
    display: flex;
    flex-direction: column;
    flex: none;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    /* Same inset ring the flanking columns use, for the same reason: the
       toolbar sits flush against the canvas below it. */
    /* Sized against the canvas column, not the window: since UI-040 the
       column can be 640px at a 1280px window, so a window-width media query
       would tighten at the wrong moments. */
    container-type: inline-size;
  }

  .canvas-toolbar.pane-focused {
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .canvas-toolbar-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .canvas-toolbar.collapsed .canvas-toolbar-bar { border-bottom: none; }

  .canvas-toolbar-stats {
    margin-left: auto;
    padding-right: 10px;
    font-size: 0.75rem;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .canvas-toolbar-handle {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    background: none;
    border: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    text-align: left;
    cursor: pointer;
  }
  .canvas-toolbar-handle:hover { color: var(--text); background: var(--bg-hover); }

  .toolbar-chev { font-size: 0.6rem; color: var(--text-dim); }

  .toolbar-summary {
    margin-left: 6px;
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    color: var(--text-dim);
  }

  .toolbar-rows {
    display: flex;
    flex-wrap: wrap;
    /* Clusters are the wrapping unit, so the gap between them has to read as
       wider than the gap between the buttons inside one. */
    gap: 8px 16px;
    padding: 6px 10px 8px;
    /* NOT `center` or `flex-end`: any alignment other than a shared top edge
       gives items on the same row different offsets, and the row count —
       here and in ux-probe — is measured from those offsets. */
    align-items: stretch;
  }

  .toolbar-group {
    display: flex;
    flex-direction: column;
    gap: 3px;
    /* Controls to the bottom, so every cluster's buttons sit on one baseline
       whatever its caption does. */
    justify-content: flex-end;
  }

  .toolbar-group-label {
    font-size: 0.64rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .toolbar-cluster {
    display: flex;
    gap: 6px;
    align-items: stretch;
  }

  .control-btn {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text);
    /* 12px, not the 16px this had on the canvas overlay. Eight clusters
       share ~880px of canvas at a 1280px window, and the ~90px this gives
       back is the margin that keeps the next control off a third row. */
    padding: 8px 12px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
    white-space: nowrap;
  }

  .icon-btn {
    padding: 8px 10px;
    min-width: 32px;
    font-size: 0.95rem;
    line-height: 1;
  }

  .control-btn:hover { background: var(--bg-hover); }
  /* A disabled control has to read as unavailable rather than as merely
     unselected, or the depth buttons look like a setting that stopped
     working (UI-054). `--text-disabled` is the one token deliberately below
     the contrast floor, which is correct here: WCAG exempts inactive
     controls, and this text carries no information the reader needs. */
  .control-btn:disabled {
    color: var(--text-disabled);
    border-color: var(--border-subtle);
    cursor: not-allowed;
  }
  .control-btn:disabled:hover { background: var(--bg-surface); }
  .control-btn.active {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
  }

  /* Filled like `.active` and meaning something else: `.active` is a toggle
     that is on, this is the one button in a cluster that *does* something
     rather than setting something. They share the accent because the marked
     rings on the canvas are drawn in it — the button is the end of that
     gesture, and the colour is what connects the two. */
  .control-btn.primary {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
  }

  .mark-note {
    align-self: center;
    font-size: 0.72rem;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .level-toggle {
    display: inline-flex;
    gap: 0;
    border-radius: 4px;
    overflow: hidden;
  }
  .level-toggle .level-btn {
    border-radius: 0;
    border-right-width: 0;
    padding: 8px 12px;
    font-size: 0.8rem;
  }
  .level-toggle .level-btn:first-child { border-top-left-radius: 4px; border-bottom-left-radius: 4px; }
  .level-toggle .level-btn:last-child  { border-top-right-radius: 4px; border-bottom-right-radius: 4px; border-right-width: 1px; }
  .level-toggle .level-btn.active {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
  }

  /* Eight clusters no longer clear two rows once the canvas column drops
     under ~800px, and a toolbar that grows downwards takes the height from
     the graph. Tightening the padding and the cluster gaps buys back the
     ~240px that keeps it at two rows; nothing is hidden, so no control
     becomes unreachable at a narrow width. */
  @container (max-width: 800px) {
    .toolbar-rows { gap: 6px 10px; padding: 5px 8px 7px; }
    .control-btn { padding: 6px 8px; font-size: 0.8rem; }
    .icon-btn { padding: 6px 7px; min-width: 26px; }
    .level-toggle .level-btn { padding: 6px 9px; font-size: 0.76rem; }
    .toolbar-group-label { font-size: 0.6rem; }
  }

  /* The 640px floor the canvas column is allowed to reach. One more step
     down keeps all eight clusters on two rows there. */
  @container (max-width: 700px) {
    .toolbar-rows { gap: 5px 8px; }
    .control-btn { padding: 5px 6px; font-size: 0.75rem; }
    .icon-btn { padding: 5px 6px; min-width: 22px; font-size: 0.85rem; }
    .level-toggle .level-btn { padding: 5px 7px; font-size: 0.72rem; }
    .toolbar-group-label { font-size: 0.57rem; letter-spacing: 0; }
  }
</style>
