<script lang="ts">
  import type { D3Node } from '../types/graph';
  import { graphLevel } from '../stores/graph';
  import {
    contextDepth,
    scopeMode,
    scopeResponse,
    scopeLoading,
    contextFiles,
    excludedFiles,
    fetchScope,
    toggleContextFile,
    toggleAllContextFiles,
  } from '../viewmodels/contextScope';
  import type { ScopeMode } from '../viewmodels/contextScope';
  import { copyToClipboard } from '../utils/clipboard';

  export let entity: D3Node;

  let collapsed = true;

  // Trigger API call when inputs change
  $: if (entity) fetchScope();
  $: $scopeMode, fetchScope();
  $: $contextDepth, fetchScope();
  $: $excludedFiles, fetchScope();

  $: resp = $scopeResponse;
  $: entityCount = resp?.entities.length ?? 0;
  $: files = $contextFiles;
  $: entityTokens = resp?.token_count_entities ?? 0;
  $: fileTokens = resp?.token_count_files ?? 0;

  // Reason chips (smart modes only)
  const REASON_ORDER = [
    'Selected', 'Parent', 'Siblings',
    'Callers', 'Callees', 'Types',
    'Traits/Interfaces', 'Base Classes', 'Imports', 'Reachable',
  ];
  $: reasonGroups = (() => {
    if ($scopeMode === 'manual' || !resp) return [];
    const summary = resp.reason_summary;
    return REASON_ORDER
      .filter((r) => summary[r] != null && summary[r] > 0)
      .map((r) => ({ reason: r, count: summary[r] }));
  })();

  // Copy logic — use pre-built exports from API response
  let copyFeedback: Record<string, string> = {};

  async function copyExport(key: string, text: string) {
    const ok = await copyToClipboard(text);
    copyFeedback = { ...copyFeedback, [key]: ok ? 'Copied!' : 'Failed' };
    setTimeout(() => {
      copyFeedback = { ...copyFeedback, [key]: '' };
    }, 1500);
  }

  // An engine older than SRV-010 returns no prompt. Hide the action rather
  // than offering a button that copies "undefined".
  $: hasPrompt = !!resp?.exports?.refactor_prompt;

  function isFileIncluded(path: string): boolean {
    return !$excludedFiles.has(path);
  }

  const MODES: { value: ScopeMode; label: string }[] = [
    { value: 'manual', label: 'Manual' },
    { value: 'refactor', label: 'Refactor' },
    { value: 'understand', label: 'Understand' },
  ];
</script>

{#if $graphLevel === 'entity' && entity}
  <div class="context-scope">
    <button type="button" class="scope-header" on:click={() => (collapsed = !collapsed)}>
      <span class="toggle-arrow">{collapsed ? '▸' : '▾'}</span>
      Context Scope
      {#if collapsed}
        <span class="scope-summary">
          {$scopeMode !== 'manual' ? $scopeMode : `depth ${$contextDepth}`} | {entityCount} entities | ~{entityTokens.toLocaleString()} tok
          {#if $scopeLoading}<span class="loading-dot">…</span>{/if}
        </span>
      {/if}
    </button>

    {#if !collapsed}
      <div class="scope-body">
        <!-- Mode selector -->
        <div class="scope-row">
          <span class="scope-label">Mode</span>
          <div class="depth-group">
            {#each MODES as m}
              <button
                type="button"
                class="depth-btn"
                class:active={$scopeMode === m.value}
                on:click={() => scopeMode.set(m.value)}
              >{m.label}</button>
            {/each}
          </div>
        </div>

        <!-- Depth selector (manual mode only) -->
        {#if $scopeMode === 'manual'}
          <div class="scope-row">
            <span class="scope-label">Depth</span>
            <div class="depth-group">
              {#each [0, 1, 2, 3] as d}
                <button
                  type="button"
                  class="depth-btn"
                  class:active={$contextDepth === d}
                  on:click={() => contextDepth.set(d)}
                >{d}</button>
              {/each}
            </div>
          </div>
        {/if}

        <!-- Reason breakdown (smart modes only) -->
        {#if $scopeMode !== 'manual' && reasonGroups.length > 0}
          <div class="reason-section">
            <span class="scope-label">Included ({entityCount})</span>
            <div class="reason-list">
              {#each reasonGroups as group}
                <span class="reason-chip">
                  <span class="reason-label">{group.reason}</span>
                  <span class="reason-count">{group.count}</span>
                </span>
              {/each}
            </div>
          </div>
        {/if}

        <!-- File selector -->
        <div class="file-section">
          <div class="file-header">
            <span class="scope-label">Files ({files.length})</span>
            <div class="file-toggle-btns">
              <button type="button" class="mini-btn" on:click={() => toggleAllContextFiles(true, files.map(f => f.path))}>All</button>
              <button type="button" class="mini-btn" on:click={() => toggleAllContextFiles(false, files.map(f => f.path))}>None</button>
            </div>
          </div>
          <div class="file-list">
            {#each files as file}
              <label class="file-item">
                <input
                  type="checkbox"
                  checked={isFileIncluded(file.path)}
                  on:change={(e) => toggleContextFile(file.path, e.currentTarget.checked)}
                />
                <span class="file-path">{file.path}</span>
                <span class="file-count">{file.count}</span>
              </label>
            {/each}
          </div>
        </div>

        <div class="scope-stats">
          <span>{entityCount} entities</span>
          <span class="stat-sep">|</span>
          <span>~{entityTokens.toLocaleString()} tok (entities)</span>
          <span class="stat-sep">|</span>
          <span>~{fileTokens.toLocaleString()} tok (files)</span>
          {#if $scopeLoading}<span class="loading-dot">…</span>{/if}
        </div>

        <div class="scope-actions">
          {#if hasPrompt}
            <button
              type="button"
              class="copy-btn copy-btn-primary"
              disabled={!resp}
              on:click={() => resp?.exports.refactor_prompt && copyExport('prompt', resp.exports.refactor_prompt)}
              title="Copy a ready-to-use refactoring prompt: the measured problems, remediation guidance, and this entity's context"
            >
              {copyFeedback['prompt'] || 'Copy Refactor Prompt'}
            </button>
          {/if}
          <button type="button" class="copy-btn" disabled={!resp} on:click={() => resp && copyExport('paths', resp.exports.paths)} title="Copy file paths">
            {copyFeedback['paths'] || 'Paths'}
          </button>
          <button type="button" class="copy-btn" disabled={!resp} on:click={() => resp && copyExport('ranges', resp.exports.ranges)} title="Copy paths with line ranges">
            {copyFeedback['ranges'] || 'Ranges'}
          </button>
          <button type="button" class="copy-btn" disabled={!resp} on:click={() => resp && copyExport('entityCtx', resp.exports.entity_context)} title="Copy entity source code">
            {copyFeedback['entityCtx'] || 'Entity Context'}
          </button>
          <button type="button" class="copy-btn copy-btn-accent" disabled={!resp} on:click={() => resp && copyExport('fullFiles', resp.exports.full_files)} title="Copy entire file contents">
            {copyFeedback['fullFiles'] || 'Full Files'}
          </button>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .context-scope {
    margin-top: 12px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }

  .scope-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--bg-hover) 50%, transparent);
    border: none;
    color: var(--text-secondary);
    font-size: 0.8rem;
    font-family: inherit;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    cursor: pointer;
  }

  .scope-header:hover {
    background: var(--bg-hover);
  }

  .toggle-arrow {
    font-size: 0.7rem;
    flex-shrink: 0;
    width: 12px;
  }

  .scope-summary {
    margin-left: auto;
    font-weight: 400;
    font-size: 0.7rem;
    color: var(--text-dim);
    text-transform: none;
  }

  .scope-body {
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .scope-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .scope-label {
    font-size: 0.7rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }

  .depth-group {
    display: flex;
    gap: 2px;
  }

  .depth-btn {
    padding: 3px 10px;
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    color: var(--text-secondary);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    font-size: 0.75rem;
    font-family: inherit;
    cursor: pointer;
    transition: background 0.12s ease, border-color 0.12s ease;
  }

  .depth-btn:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
  }

  .depth-btn.active {
    background: rgba(33, 150, 243, 0.2);
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 700;
  }

  /* Reason chips */
  .reason-section {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .reason-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .reason-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    font-size: 0.72rem;
  }

  .reason-label {
    color: var(--text-secondary);
  }

  .reason-count {
    color: var(--accent);
    font-weight: 700;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.7rem;
  }

  /* File selector */
  .file-section {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .file-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .file-toggle-btns {
    display: flex;
    gap: 2px;
  }

  .mini-btn {
    padding: 1px 6px;
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    color: var(--text-dim);
    border: 1px solid var(--border-subtle);
    border-radius: 3px;
    font-size: 0.65rem;
    font-family: inherit;
    cursor: pointer;
  }

  .mini-btn:hover {
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .file-list {
    max-height: 150px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .file-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
    border-radius: 3px;
    font-size: 0.75rem;
    cursor: pointer;
  }

  .file-item:hover {
    background: color-mix(in srgb, var(--bg-hover) 30%, transparent);
  }

  .file-item input[type="checkbox"] {
    cursor: pointer;
    flex-shrink: 0;
  }

  .file-path {
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-count {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 0.65rem;
    color: var(--text-dim);
  }

  /* Stats */
  .scope-stats {
    font-size: 0.72rem;
    color: var(--text-secondary);
    font-family: 'Monaco', 'Menlo', monospace;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
  }

  .stat-sep {
    color: var(--text-dim);
  }

  .loading-dot {
    color: var(--accent);
    animation: pulse 1s infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 0.3; }
    50% { opacity: 1; }
  }

  /* Actions */
  .scope-actions {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .copy-btn {
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    color: var(--text-secondary);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    padding: 3px 10px;
    font-size: 0.7rem;
    font-family: inherit;
    cursor: pointer;
    transition: background 0.12s ease, border-color 0.12s ease;
  }

  .copy-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: #4CAF50;
  }

  .copy-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .copy-btn-accent {
    border-color: rgba(33, 150, 243, 0.4);
  }

  .copy-btn-accent:hover:not(:disabled) {
    border-color: var(--accent);
  }

  /* The recommended action. The four raw-context buttons beside it stay the
     escape hatch for people who want to assemble their own prompt, so this
     one carries the fill and takes the whole first row. */
  .copy-btn-primary {
    flex-basis: 100%;
    background: rgba(33, 150, 243, 0.18);
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }

  .copy-btn-primary:hover:not(:disabled) {
    background: rgba(33, 150, 243, 0.3);
    border-color: var(--accent);
  }
</style>
