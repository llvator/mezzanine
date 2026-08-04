<script lang="ts">
  import {
    commits, commitsLoading, diffComputing, diffApiError,
    fetchCommits, triggerDiff, diffActive,
  } from '../stores/diff';

  let showPicker = false;
  let fromRef = '';
  let toRef = 'HEAD';

  async function openPicker() {
    showPicker = true;
    if ($commits.length === 0) {
      await fetchCommits();
    }
    // Default "to" to HEAD/first commit if not set
    if (!toRef && $commits.length > 0) {
      toRef = 'HEAD';
    }
  }

  function closePicker() {
    showPicker = false;
    diffApiError.set(null);
  }

  async function computeDiff() {
    if (!fromRef || !toRef) return;
    await triggerDiff(fromRef, toRef);
    if (!$diffApiError) {
      closePicker();
    }
  }

  async function showCurrentChanges() {
    await triggerDiff('HEAD', 'WORKING');
  }

  function selectCommit(hash: string, target: 'from' | 'to') {
    if (target === 'from') {
      fromRef = hash;
    } else {
      toRef = hash;
    }
  }

  function formatDate(dateStr: string): string {
    const d = new Date(dateStr);
    return d.toLocaleDateString() + ' ' + d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function truncateMessage(msg: string, maxLen = 60): string {
    const firstLine = msg.split('\n')[0];
    return firstLine.length > maxLen ? firstLine.slice(0, maxLen) + '…' : firstLine;
  }
</script>

<div class="commit-picker-trigger">
  <button type="button" class="picker-btn" on:click={showCurrentChanges} disabled={$diffComputing} title="Compare HEAD with current working directory">
    {$diffComputing ? '⏳' : '📝'} Current Changes
  </button>
  <button type="button" class="picker-btn" on:click={openPicker} title="Select commits to compare">
    🔄 Compare Commits
  </button>
</div>

{#if showPicker}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="picker-overlay" on:click|self={closePicker}>
    <div class="picker-modal">
      <header class="picker-header">
        <h3>Compare Commits</h3>
        <button type="button" class="close-btn" on:click={closePicker}>×</button>
      </header>

      <div class="picker-body">
        {#if $diffApiError}
          <div class="error-banner">{$diffApiError}</div>
        {/if}

        <div class="ref-inputs">
          <div class="ref-input-group">
            <label for="from-ref">From (base)</label>
            <input
              id="from-ref"
              type="text"
              bind:value={fromRef}
              placeholder="commit hash, branch, or tag"
            />
          </div>
          <span class="arrow">→</span>
          <div class="ref-input-group">
            <label for="to-ref">To (compare)</label>
            <input
              id="to-ref"
              type="text"
              bind:value={toRef}
              placeholder="HEAD, commit hash, branch..."
            />
          </div>
        </div>

        <div class="commit-list-section">
          <div class="commit-list-header">
            <span>Recent Commits</span>
            <button type="button" class="refresh-commits-btn" on:click={() => fetchCommits()} disabled={$commitsLoading}>
              {$commitsLoading ? '↻' : '↻ Refresh'}
            </button>
          </div>

          {#if $commitsLoading}
            <div class="loading">Loading commits…</div>
          {:else if $commits.length === 0}
            <div class="empty">No commits found. Is the server running?</div>
          {:else}
            <div class="commit-list">
              {#each $commits as commit (commit.hash)}
                <div class="commit-row" class:selected-from={fromRef === commit.hash} class:selected-to={toRef === commit.hash}>
                  <div class="commit-info">
                    <code class="commit-hash">{commit.short_hash}</code>
                    <span class="commit-msg" title={commit.message}>{truncateMessage(commit.message)}</span>
                    <span class="commit-meta">{commit.author} · {formatDate(commit.date)}</span>
                  </div>
                  <div class="commit-actions">
                    <button
                      type="button"
                      class="select-btn from"
                      class:active={fromRef === commit.hash}
                      on:click={() => selectCommit(commit.hash, 'from')}
                      title="Set as base (from)"
                    >From</button>
                    <button
                      type="button"
                      class="select-btn to"
                      class:active={toRef === commit.hash}
                      on:click={() => selectCommit(commit.hash, 'to')}
                      title="Set as target (to)"
                    >To</button>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <footer class="picker-footer">
        <button type="button" class="cancel-btn" on:click={closePicker}>Cancel</button>
        <button
          type="button"
          class="compute-btn"
          on:click={computeDiff}
          disabled={!fromRef || !toRef || $diffComputing}
        >
          {#if $diffComputing}
            Computing…
          {:else}
            Compare
          {/if}
        </button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .commit-picker-trigger {
    display: inline-block;
    margin-left: 0.5rem;
  }

  .picker-btn {
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    color: var(--text);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .picker-btn:hover {
    background: var(--bg-hover, #333);
  }

  .picker-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .picker-modal {
    background: var(--bg-body);
    border: 1px solid var(--border);
    border-radius: 8px;
    width: 90%;
    max-width: 700px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }

  .picker-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem;
    border-bottom: 1px solid var(--border);
  }
  .picker-header h3 {
    margin: 0;
    font-size: 1.1rem;
  }
  .close-btn {
    background: none;
    border: none;
    color: var(--text-secondary, #888);
    font-size: 1.5rem;
    cursor: pointer;
    line-height: 1;
  }
  .close-btn:hover {
    color: var(--text);
  }

  .picker-body {
    padding: 1rem;
    overflow-y: auto;
    flex: 1;
  }

  .error-banner {
    background: rgba(244, 67, 54, 0.15);
    border: 1px solid #f44336;
    color: #ef5350;
    padding: 0.5rem 0.75rem;
    border-radius: 4px;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }

  .ref-inputs {
    display: flex;
    align-items: flex-end;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }
  .ref-input-group {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .ref-input-group label {
    font-size: 0.8rem;
    color: var(--text-secondary, #888);
  }
  .ref-input-group input {
    padding: 0.5rem;
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-family: monospace;
    font-size: 0.9rem;
  }
  .ref-input-group input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .arrow {
    color: var(--text-secondary, #888);
    font-size: 1.2rem;
    padding-bottom: 0.4rem;
  }

  .commit-list-section {
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
  }
  .commit-list-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.5rem 0.75rem;
    background: var(--bg-surface-alt);
    border-bottom: 1px solid var(--border);
    font-size: 0.85rem;
    color: var(--text-secondary, #888);
  }
  .refresh-commits-btn {
    background: none;
    border: none;
    color: var(--text-secondary, #888);
    cursor: pointer;
    font-size: 0.85rem;
  }
  .refresh-commits-btn:hover:not(:disabled) {
    color: var(--text);
  }
  .refresh-commits-btn:disabled {
    opacity: 0.5;
  }

  .loading, .empty {
    padding: 2rem;
    text-align: center;
    color: var(--text-secondary, #888);
  }

  .commit-list {
    max-height: 300px;
    overflow-y: auto;
  }

  .commit-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    transition: background 0.15s;
  }
  .commit-row:last-child {
    border-bottom: none;
  }
  .commit-row:hover {
    background: var(--bg-hover, #2a2a2a);
  }
  .commit-row.selected-from {
    background: rgba(76, 175, 80, 0.15);
  }
  .commit-row.selected-to {
    background: rgba(33, 150, 243, 0.15);
  }
  .commit-row.selected-from.selected-to {
    background: linear-gradient(90deg, rgba(76, 175, 80, 0.15) 50%, rgba(33, 150, 243, 0.15) 50%);
  }

  .commit-info {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    overflow: hidden;
    flex: 1;
    min-width: 0;
  }
  .commit-hash {
    font-family: monospace;
    font-size: 0.8rem;
    color: var(--accent);
  }
  .commit-msg {
    font-size: 0.9rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .commit-meta {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .commit-actions {
    display: flex;
    gap: 0.25rem;
    margin-left: 0.5rem;
    flex-shrink: 0;
  }
  .select-btn {
    padding: 0.2rem 0.5rem;
    font-size: 0.75rem;
    border-radius: 3px;
    cursor: pointer;
    border: 1px solid transparent;
    background: var(--bg-surface-alt);
    color: var(--text-secondary, #888);
  }
  .select-btn:hover {
    background: var(--bg-hover, #333);
    color: var(--text);
  }
  .select-btn.from.active {
    background: rgba(76, 175, 80, 0.3);
    border-color: #4CAF50;
    color: #A5D6A7;
  }
  .select-btn.to.active {
    background: rgba(33, 150, 243, 0.3);
    border-color: #2196F3;
    color: #90CAF9;
  }

  .picker-footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding: 1rem;
    border-top: 1px solid var(--border);
  }
  .cancel-btn, .compute-btn {
    padding: 0.5rem 1rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.9rem;
  }
  .cancel-btn {
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    color: var(--text-secondary, #888);
  }
  .cancel-btn:hover {
    background: var(--bg-hover, #333);
    color: var(--text);
  }
  .compute-btn {
    background: var(--accent);
    border: none;
    color: var(--accent-fg);
  }
  .compute-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .compute-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
