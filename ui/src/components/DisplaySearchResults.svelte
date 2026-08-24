<script lang="ts">
  /**
   * "Search in view" — the Ctrl+F over what the canvas is currently drawing,
   * its result list, and the picks made in it.
   *
   * Its own component for the reason `EntitySearchResults.svelte` is:
   * `FilterPanel.svelte` is five times the ~300-line guideline and new query
   * UI belongs outside it.
   *
   * The list offers two gestures, and the distinction is the whole point of
   * the pane. Clicking a row *selects* the entity — a graph selection, which
   * narrows the canvas and therefore rewrites this very list, since the
   * search matches only what is drawn. Ticking a row *picks* it, which does
   * nothing but narrow the highlight, so the list holds still and a second
   * pick is possible. Before picking existed, clicking was the only gesture
   * and choosing one result was choosing away all the others.
   */
  import { NODE_COLORS } from '../types/graph';
  import type { D3Node } from '../types/graph';
  import { selectedNode } from '../stores/graph';
  import {
    displaySearchTerm,
    displaySearchMatches,
    displaySearchPicked,
    setDisplaySearchPicks,
    clearDisplaySearchPicks,
  } from '../viewmodels/displayPlan';
  import { rowsForClick } from '../utils/rowPicks';

  /** Cap the rendered rows so a broad term doesn't drop hundreds of DOM
   *  nodes into the sidebar. The canvas still marks every pick. */
  const MAX_RESULTS_SHOWN = 50;

  $: shown = $displaySearchMatches.slice(0, MAX_RESULTS_SHOWN);
  /** Picks that are still in view. The highlight is exactly this set, so it
   *  is the number the status row has to report — `$displaySearchPicked`
   *  can hold ids the canvas has since stopped drawing. */
  $: pickedInView = $displaySearchMatches.filter((n) => $displaySearchPicked.has(n.id)).length;

  let pickAnchor = -1;
  // An index into the previous result set addresses a different entity in
  // the new one, so a new term starts without an anchor.
  $: if ($displaySearchTerm !== undefined) pickAnchor = -1;

  function selectMatch(node: D3Node) {
    selectedNode.set(node);
  }

  /**
   * Pick a row, or a run of rows when shift is held.
   *
   * On `click` rather than `change`: `shiftKey` is not on the change event,
   * and by click time `currentTarget.checked` already holds the new state,
   * which is the state the whole range takes.
   */
  function onPickClick(e: MouseEvent & { currentTarget: HTMLInputElement }, i: number) {
    const checked = e.currentTarget.checked;
    const rows = rowsForClick(shown, pickAnchor, i, e.shiftKey);
    setDisplaySearchPicks(rows.map((n) => n.id), checked);
    pickAnchor = i;
  }
</script>

<!-- Display search: "Ctrl+F within the current view". Only highlights,
     never filters. Useful for locating a specific entity inside an
     already-scoped view without reshaping the graph. -->
<div class="sub-title">
  <span class="search-label search-label-view">Search in view</span>
  <span class="search-hint">— highlight only, no filtering</span>
</div>
<div class="filter-group">
  <input type="text" bind:value={$displaySearchTerm} placeholder="Search visible entities..." />
</div>

{#if $displaySearchTerm.trim()}
  <div class="search-results search-results-display">
    <div class="search-results-header">
      {#if $displaySearchMatches.length === 0}
        <span class="no-match">No matches in current view</span>
      {:else}
        <span class="match-count">
          {$displaySearchMatches.length} match{$displaySearchMatches.length === 1 ? '' : 'es'} in view
        </span>
        {#if $displaySearchPicked.size > 0}
          <!-- Reported against what is highlighted, not against what was
               ticked: a pick the canvas has stopped drawing is not marking
               anything, and saying "3 picked" while nothing glows would be
               the same lie the ticket was filed about. -->
          <span class="picked-count">
            {#if pickedInView > 0}
              highlighting {pickedInView} of them
            {:else}
              picks are no longer in view
            {/if}
          </span>
          <button type="button" class="link-btn" on:click={clearDisplaySearchPicks}>Clear picks</button>
        {/if}
        {#if $displaySearchMatches.length > MAX_RESULTS_SHOWN}
          <span class="truncated">(showing first {MAX_RESULTS_SHOWN})</span>
        {/if}
      {/if}
    </div>

    {#if $displaySearchMatches.length > 0}
      <!-- Neither gesture is discoverable from the row itself, and they do
           very different things — one moves the canvas, one does not. -->
      <div class="select-hint">
        Tick to highlight just those{shown.length > 1 ? '; shift-click for a range' : ''}. Click a
        row to select it on the canvas.
      </div>
      <ul class="search-results-list" data-probe="display-search-results">
        {#each shown as match, i (match.id)}
          <li
            class="search-result-item"
            class:active={$selectedNode?.id === match.id}
            class:picked={$displaySearchPicked.has(match.id)}
            title="{match.qualified_name} — {match.file_path}:{match.line}"
          >
            <input type="checkbox"
              class="result-pick-cb"
              checked={$displaySearchPicked.has(match.id)}
              on:click|stopPropagation={(e) => onPickClick(e, i)}
              title="Highlight only the ticked matches (shift-click for a range)" />
            <button type="button" class="row-btn" on:click={() => selectMatch(match)}>
              <span class="result-kind"><i class="kind-dot"
                  style="background: {NODE_COLORS[match.kind_raw] || 'var(--text-muted)'}"></i>
                {match.kind}
              </span>
              <span class="result-name">{match.name}</span>
              <span class="result-path">{match.file_path}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  /* Lifted from FilterPanel.svelte with the markup, unchanged apart from
     the two rules picking needed. */
  .sub-title {
    font-size: 0.8rem;
    color: var(--text-muted);
    margin: 10px 0 6px;
    padding-bottom: 3px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
  }
  .search-label { font-weight: 600; }
  .search-label-view { color: var(--text-secondary); }
  .search-hint { color: var(--text-disabled); font-weight: 400; font-size: 0.7rem; margin-left: 4px; }

  .filter-group { margin-bottom: 10px; }
  .filter-group input[type="text"] {
    width: 100%;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-body);
    color: var(--text);
  }

  .search-results {
    margin: 8px 0 12px;
    border: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
    border-radius: 4px;
    background: color-mix(in srgb, var(--bg-body) 50%, transparent);
  }
  .search-results-display {
    border-color: rgba(77, 208, 225, 0.3);
    background: rgba(77, 208, 225, 0.04);
  }

  .search-results-header {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 10px;
    font-size: 0.75rem;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
  }
  /* The display search's own colour, the one the canvas outlines matches
     with — the dataset search's status row is amber for the same reason. */
  .match-count { color: #4DD0E1; font-weight: 600; }
  .picked-count { color: var(--text-secondary); }
  .no-match { color: var(--text-dim); font-style: italic; }
  .truncated { color: var(--text-disabled); font-size: 0.7rem; }
  .link-btn {
    background: none; border: none; padding: 0; cursor: pointer;
    color: var(--accent); font-size: 0.7rem; text-decoration: underline;
  }
  .select-hint {
    font-size: 0.7rem; color: var(--text-disabled);
    padding: 4px 10px 0;
  }

  .search-results-list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
  }

  .search-result-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    font-size: 0.8rem;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 20%, transparent);
  }
  .search-result-item:last-child { border-bottom: none; }
  .search-result-item:hover { background: color-mix(in srgb, var(--bg-hover) 40%, transparent); }
  .search-result-item.active { background: color-mix(in srgb, var(--accent) 25%, transparent); }
  /* A picked row wears the highlight colour the canvas gives it, so the
     two pictures can be read against each other. */
  .search-result-item.picked { box-shadow: inset 2px 0 0 #4DD0E1; }

  .result-pick-cb { width: 13px; flex: none; margin: 0; cursor: pointer; }
  .row-btn {
    display: flex; align-items: center; gap: 6px;
    flex: 1; min-width: 0;
    background: none; border: none; padding: 0; cursor: pointer;
    text-align: left; font: inherit; color: inherit;
  }

  /* Palette swatch beside themed text — see ColorChip for the reasoning. */
  .kind-dot {
    display: inline-block;
    width: 7px; height: 7px;
    border-radius: 50%;
    margin-right: 4px;
    box-shadow: 0 0 0 1px var(--border-subtle);
  }
  .result-kind {
    color: var(--text-muted);
    font-size: 0.65rem;
    text-transform: uppercase;
    flex-shrink: 0;
    min-width: 56px;
  }
  .result-name {
    color: var(--text);
    flex-shrink: 0;
    font-family: 'Monaco', 'Menlo', monospace;
  }
  .result-path {
    color: var(--text-disabled);
    font-size: 0.7rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    text-align: right;
  }
</style>
