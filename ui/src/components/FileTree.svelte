<script lang="ts">
  import FileTreeNode from './FileTreeNode.svelte';
  import {
    filterText, tree, allSelected, matchedFiles, allFiles, queryActive,
    toggleAll,
  } from '../viewmodels/fileTreeViewModel';
  import type { TreeNode } from '../viewmodels/fileTreeViewModel';

  /** Folders first, then files, each alphabetical. */
  function sortedKeys(t: TreeNode): string[] {
    return Object.keys(t).sort((a, b) => {
      const aFile = t[a] === null;
      const bFile = t[b] === null;
      if (aFile !== bFile) return aFile ? 1 : -1;
      return a.localeCompare(b);
    });
  }
</script>

<div class="file-filter-toolbar">
  <input
    type="text"
    class="file-filter-search"
    data-probe="file-filter"
    bind:value={$filterText}
    placeholder="Filter paths…" />
  <button
    class="toggle-all-btn"
    on:click={toggleAll}
    title={$queryActive
      ? `${$allSelected ? 'Hide' : 'Show'} the ${$matchedFiles.length} matching file${$matchedFiles.length === 1 ? '' : 's'}`
      : `${$allSelected ? 'Hide' : 'Show'} every file`}>
    {$allSelected ? 'None' : 'All'}
  </button>
</div>

{#if $queryActive}
  <div class="filter-status">
    {$matchedFiles.length} of {$allFiles.length} files
  </div>
{/if}

<div class="file-tree" data-probe="file-tree">
  {#each sortedKeys($tree) as key (key)}
    <FileTreeNode name={key} path={key} node={$tree[key]} />
  {/each}
</div>

<style>
  .file-filter-toolbar {
    display: flex;
    gap: 4px;
    margin-bottom: 6px;
  }

  .file-filter-search {
    flex: 1;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-body);
    color: var(--text);
    font-size: 0.8rem;
  }

  .toggle-all-btn {
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text);
    font-size: 0.75rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .toggle-all-btn:hover {
    background: color-mix(in srgb, var(--bg-hover) 70%, var(--text) 30%);
  }

  .filter-status {
    font-size: 0.7rem;
    color: var(--text-dim);
    padding: 0 4px 4px;
  }

  .file-tree {
    font-size: 0.8rem;
    max-height: 300px;
    overflow-y: auto;
  }
</style>
