<script lang="ts">
  import {
    commits, commitsLoading, diffComputing, diffApiError,
    fetchCommits, triggerDiff, diffActive,
    stashes, stashesLoading, fetchStashes, type Stash,
    stagedFiles, stagedLoading, fetchStaged,
  } from '../stores/diff';

  let showPicker = false;
  let fromRef = '';
  let toRef = 'HEAD';

  /**
   * Which listing the modal is showing. Stashes get their own mode rather
   * than a row in the commit list: a stash carries its own base, so there is
   * nothing for the reader to pair up, and the From/To inputs the commit mode
   * is built around would be two ways to get the pairing wrong (UI-107).
   *
   * The index gets one for the same reason, plus two of its own. Its base is
   * always HEAD, so there is nothing to pair; and unlike Current Changes it
   * needs somewhere to say two things — that the canvas will not become the
   * index, and that nothing is staged, which is the ordinary state of a
   * repository rather than a failed comparison (UI-111).
   */
  let mode: 'commits' | 'stashes' | 'staged' = 'commits';

  async function openPicker() {
    mode = 'commits';
    showPicker = true;
    if ($commits.length === 0) {
      await fetchCommits();
    }
    // Default "to" to HEAD/first commit if not set
    if (!toRef && $commits.length > 0) {
      toRef = 'HEAD';
    }
  }

  async function openStashes() {
    mode = 'stashes';
    showPicker = true;
    await fetchStashes();
  }

  async function openStaged() {
    mode = 'staged';
    showPicker = true;
    await fetchStaged();
  }

  /**
   * Compare the index against HEAD.
   *
   * `STAGED` is a sentinel and not a hash, which is the whole of why the list
   * above it carries no hashes either. The index is the one tree in a
   * repository that no ref resolves to, so the server manufactures a commit
   * for it — and that commit means whatever the index held when it was made.
   * Sending the sentinel is what makes the comparison be of the index as it is
   * *now*, however long this modal has been open.
   */
  async function compareStaged() {
    await triggerDiff('HEAD', 'STAGED');
    if (!$diffApiError) {
      closePicker();
    }
  }

  /** git's status letters, for the one-glance read of what a row is. */
  const STAGED_STATUS: Record<string, string> = {
    M: 'modified', A: 'added', D: 'deleted',
    R: 'renamed', C: 'copied', T: 'type changed',
  };

  /**
   * Compare a stash against the commit it was taken on.
   *
   * Both refs are hashes. `stash@{N}` is a position in the stash list and
   * every `git stash` renumbers it, so a selector sent to the server names
   * whichever stash happens to sit there when the diff runs — which need not
   * be the one that was clicked.
   */
  async function compareStash(stash: Stash) {
    await triggerDiff(stash.base_hash, stash.hash);
    if (!$diffApiError) {
      closePicker();
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
  <button type="button" class="picker-btn" on:click={openStaged} title="Compare what you have staged against HEAD">
    🗂 Staged
  </button>
  <button type="button" class="picker-btn" on:click={openStashes} title="Look at work set aside with git stash">
    📦 Stashes
  </button>
</div>

{#if showPicker}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="picker-overlay" on:click|self={closePicker}>
    <div class="picker-modal">
      <header class="picker-header">
        <h3>{mode === 'stashes' ? 'Stashes' : mode === 'staged' ? 'Staged Changes' : 'Compare Commits'}</h3>
        <button type="button" class="close-btn" on:click={closePicker}>×</button>
      </header>

      <div class="picker-body">
        {#if $diffApiError}
          <div class="error-banner">{$diffApiError}</div>
        {/if}

      {#if mode === 'staged'}
        <!-- Same trap as the stash note, arrived at from the other side. The
             index reads as *more* live than a stash, so the pull to expect the
             canvas to become it is stronger — and it is a commit all the same,
             manufactured in the diff call and checked out into a temp
             worktree, so the server declines to adopt it (SRV-019). -->
        <p class="stash-note" data-probe="staged-note">
          Your staged changes, compared against HEAD. The canvas keeps drawing your
          working tree — the overlay describes the index.
        </p>

        <div class="commit-list-section">
          <div class="commit-list-header">
            <span>In the index</span>
            <button type="button" class="refresh-commits-btn" on:click={() => fetchStaged()} disabled={$stagedLoading}>
              {$stagedLoading ? '↻' : '↻ Refresh'}
            </button>
          </div>

          {#if $stagedLoading}
            <div class="loading">Reading the index…</div>
          {:else if $stagedFiles.length === 0}
            <!-- Nothing staged is the ordinary state of a repository. Said
                 here, before a comparison runs, because an empty overlay would
                 say "nothing changed" about the wrong thing. -->
            <div class="empty">Nothing is staged. <code>git add</code> the changes you want to look at.</div>
          {:else}
            <div class="commit-list" data-probe="staged-list">
              {#each $stagedFiles as file (file.path)}
                <div class="commit-row">
                  <div class="commit-info">
                    <span class="commit-msg" title={file.path}>{file.path}</span>
                    <span class="commit-meta">{STAGED_STATUS[file.status] ?? file.status}</span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {:else if mode === 'stashes'}
        <!-- The one thing a reader cannot see for themselves. A stash reads
             as uncommitted work, so it is natural to expect the canvas to
             become it — but the server only adopts a working-tree head as
             live state (SRV-019), and a stash is a commit. The circles stay
             the working tree; the colours describe the stash. -->
        <p class="stash-note" data-probe="stash-note">
          A stash is compared against the commit it was taken on. The canvas keeps
          drawing your working tree — the overlay describes the stash.
        </p>

        <div class="commit-list-section">
          <div class="commit-list-header">
            <span>Stashed Changes</span>
            <button type="button" class="refresh-commits-btn" on:click={() => fetchStashes()} disabled={$stashesLoading}>
              {$stashesLoading ? '↻' : '↻ Refresh'}
            </button>
          </div>

          {#if $stashesLoading}
            <div class="loading">Loading stashes…</div>
          {:else if $stashes.length === 0}
            <!-- Nothing stashed is the ordinary state of a repository, and
                 saying so is not the same as reporting a failure. -->
            <div class="empty">No stashes. Nothing has been set aside with <code>git stash</code>.</div>
          {:else}
            <div class="commit-list" data-probe="stash-list">
              {#each $stashes as stash (stash.hash)}
                <div class="commit-row">
                  <div class="commit-info">
                    <code class="commit-hash">{stash.selector}</code>
                    <span class="commit-msg" title={stash.message}>{truncateMessage(stash.message)}</span>
                    <!-- The base is shown, not assumed: pairing a stash with
                         HEAD instead reports every commit landed since as
                         something the stash removed. -->
                    <span class="commit-meta">
                      on <code class="commit-hash">{stash.base_short}</code> · {stash.author} · {formatDate(stash.date)}
                    </span>
                  </div>
                  <div class="commit-actions">
                    <button
                      type="button"
                      class="select-btn to"
                      on:click={() => void compareStash(stash)}
                      disabled={$diffComputing}
                      title="Compare this stash against {stash.base_short}, the commit it was taken on"
                    >{$diffComputing ? '…' : 'Show'}</button>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {:else}
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
      {/if}
      </div>

      <footer class="picker-footer">
        <button type="button" class="cancel-btn" on:click={closePicker}>Cancel</button>
        <!-- Stash mode has no Compare: each row carries its own base, so the
             comparison is decided by which row was clicked and there is
             nothing left for a footer button to confirm. Staged mode is the
             opposite — one comparison, no rows to choose between — so the
             button is the whole of the choice, and the list above it is what
             the reader is agreeing to. -->
        {#if mode === 'staged'}
        <button
          type="button"
          class="compute-btn"
          data-probe="staged-compare"
          on:click={compareStaged}
          disabled={$stagedFiles.length === 0 || $stagedLoading || $diffComputing}
          title="Compare the index against HEAD"
        >
          {$diffComputing ? 'Computing…' : 'Compare staged'}
        </button>
        {:else if mode !== 'stashes'}
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
        {/if}
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

  .stash-note {
    margin: 0 0 1rem;
    padding: 0.5rem 0.75rem;
    border-left: 2px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    font-size: 0.85rem;
    line-height: 1.45;
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
