<script lang="ts">
  /**
   * The entity search's mode control, status row and ranked result list.
   *
   * Extracted from `FilterPanel.svelte` rather than added to it: that file
   * was already at 1577 lines against the ~300 guideline, and the rule in
   * `context-for-issues-of-fuzzy-scope.md` is that new query UI gets its own
   * component. The search *input* stays at the top of the panel; this is
   * everything that hangs off it.
   */
  import { NODE_COLORS } from '../types/graph';
  import type { D3Node } from '../types/graph';
  import { selectedNode, graphData } from '../stores/graph';
  import { get } from 'svelte/store';
  import {
    searchTerm,
    committedSearchIds,
    commitAllMatches,
    clearCommittedMatches,
    setCommittedMatches,
    searchHidesNonMatches,
  } from '../viewmodels/filterViewModel';
  import {
    searchResults,
    visibleSearchResults,
    searchResultCounts,
    relaxedFilters,
    unblock,
    unblockAll,
    undoRelaxations,
    scopeToResults,
    MAX_RESULTS_SHOWN,
    type SearchResult,
  } from '../viewmodels/searchResults';

  /** Rows that carry a commit checkbox — the in-scope ones. An out-of-scope
   *  hit has no id in `graphData`, so committing it would filter the graph
   *  on something the graph does not hold. */
  $: selectable = $visibleSearchResults.filter((r) => r.inScope);
  $: outOfScope = $searchResults.filter((r) => !r.inScope);

  let commitAnchor = -1;
  // An index into the previous result set addresses a different entity in
  // the new one, so a new query starts without an anchor.
  $: if ($searchTerm !== undefined) commitAnchor = -1;

  function selectRow(r: SearchResult) {
    if (r.inScope) selectedNode.set(r.node);
  }

  /**
   * Commit a row, or a run of rows when shift is held.
   *
   * On `click` rather than `change`: `shiftKey` is not on the change event,
   * and by click time `currentTarget.checked` already holds the new state,
   * which is the state the whole range takes.
   *
   * Committing a row that is blocked also unblocks it. Filtering the graph
   * down to something no filter will draw is the failure this whole list
   * exists to make visible — doing it deliberately, one checkbox at a time,
   * would be no better than doing it by accident.
   */
  function onCommitClick(e: MouseEvent & { currentTarget: HTMLInputElement }, i: number) {
    const checked = e.currentTarget.checked;
    const rows =
      e.shiftKey && commitAnchor >= 0 && commitAnchor < selectable.length
        ? selectable.slice(Math.min(commitAnchor, i), Math.max(commitAnchor, i) + 1)
        : [selectable[i]];
    setCommittedMatches(rows.map((r) => r.node.id), checked);
    if (checked) {
      for (const r of rows) if (r.blocked?.reversible) unblock(r.blocked);
    }
    commitAnchor = i;
  }

  /** Bring every out-of-scope hit into the analysis scope in one commit,
   *  then select the best-ranked one so the view lands somewhere useful. */
  async function scopeToAll() {
    const best = outOfScope[0]?.node;
    await scopeToResults(outOfScope.map((r) => r.node));
    if (best) reselect(best);
  }

  async function scopeToOne(node: D3Node) {
    await scopeToResults([node]);
    reselect(node);
  }

  /** After a scope change the node is a *different object* in the freshly
   *  loaded graph, so re-find it rather than selecting the stale one. */
  function reselect(node: D3Node) {
    const data = get(graphData);
    const match =
      data.nodes.find((n) => n.id === node.id) ??
      data.nodes.find((n) => n.original_id === node.original_id);
    if (match) selectedNode.set(match);
  }
</script>

<!-- Mode. Visible before a commit, because the choice is what committing
     will DO and belongs in front of that decision — but the canvas only
     reads it while something is committed (`searchMatched.size > 0` in
     displayPlan), so with an empty commit set clicking these changes
     nothing. Saying so is the difference between a control that is waiting
     and a control that is broken. -->
<div class="mode-row">
  <span class="mode-label">Mode</span>
  <div class="mode-toggle" role="group" aria-label="Search mode">
    <button
      type="button"
      class="mode-btn"
      class:active={!$searchHidesNonMatches}
      aria-pressed={!$searchHidesNonMatches}
      on:click={() => searchHidesNonMatches.set(false)}
      title="Keep the whole graph, fade everything that isn't a match"
    >Highlight</button>
    <button
      type="button"
      class="mode-btn"
      class:active={$searchHidesNonMatches}
      aria-pressed={$searchHidesNonMatches}
      on:click={() => searchHidesNonMatches.set(true)}
      title="Draw only the matches and their immediate neighbours"
    >Filter</button>
  </div>
  {#if $committedSearchIds.size === 0}
    <span class="mode-pending" title="Tick a result, or press Commit all, and the canvas takes this mode">
      on commit
    </span>
  {/if}
</div>

{#if $committedSearchIds.size > 0}
  <div class="commit-status">
    <span class="commit-count">
      {$searchHidesNonMatches ? 'Filtering on' : 'Highlighting'}
      {$committedSearchIds.size} entit{$committedSearchIds.size === 1 ? 'y' : 'ies'}
    </span>
    <button type="button" class="link-btn" on:click={clearCommittedMatches}>Clear</button>
  </div>
{/if}

<!-- What this search widened, and the way back. A search that silently
     relaxed a filter the user set deliberately would leave them wondering
     why unrelated nodes reappeared. -->
{#if $relaxedFilters.length > 0}
  <div class="relaxed-row">
    <span class="relaxed-text">
      Filters relaxed: {$relaxedFilters.map((r) => r.label).join(', ')}
    </span>
    <button type="button" class="link-btn" on:click={undoRelaxations}>Undo</button>
  </div>
{/if}

{#if $searchTerm.trim()}
  <div class="search-results">
    <div class="search-results-header">
      {#if $searchResultCounts.total === 0}
        <span class="no-match">No matches</span>
      {:else}
        <span class="match-count">
          {$searchResultCounts.total} match{$searchResultCounts.total === 1 ? '' : 'es'}
        </span>
        {#if $searchResultCounts.hidden > 0}
          <span class="hidden-count" title="Matches that exist but are not on the canvas">
            {$searchResultCounts.hidden} not shown
          </span>
        {/if}
        {#if $searchResultCounts.total > MAX_RESULTS_SHOWN}
          <span class="truncated">(top {MAX_RESULTS_SHOWN})</span>
        {/if}
        <button type="button" class="link-btn" on:click={commitAllMatches}
          title="Commit every current match (same as Enter)">Commit all</button>
      {/if}
    </div>

    <!-- Selection is per-row and ranged, and neither is discoverable from a
         bare checkbox column. When nothing is selectable the column is all
         spacers, which looks like the feature is missing rather than like
         the scope holding none of these entities — so say which it is. -->
    {#if $searchResultCounts.total > 0}
      <div class="select-hint">
        {#if selectable.length === 0}
          Nothing here is loaded — bring a file into scope before selecting.
        {:else if selectable.length === 1}
          1 result can be selected.
        {:else}
          Tick to select; shift-click for a range ({selectable.length} selectable).
        {/if}
      </div>
    {/if}

    {#if $searchResultCounts.blocked > 0 || $searchResultCounts.outOfScope > 0}
      <div class="bulk-row">
        {#if $searchResultCounts.blocked > 0}
          <button type="button" class="bulk-btn" on:click={unblockAll}
            title="Re-admit every filter hiding a match">
            Show {$searchResultCounts.blocked} hidden
          </button>
        {/if}
        {#if $searchResultCounts.outOfScope > 0}
          <button type="button" class="bulk-btn" on:click={scopeToAll}
            title="Add every out-of-scope match's file to the analysis scope">
            Scope to {$searchResultCounts.outOfScope} more
          </button>
        {/if}
      </div>
    {/if}

    {#if $visibleSearchResults.length > 0}
      <ul class="search-results-list" data-probe="search-results">
        {#each $visibleSearchResults as r (r.node.id)}
          {@const i = selectable.indexOf(r)}
          <li
            class="search-result-item"
            class:active={$selectedNode?.id === r.node.id}
            class:committed={$committedSearchIds.has(r.node.id)}
            class:muted={!r.inScope || r.blocked !== null}
            title="{r.node.qualified_name} — {r.node.file_path}:{r.node.line}"
          >
            {#if r.inScope}
              <input type="checkbox"
                class="result-commit-cb"
                checked={$committedSearchIds.has(r.node.id)}
                on:click|stopPropagation={(e) => onCommitClick(e, i)}
                title="Commit this match (shift-click for a range)" />
            {:else}
              <span class="cb-spacer" aria-hidden="true"></span>
            {/if}
            <!-- Two lines, because one does not fit. A sidebar row has room
                 for a name or a path, not both plus a badge and an action —
                 packed onto one line every column truncated and the name,
                 the thing being searched for, lost characters first. -->
            <div class="row-body">
              <button type="button" class="row-btn" on:click={() => selectRow(r)}>
                <span class="row-top">
                  <i class="kind-dot" style="background: {NODE_COLORS[r.node.kind_raw] || 'var(--text-muted)'}"
                    title={r.node.kind}></i>
                  <span class="result-name">{r.node.name}</span>
                </span>
                <span class="result-path">{r.node.file_path}</span>
              </button>
              {#if !r.inScope}
                <span class="badge">out of scope</span>
                <button type="button" class="row-action" on:click={() => scopeToOne(r.node)}
                  title="Add this file to the analysis scope">Scope</button>
              {:else if r.blocked}
                <span class="badge">{r.blocked.label}</span>
                {#if r.blocked.reversible}
                  <button type="button" class="row-action" on:click={() => unblock(r.blocked!)}
                    title="Re-admit the filter hiding this entity">Show</button>
                {/if}
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  .mode-row { display: flex; align-items: center; gap: 8px; margin: 6px 0; }
  .mode-label { font-size: 11px; color: var(--text-muted); }
  .mode-pending { font-size: 10px; color: var(--text-dim); font-style: italic; }
  .mode-toggle { display: inline-flex; border: 1px solid var(--border); border-radius: 4px; overflow: hidden; }
  .mode-btn {
    background: var(--bg-surface); color: var(--text-secondary);
    border: none; padding: 3px 10px; font-size: 11px; cursor: pointer;
  }
  .mode-btn:hover { background: var(--bg-hover); color: var(--text); }
  .mode-btn.active { background: var(--accent); color: var(--accent-fg); }

  .commit-status, .relaxed-row, .bulk-row {
    display: flex; align-items: center; gap: 8px;
    padding: 4px 0; font-size: 11px; color: var(--text-secondary);
  }
  .relaxed-text { flex: 1; color: var(--text-muted); }
  .link-btn {
    background: none; border: none; padding: 0; cursor: pointer;
    color: var(--accent); font-size: 11px; text-decoration: underline;
  }
  .bulk-btn {
    background: var(--bg-surface); border: 1px solid var(--border); border-radius: 3px;
    color: var(--text-secondary); font-size: 11px; padding: 2px 8px; cursor: pointer;
  }
  .bulk-btn:hover { background: var(--bg-hover); color: var(--text); }

  .search-results-header {
    display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
    padding: 4px 0; font-size: 11px;
  }
  .match-count { color: var(--accent); }
  .hidden-count, .truncated, .no-match { color: var(--text-muted); }
  .select-hint { font-size: 10px; color: var(--text-dim); padding: 0 0 3px; }

  .search-results-list { list-style: none; margin: 0; padding: 0; }
  .search-result-item {
    display: flex; align-items: flex-start; gap: 6px;
    padding: 3px 4px; border-radius: 3px; font-size: 11px;
  }
  .row-body {
    display: flex; align-items: center; gap: 6px;
    flex: 1; min-width: 0;
  }
  .search-result-item:hover { background: var(--bg-hover); }
  .search-result-item.active { background: var(--bg-surface-alt); }
  .search-result-item.committed { border-left: 2px solid var(--accent); }
  /* Muted, not disabled: `--text-disabled` is the one documented contrast
     exemption and these rows still have to be read. */
  .search-result-item.muted .result-name,
  .search-result-item.muted .result-path { color: var(--text-muted); }

  /* Both the box and its placeholder are fixed-width and top-aligned to the
     first line of a two-line row, so the checkbox column stays a column —
     without `flex: none` the box is shrinkable, and the row grew a second
     line after this rule was first written. */
  .result-commit-cb {
    width: 13px; flex: none; margin: 2px 0 0; cursor: pointer;
  }
  .cb-spacer { width: 13px; flex: none; }
  .row-btn {
    display: flex; flex-direction: column; gap: 1px; flex: 1; min-width: 0;
    background: none; border: none; padding: 0; cursor: pointer;
    text-align: left; font-size: 11px; color: inherit;
  }
  .row-top { display: flex; align-items: center; gap: 5px; min-width: 0; }
  .kind-dot { width: 7px; height: 7px; border-radius: 50%; display: inline-block; flex: none; }
  .result-name {
    color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  /* `direction: rtl` keeps the ellipsis at the *front* of a long path, so
     the filename — the part that identifies it — survives the truncation. */
  .result-path {
    color: var(--text-dim); font-size: 10px;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    min-width: 0; direction: rtl; text-align: left;
  }
  .badge {
    flex: none; font-size: 10px; padding: 0 4px; border-radius: 3px;
    background: var(--bg-surface-alt); color: var(--text-muted); white-space: nowrap;
  }
  .row-action {
    flex: none; background: none; border: 1px solid var(--border); border-radius: 3px;
    color: var(--accent); font-size: 10px; padding: 0 5px; cursor: pointer;
  }
  .row-action:hover { background: var(--bg-hover); }
</style>
