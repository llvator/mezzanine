<script lang="ts">
  /**
   * One row of the Files tree (Graph Visualization block), recursing into
   * itself for folders.
   *
   * Split out of `FileTree.svelte` because that component unrolled the tree
   * by hand: top-level entries, then one level of children, files only,
   * with a `<!-- Recursive rendering would go here -->` comment where the
   * recursion belonged (UI-046). Anything three or more directories deep —
   * which in this repo is nearly everything — could not be reached or
   * toggled at all.
   */
  import { hiddenFiles } from '../stores/graph';
  import {
    visibleFiles, openFolders, entityCounts, queryActive,
    toggleFile, toggleFolder, toggleFolderOpen, folderState,
  } from '../viewmodels/fileTreeViewModel';
  import type { TreeNode } from '../viewmodels/fileTreeViewModel';

  export let name: string;
  export let path: string;
  /** `null` for a file; the subtree for a folder. */
  export let node: TreeNode | null;
  export let depth = 0;

  // A query already pruned the tree to matching files, so everything left is
  // worth showing — forcing folders open saves the user re-expanding their
  // way to a result they just asked for.
  $: open = $queryActive || $openFolders.has(path);
  $: state = node ? folderState(node, path, $hiddenFiles) : null;

  /** Folders first, then files, each alphabetical — the same order the
   *  server sorts index children into, so the two trees agree. */
  function sortedKeys(t: TreeNode): string[] {
    return Object.keys(t).sort((a, b) => {
      const aFile = t[a] === null;
      const bFile = t[b] === null;
      if (aFile !== bFile) return aFile ? 1 : -1;
      return a.localeCompare(b);
    });
  }
</script>

<div class="tree-item" class:file-item={!node} style="padding-left: {depth * 12}px">
  {#if node}
    <button
      type="button"
      class="ft-toggle"
      aria-expanded={open}
      aria-label={open ? `Collapse ${name}` : `Expand ${name}`}
      disabled={$queryActive}
      on:click={() => toggleFolderOpen(path)}
    >{open ? '▼' : '▶'}</button>
    <input
      type="checkbox"
      checked={state === 'all'}
      indeterminate={state === 'some'}
      title="{state === 'all' ? 'All' : state === 'some' ? 'Some' : 'No'} files in this folder are shown"
      on:change={(e) => toggleFolder(node, path, e.currentTarget.checked)} />
    <span class="ft-icon folder">&#128193;</span>
  {:else}
    <span class="ft-toggle-spacer"></span>
    <input
      type="checkbox"
      checked={$visibleFiles.has(path)}
      on:change={(e) => toggleFile(path, e.currentTarget.checked)} />
    <span class="ft-icon file">&#128196;</span>
  {/if}
  <span class="ft-label" title={path}>{name}</span>
  {#if !node}
    <span class="ft-count">{$entityCounts.get(path) ?? 0}</span>
  {/if}
</div>

{#if node && open}
  {#each sortedKeys(node) as childKey (childKey)}
    <svelte:self
      name={childKey}
      path={path ? path + '/' + childKey : childKey}
      node={node[childKey]}
      depth={depth + 1} />
  {/each}
{/if}

<style>
  .tree-item {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
  }

  .tree-item:hover { background: color-mix(in srgb, var(--bg-hover) 30%, transparent); }
  .tree-item input[type="checkbox"] { cursor: pointer; flex-shrink: 0; }

  /* Emoji glyphs carry their own colour, so no `color` here: the old rules
     pinned literals (#FF9800 / #64B5F6) that the emoji ignored anyway. */
  .ft-icon { flex-shrink: 0; font-size: 0.75rem; width: 14px; text-align: center; }
  .ft-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .ft-toggle {
    cursor: pointer;
    font-size: 0.6rem;
    width: 10px;
    padding: 0;
    border: none;
    background: none;
    text-align: center;
    flex-shrink: 0;
    color: var(--text-dim);
  }

  .ft-toggle:disabled { cursor: default; opacity: 0.4; }
  .ft-toggle-spacer { width: 10px; flex-shrink: 0; }

  .ft-count {
    font-size: 0.65rem;
    color: var(--text-dim);
    margin-left: auto;
    flex-shrink: 0;
    padding-left: 6px;
  }
</style>
