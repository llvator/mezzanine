<script lang="ts">
  import {
    rootPath, rootPathLoading, rootPathError,
    fetchRootPath, setRootPath,
  } from '../stores/scope';
  import { onMount } from 'svelte';

  let editMode = false;
  let inputPath = '';

  onMount(() => {
    fetchRootPath();
  });

  function startEdit() {
    inputPath = $rootPath;
    editMode = true;
  }

  function cancelEdit() {
    editMode = false;
    inputPath = '';
    rootPathError.set(null);
  }

  async function applyChange() {
    if (!inputPath.trim()) return;
    const result = await setRootPath(inputPath.trim());
    if (result.success) {
      editMode = false;
      inputPath = '';
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      applyChange();
    } else if (e.key === 'Escape') {
      cancelEdit();
    }
  }

  function shortenPath(path: string, maxLen = 40): string {
    if (path.length <= maxLen) return path;
    const parts = path.split('/');
    if (parts.length <= 2) return '…' + path.slice(-maxLen + 1);
    // Keep first and last parts, ellipsis in middle
    const first = parts[0] || parts[1];
    const last = parts[parts.length - 1];
    return first + '/…/' + last;
  }
</script>

<div class="root-path-picker">
  {#if editMode}
    <div class="edit-mode">
      <input
        type="text"
        bind:value={inputPath}
        on:keydown={onKeydown}
        placeholder="/path/to/codebase"
        disabled={$rootPathLoading}
        class="path-input"
      />
      <div class="edit-actions">
        <button
          type="button"
          class="apply-btn"
          on:click={applyChange}
          disabled={$rootPathLoading || !inputPath.trim()}
        >
          {#if $rootPathLoading}
            Analyzing…
          {:else}
            Apply
          {/if}
        </button>
        <button
          type="button"
          class="cancel-btn"
          on:click={cancelEdit}
          disabled={$rootPathLoading}
        >
          Cancel
        </button>
      </div>
    </div>
    {#if $rootPathError}
      <div class="error">{$rootPathError}</div>
    {/if}
  {:else}
    <div class="display-mode">
      <span class="path-display" title={$rootPath}>
        {#if $rootPathLoading}
          Loading…
        {:else if $rootPath}
          📁 {shortenPath($rootPath)}
        {:else}
          <em>No path loaded</em>
        {/if}
      </span>
      <button
        type="button"
        class="change-btn"
        on:click={startEdit}
        disabled={$rootPathLoading}
        title="Change analyzed root folder"
      >
        Change
      </button>
    </div>
  {/if}
</div>

<style>
  .root-path-picker {
    margin-bottom: 0.5rem;
  }

  .display-mode {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .path-display {
    flex: 1;
    font-size: 0.85rem;
    color: var(--text-secondary, #888);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .change-btn {
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    color: var(--text-secondary, #888);
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    cursor: pointer;
    font-size: 0.75rem;
    flex-shrink: 0;
  }
  .change-btn:hover:not(:disabled) {
    background: var(--bg-hover, #333);
    color: var(--text);
  }
  .change-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .edit-mode {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .path-input {
    width: 100%;
    padding: 0.4rem 0.5rem;
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-family: monospace;
    font-size: 0.85rem;
    box-sizing: border-box;
  }
  .path-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .path-input:disabled {
    opacity: 0.6;
  }

  .edit-actions {
    display: flex;
    gap: 0.4rem;
  }

  .apply-btn, .cancel-btn {
    padding: 0.3rem 0.6rem;
    border-radius: 3px;
    cursor: pointer;
    font-size: 0.8rem;
  }

  .apply-btn {
    background: var(--accent);
    border: none;
    color: var(--accent-fg);
    flex: 1;
  }
  .apply-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .apply-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .cancel-btn {
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    color: var(--text-secondary, #888);
  }
  .cancel-btn:hover:not(:disabled) {
    background: var(--bg-hover, #333);
    color: var(--text);
  }
  .cancel-btn:disabled {
    opacity: 0.5;
  }

  .error {
    margin-top: 0.3rem;
    padding: 0.3rem 0.5rem;
    background: rgba(244, 67, 54, 0.15);
    border: 1px solid #f44336;
    border-radius: 3px;
    color: #ef5350;
    font-size: 0.8rem;
  }
</style>
