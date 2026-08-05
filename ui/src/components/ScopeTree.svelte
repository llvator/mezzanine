<script lang="ts">
  import {
    indexData, selectedScopes, scopeRules, selectionStats, refreshing,
    treeLanguageFilter, availableLanguages,
    toggleScope, clearScope, selectAllScope, extendScopeToParents, refreshData,
    filterText, openFolders, flatList,
    queryActive, queryMatches, matchOverflow, queryProjection, commitQuery,
    toggleFolderOpen, toggleTreeLanguage, clearLanguageFilter,
    displayName, formatCount, isInScope, isDirectRule,
  } from '../viewmodels/scopeTreeViewModel';

  /** Enter applies the query as the scope, Shift+Enter adds to what's
   *  already selected, Escape abandons it. The same contract the entity
   *  search in FilterPanel uses, pointed at the scope instead of the view. */
  function onQueryKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitQuery(e.shiftKey);
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      filterText.set('');
    }
  }
</script>

<div class="scope-tree-wrap">
  <div class="toolbar">
    <input
      type="text"
      class="search"
      data-probe="scope-query"
      bind:value={$filterText}
      on:keydown={onQueryKeydown}
      placeholder="Filter paths…  (Enter to scope)" />
    <button
      type="button"
      class="refresh-btn"
      title="Extend selection to parent folder"
      disabled={$selectedScopes.size === 0}
      on:click={extendScopeToParents}
    >⬆</button>
    <button
      type="button"
      class="refresh-btn"
      title="Reload data from disk (use after re-running nao analyze)"
      disabled={$refreshing}
      on:click={refreshData}
    >{$refreshing ? '⟳…' : '⟳'}</button>
  </div>

  {#if $availableLanguages.length > 0}
    <div class="lang-filter">
      <span class="lang-filter-label">Languages:</span>
      {#each $availableLanguages as lang}
        <button
          type="button"
          class="lang-chip"
          class:active={$treeLanguageFilter.has(lang)}
          on:click={() => toggleTreeLanguage(lang)}
        >{lang}</button>
      {/each}
      {#if $treeLanguageFilter.size > 0}
        <button type="button" class="lang-clear" on:click={clearLanguageFilter}>×</button>
      {/if}
    </div>
  {/if}

  {#if $indexData}
    <div class="tree-stats" data-probe="scope-tree">
      <div class="totals-row">
        <span title="Everything nao has indexed across the whole repository.">Indexed:
          {$indexData.total_entities.toLocaleString()} entities,
          {$indexData.total_relationships.toLocaleString()} rels</span>
        <button class="all-btn" on:click={selectAllScope}>Select All</button>
      </div>
      {#if $selectedScopes.size > 0}
        <div class="sel-row">
          <span class="sel-stats" title="Entities inside the folders and files checked below. The canvas may draw fewer — see the count on the graph.">
            In scope: <strong>{$selectionStats.entities.toLocaleString()}</strong> entities,
            <strong>{$selectionStats.relationships.toLocaleString()}</strong> rels
          </span>
          <button class="clear-btn" on:click={clearScope}>Clear</button>
        </div>
      {/if}
    </div>
    {#if $queryActive}
      <div class="match-status">
        {#if $queryMatches.length === 0}
          <span class="no-match">No matches</span>
        {:else}
          <span class="match-count">
            {$queryMatches.length} match{$queryMatches.length === 1 ? '' : 'es'}
          </span>
          {#if $matchOverflow > 0}
            <span class="truncated">showing first {$flatList.length}</span>
          {/if}
          <!-- The cost of pressing Enter, before pressing it — a number, not
               a verdict. It used to turn red past `ENTITY_THRESHOLD` to warn
               that the canvas would refuse; since UI-061 the canvas decides
               from what it would draw, after collapse and filters, so a large
               selection here usually renders fine and the red was a wrong
               prediction rather than an early one. -->
          {#if $queryProjection}
            <span class="projection" title="Pressing Enter scopes to these matches">
              ⏎ {formatCount($queryProjection.entities)} entities
            </span>
          {/if}
        {/if}
      </div>
    {/if}
    <div class="scope-tree">
      {#each $flatList as item (item.path)}
        <!-- An entity's declaring file is normally in the index, but a file
             whose only entities are excluded kinds (parameters, branches)
             isn't — so the row must survive `node` being absent. -->
        {@const node = $indexData.nodes[item.path]}
        <!-- Direct = a rule names this exact path; inherited = it is in
             scope because some other rule covers it. Both come from
             evaluating the rule list, not from walking ancestors. -->
        {@const directlySelected = isDirectRule(item.path, $scopeRules)}
        {@const inheritedSelected = !directlySelected && isInScope(item.path, $scopeRules)}
        <div
          class="tree-item"
          class:selected={directlySelected}
          class:inherited={inheritedSelected}
          style="padding-left: {item.depth * 12}px"
        >
          <!-- No expander while querying: results are a flat list, so an
               arrow would promise a drill-down the list doesn't have. -->
          {#if item.isFolder && !$queryActive}
            <span class="toggle" on:click={() => toggleFolderOpen(item.path)}>
              {$openFolders.has(item.path) ? '▼' : '▶'}
            </span>
          {:else}
            <span class="toggle-spacer"></span>
          {/if}
          <input
            type="checkbox"
            class="select-cb"
            checked={directlySelected || inheritedSelected}
            title={inheritedSelected ? 'Selected via parent folder — click to exclude' : ''}
            on:change={() => toggleScope(item.path)}
          />
          {#if item.entityName}
            <span class="icon entity" title="Matched an entity name">◆</span>
          {:else if item.isFolder}
            <span class="icon folder">📁</span>
          {:else}
            <span class="icon file">📄</span>
          {/if}
          <!-- Query results show the whole path: two `mod.rs` rows from
               different folders are otherwise the same row twice. An entity
               hit leads with the name that matched and keeps the path, since
               the path is what selecting the row would scope to. -->
          <span class="label" class:full-path={$queryActive} title={item.path}>
            {#if item.entityName}
              <span class="entity-name">{item.entityName}</span>
              <span class="entity-path">{item.path}</span>
            {:else}
              {$queryActive ? item.path : (displayName(item.path) || item.path)}
            {/if}
          </span>
          <!-- Count only. A ⚠ used to appear past `ENTITY_THRESHOLD`, meaning
               "this won't render" — which discouraged selecting exactly the
               folders worth looking at, before anything had refused. Since
               UI-061 a large folder is drawn collapsed rather than refused,
               so the glyph asserted something untrue. The number is the
               honest part and it stayed. -->
          <span class="count" title="{node?.entity_count ?? 0} entities, {node?.relationship_count ?? 0} relationships">
            {formatCount(node?.entity_count ?? 0)}
          </span>
        </div>
      {/each}
    </div>
  {:else}
    <div class="loading">Loading index…</div>
  {/if}
</div>

<style>
  /* Applied imperatively by the empty-state card's "Scope panel" link, so
     Svelte can't see it referenced and would prune it without :global. */
  :global(.scope-tree-flash) {
    animation: scope-tree-flash 1.6s ease-out;
    border-radius: 4px;
  }

  @keyframes scope-tree-flash {
    0%, 100% { box-shadow: 0 0 0 0 transparent; }
    15%      { box-shadow: 0 0 0 3px var(--accent); }
    70%      { box-shadow: 0 0 0 3px var(--accent); }
  }

  .scope-tree-wrap {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .toolbar {
    display: flex;
    gap: 4px;
  }

  .search {
    flex: 1;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-body);
    color: var(--text);
    font-size: 0.8rem;
  }

  .refresh-btn {
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text);
    font-size: 0.9rem;
    cursor: pointer;
    flex-shrink: 0;
  }

  .refresh-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
  }

  .refresh-btn:disabled {
    opacity: 0.5;
    cursor: wait;
  }

  .tree-stats {
    font-size: 0.7rem;
    color: var(--text-dim);
    padding: 2px 4px;
  }

  .scope-tree {
    font-size: 0.8rem;
    max-height: 400px;
    overflow-y: auto;
  }

  .tree-item {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px;
    border-radius: 3px;
  }

  .tree-item:hover {
    background: color-mix(in srgb, var(--bg-hover) 30%, transparent);
  }

  .tree-item.selected {
    background: rgba(33, 150, 243, 0.25);
  }

  .tree-item.inherited {
    background: rgba(33, 150, 243, 0.08);
  }

  .select-cb {
    flex-shrink: 0;
    cursor: pointer;
    margin: 0;
  }

  .select-cb:disabled {
    cursor: not-allowed;
    opacity: 0.7;
  }

  .sel-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
    color: var(--text-muted);
  }

  .sel-stats strong {
    color: var(--text);
  }

  .clear-btn {
    margin-left: auto;
    background: var(--bg-hover);
    color: var(--text-secondary);
    border: 1px solid color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
    border-radius: 3px;
    padding: 2px 8px;
    font-size: 0.7rem;
    cursor: pointer;
  }

  .clear-btn:hover {
    background: color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
  }

  .totals-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .all-btn {
    margin-left: auto;
    background: var(--bg-hover);
    color: var(--text-secondary);
    border: 1px solid color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
    border-radius: 3px;
    padding: 2px 8px;
    font-size: 0.7rem;
    cursor: pointer;
  }

  .all-btn:hover {
    background: color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
  }

  .lang-filter {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
  }

  .lang-filter-label {
    font-size: 0.7rem;
    color: var(--text-dim);
    margin-right: 2px;
  }

  .lang-chip {
    background: var(--bg-body);
    color: var(--text-muted);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 2px 8px;
    font-size: 0.7rem;
    cursor: pointer;
  }

  .lang-chip:hover {
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .lang-chip.active {
    background: #2196F3;
    color: #fff;
    border-color: #2196F3;
  }

  .lang-clear {
    background: transparent;
    color: var(--text-dim);
    border: none;
    font-size: 0.9rem;
    cursor: pointer;
    padding: 0 4px;
    line-height: 1;
  }

  .lang-clear:hover {
    color: var(--danger-fg);
  }

  .toggle {
    cursor: pointer;
    font-size: 0.6rem;
    width: 12px;
    text-align: center;
    color: var(--text-dim);
    flex-shrink: 0;
  }

  .toggle-spacer {
    width: 12px;
    flex-shrink: 0;
  }

  .icon {
    flex-shrink: 0;
    font-size: 0.75rem;
    width: 14px;
    text-align: center;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
    flex: 1;
  }

  .label:hover {
    color: #8cb4ff;
  }

  .count {
    font-size: 0.65rem;
    color: var(--text-dim);
    flex-shrink: 0;
    white-space: nowrap;
  }

  .loading {
    color: var(--text-dim);
    font-size: 0.8rem;
    padding: 10px;
  }

  .match-status {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 0.7rem;
    color: var(--text-dim);
    padding: 2px 4px;
  }

  .match-count {
    color: var(--text-secondary);
  }

  .projection {
    margin-left: auto;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  /* Full paths are long; the folder prefix is what disambiguates two
     same-named files, so it's the tail that gets elided. */
  .label.full-path {
    direction: ltr;
    font-size: 0.75rem;
  }

  .icon.entity {
    color: var(--accent);
    font-size: 0.6rem;
  }

  .entity-name {
    color: var(--text);
  }

  .entity-path {
    color: var(--text-dim);
    margin-left: 5px;
  }
</style>
