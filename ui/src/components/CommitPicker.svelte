<script lang="ts">
  import {
    commits, commitsLoading, diffComputing, diffApiError,
    fetchCommits, triggerDiff, diffActive,
    diffStopping, cancelDiff,
    stashes, stashesLoading, fetchStashes, type Stash,
    stagedFiles, stagedLoading, fetchStaged,
    branches, fetchBranches,
  } from '../stores/diff';
  import { knownCommits } from '../stores/diff';
  import BranchCompare from './BranchCompare.svelte';
  import { orderBranches } from '../viewmodels/branchCompare';
  import {
    railRows, baseRefFor, includedCount, rangeWarning, findCommit, isRoot,
    type BaseRef,
  } from '../viewmodels/commitRange';

  /**
   * What the Stop button says it will do (UI-141).
   *
   * Both halves matter. A comparison of two commits checks out and analyses
   * two whole trees, so a reader who picked the wrong pair is otherwise
   * watching a minute of work they no longer want with nothing to press. And
   * stopping is *not* leaving diff mode: whatever overlay they were already
   * looking at is still there afterwards, which is the reason to stop rather
   * than to clear.
   */
  const STOP_TITLE =
    'Stop this comparison. The engine ends at its next checkpoint; '
    + 'the diff you were already looking at stays on screen.';

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
   *
   * Branches get one because their base is *computed* rather than picked: two
   * branches are compared from where they diverged, which is a question only
   * the server can answer and a sentence the reader has to be shown (UI-143).
   */
  type Mode = 'commits' | 'branches' | 'stashes' | 'staged';
  let mode: Mode = 'commits';

  /** One heading per mode. A table rather than a nested ternary that grows a
   *  branch every time the modal learns to show something new. */
  const HEADINGS: Record<Mode, string> = {
    commits: 'Compare Commits',
    branches: 'Compare Branches',
    stashes: 'Stashes',
    staged: 'Staged Changes',
  };

  /**
   * Whose history the commit list is showing. Empty means the checkout's own
   * `HEAD`, which is the only listing there used to be.
   *
   * The list is one branch at a time and `From`/`To` are not, which is what
   * makes a cross-branch pair possible: pick the base from one branch, switch
   * the list, pick the target from another. The two selections are held as
   * hashes, so switching the list underneath them changes nothing (UI-143).
   */
  let listRef = '';

  async function openPicker() {
    mode = 'commits';
    showPicker = true;
    if ($commits.length === 0) {
      await fetchCommits(listRef || undefined);
    }
    // The branch list is what the commit list is switched with. Cheap, and
    // fetched here rather than on first use so the dropdown is never empty
    // for the moment after it appears.
    if ($branches.length === 0) {
      void fetchBranches();
    }
    // Default "to" to HEAD/first commit if not set
    if (!toRef && $commits.length > 0) {
      toRef = 'HEAD';
    }
  }

  /** Show another branch's history in the list below. */
  async function listBranch(name: string) {
    listRef = name;
    await fetchCommits(name || undefined);
  }

  async function openBranches() {
    mode = 'branches';
    showPicker = true;
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

  /**
   * The range the two picks describe, drawn onto the listing (UI-151).
   *
   * `From` names the oldest commit *included*, so the tree the engine starts
   * from is the one below it — and the rail is what makes that visible
   * instead of a rule the reader has to hold in their head. See
   * `viewmodels/commitRange.ts`.
   */
  $: hashes = $commits.map((c) => c.hash);
  /** What `HEAD` means in the list on screen. Nothing, when the list is
   *  another branch's: the checkout's HEAD is genuinely not in it. */
  $: headHash = listRef === '' ? ($commits[0]?.hash ?? null) : null;
  $: rail = railRows(hashes, fromRef, toRef, headHash);
  $: railByHash = new Map(rail.map((r) => [r.hash, r]));
  $: included = includedCount(rail);
  $: warning = rangeWarning(hashes, fromRef, toRef, headHash);
  /** Looked up in everything known rather than the list on screen, so a base
   *  picked on one branch survives switching the list under it (UI-143). */
  $: base = (fromRef ? baseRefFor(fromRef, $knownCommits) : { ref: null }) as BaseRef;
  $: baseCommit = base.ref ? findCommit(base.ref, $knownCommits) : undefined;

  async function computeDiff() {
    // `base.ref`, not `fromRef`: the engine is given the tree to start from,
    // and the reader named the first commit they wanted to see. The button is
    // already disabled when there is no such tree, so this guard is the
    // second of two.
    if (!fromRef || !toRef || !base.ref) return;
    await triggerDiff(base.ref, toRef, {
      inclusiveFrom: findCommit(fromRef, $knownCommits),
    });
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
  <button type="button" class="picker-btn" on:click={openPicker} title="Select commits to compare — from this branch or any other">
    🔄 Compare Commits
  </button>
  <button type="button" class="picker-btn" on:click={openBranches} title="Review one branch against another, from where they diverged">
    🌿 Branches
  </button>
  <button type="button" class="picker-btn" on:click={openStaged} title="Compare what you have staged against HEAD">
    🗂 Staged
  </button>
  <button type="button" class="picker-btn" on:click={openStashes} title="Look at work set aside with git stash">
    📦 Stashes
  </button>
  <!-- Only while something is running, and outside the modal as well as in
       it: `Current Changes` starts a comparison with no modal at all, and
       every other button in this row is disabled for as long as it lasts. -->
  {#if $diffComputing}
    <button
      type="button"
      class="picker-btn stop-btn"
      data-probe="diff-cancel"
      on:click={() => void cancelDiff()}
      disabled={$diffStopping}
      title={STOP_TITLE}
    >
      {$diffStopping ? '⏳ Stopping…' : '⏹ Stop'}
    </button>
  {/if}
</div>

{#if showPicker}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="picker-overlay" on:click|self={closePicker}>
    <div class="picker-modal">
      <header class="picker-header">
        <h3>{HEADINGS[mode]}</h3>
        <button type="button" class="close-btn" on:click={closePicker}>×</button>
      </header>

      <div class="picker-body">
        {#if $diffApiError}
          <div class="error-banner">{$diffApiError}</div>
        {/if}

      {#if mode === 'branches'}
        <BranchCompare onDone={closePicker} />
      {:else if mode === 'staged'}
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
        <!-- Both ends are *included*, and the labels say so. `From (base)`
             was accurate about the engine and wrong about the reader: the
             commit they clicked was the one commit the comparison could not
             see, so reviewing four commits meant hunting for the fifth
             (UI-151). -->
        <div class="ref-inputs">
          <div class="ref-input-group">
            <label for="from-ref">From — oldest commit included</label>
            <input
              id="from-ref"
              type="text"
              bind:value={fromRef}
              placeholder="commit hash, branch, or tag"
            />
          </div>
          <span class="arrow">→</span>
          <div class="ref-input-group">
            <label for="to-ref">To — newest commit included</label>
            <input
              id="to-ref"
              type="text"
              bind:value={toRef}
              placeholder="HEAD, commit hash, branch..."
            />
          </div>
        </div>

        <!-- What the pair actually resolves to, in one line, before a minute
             of analysis rather than after it. Naming the base tree here is
             the whole of the ambiguity: the reader can see that it sits
             outside what they asked for, and the rail below shows where. -->
        <p class="range-note" data-probe="commit-range-note">
          {#if base.problem === 'root'}
            <span class="range-blocked">
              That is the first commit in this history — there is no earlier tree to
              compare it against, so it cannot be the oldest one included.
            </span>
          {:else if included}
            Comparing <strong>{included}</strong> commit{included === 1 ? '' : 's'}, both ends
            included.{#if baseCommit}{' '}Starting from the tree at
              <code class="commit-hash">{baseCommit.short_hash}</code>, which is not part of it.{/if}
          {:else if fromRef && toRef}
            Both ends included. The comparison starts from the tree just before
            <code class="commit-hash">{fromRef.slice(0, 12)}</code>.
          {:else}
            Pick the oldest and newest commits you want to see.
          {/if}
        </p>
        {#if warning}
          <p class="range-warning" data-probe="commit-range-warning">{warning}</p>
        {/if}

        <div class="commit-list-section">
          <div class="commit-list-header">
            <!-- Which history is listed, as a control rather than a caption.
                 `From` and `To` hold hashes, so switching the list under them
                 loses nothing: that is what makes a base on one branch and a
                 target on another two clicks apart (UI-143). -->
            <span class="list-source">
              <label for="list-branch">Commits on</label>
              <select
                id="list-branch"
                data-probe="commit-list-branch"
                value={listRef}
                on:change={(e) => void listBranch(e.currentTarget.value)}
                disabled={$commitsLoading}
              >
                <option value="">this checkout (HEAD)</option>
                {#each orderBranches($branches) as b (b.name)}
                  <option value={b.name}>{b.name}</option>
                {/each}
              </select>
            </span>
            <button type="button" class="refresh-commits-btn" on:click={() => fetchCommits(listRef || undefined)} disabled={$commitsLoading}>
              {$commitsLoading ? '↻' : '↻ Refresh'}
            </button>
          </div>

          {#if $commitsLoading}
            <div class="loading">Loading commits…</div>
          {:else if $commits.length === 0}
            <div class="empty">No commits found. Is the server running?</div>
          {:else}
            <!-- The rail down the left is the boundary, drawn (UI-151). Row
                 tints said *which two rows were clicked*; a reader still had
                 to work out what lay between them and which side of `From`
                 the comparison began on. A line with a node per commit says
                 both at a glance, and the divider below `From` puts the one
                 excluded commit visibly outside it. -->
            <div class="commit-list" data-probe="commit-list">
              {#each $commits as commit (commit.hash)}
                {@const row = railByHash.get(commit.hash)}
                {#if row?.boundaryAbove}
                  <div class="range-boundary" data-probe="range-boundary">
                    <span>base — everything below is where the comparison starts from</span>
                  </div>
                {/if}
                <div
                  class="commit-row"
                  class:in-range={row?.included}
                  class:selected-from={row?.isFrom}
                  class:selected-to={row?.isTo}
                >
                  <!-- Decoration: every state it shows is also written in the
                       row beside it or in the sentence above the list. -->
                  <div class="rail" aria-hidden="true">
                    <span class="rail-line up" class:lit={row?.litAbove}></span>
                    <span
                      class="rail-node"
                      class:from={row?.isFrom}
                      class:to={row?.isTo}
                      class:inside={row?.included}
                      class:base={row?.isBase}
                    ></span>
                    <span class="rail-line down" class:lit={row?.litBelow}></span>
                  </div>
                  <div class="commit-info">
                    <code class="commit-hash">{commit.short_hash}</code>
                    <span class="commit-msg" title={commit.message}>{truncateMessage(commit.message)}</span>
                    <span class="commit-meta">{commit.author} · {formatDate(commit.date)}</span>
                  </div>
                  <div class="commit-actions">
                    <!-- Declined on the root commit rather than sent and
                         failed: there is no tree before the first commit, and
                         finding that out from a red banner after a minute of
                         analysis is the worse way to learn it. -->
                    <button
                      type="button"
                      class="select-btn from"
                      class:active={fromRef === commit.hash}
                      disabled={isRoot(commit, $commits)}
                      on:click={() => selectCommit(commit.hash, 'from')}
                      title={isRoot(commit, $commits)
                        ? 'The first commit in this history — nothing earlier to compare it against'
                        : 'Include this commit and everything after it'}
                    >From</button>
                    <button
                      type="button"
                      class="select-btn to"
                      class:active={toRef === commit.hash}
                      on:click={() => selectCommit(commit.hash, 'to')}
                      title="Stop at this commit, including it"
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
        <!-- In every mode, including stashes: a stash row's `Show` starts the
             same two-worktree comparison the footer button does, and the
             modal stays open for all of it. `Cancel` beside it closes this
             dialog and leaves the engine working — the two words are close
             enough that the tooltip has to say which is which. -->
        {#if $diffComputing}
        <button
          type="button"
          class="stop-btn"
          data-probe="diff-cancel-footer"
          on:click={() => void cancelDiff()}
          disabled={$diffStopping}
          title={STOP_TITLE}
        >
          {$diffStopping ? 'Stopping…' : '⏹ Stop'}
        </button>
        {/if}
        <!-- Stash mode has no Compare: each row carries its own base, so the
             comparison is decided by which row was clicked and there is
             nothing left for a footer button to confirm. Branch mode has none
             for the mirror reason — its button sits under the sentence saying
             what will be compared, which is the thing being agreed to. Staged
             mode is the opposite of both — one comparison, no rows to choose
             between — so the button is the whole of the choice, and the list
             above it is what the reader is agreeing to. -->
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
        {:else if mode === 'commits'}
        <button
          type="button"
          class="compute-btn"
          on:click={computeDiff}
          disabled={!fromRef || !toRef || !base.ref || $diffComputing}
          title={base.problem === 'root'
            ? 'There is no tree before the first commit to compare against'
            : 'Compare the range, both ends included'}
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
  .list-source {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }
  .list-source select {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text);
    font-family: monospace;
    font-size: 0.8rem;
    padding: 0.15rem 0.25rem;
    max-width: 16rem;
  }
  .list-source select:disabled {
    opacity: 0.6;
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
    align-items: stretch;
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
  /* The span, not the two clicks. Faint, because the rail beside it is what
     states the range and a tint strong enough to compete would say the rows
     between the ends were selected too. */
  .commit-row.in-range {
    background: rgba(33, 150, 243, 0.06);
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

  /* --- The rail (UI-151) ---------------------------------------------- */

  /* A column of its own rather than a border on the row, so the line can stop
     at a node instead of running the full height of every row: where the
     accent *ends* is the boundary, and a border cannot express that. */
  .rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 14px;
    flex-shrink: 0;
    margin-right: 0.6rem;
    align-self: stretch;
  }
  .rail-line {
    flex: 1;
    width: 2px;
    min-height: 6px;
    background: var(--border);
  }
  .rail-line.lit {
    background: #2196F3;
  }
  .rail-node {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--bg-surface-alt);
    border: 2px solid var(--border);
    box-sizing: border-box;
  }
  .rail-node.inside {
    border-color: #2196F3;
    background: #2196F3;
  }
  /* Hollow ends. Both are *in* the comparison, and a filled node next to a
     filled span would leave nothing saying which two the reader chose. */
  .rail-node.from {
    width: 12px;
    height: 12px;
    border-color: #4CAF50;
    background: var(--bg-body);
  }
  .rail-node.to {
    width: 12px;
    height: 12px;
    border-color: #2196F3;
    background: var(--bg-body);
  }
  /* Outside the range and named as such — the one commit that used to be
     picked by accident. */
  .rail-node.base {
    border-style: dashed;
    border-color: var(--text-dim);
    background: transparent;
  }

  /* The answer to "where does it start". A divider rather than a caption on a
     row, because the thing being drawn is the seam between two commits and
     not a property of either. */
  .range-boundary {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.75rem;
    background: var(--bg-surface-alt);
    border-bottom: 1px solid var(--border);
    border-top: 1px dashed var(--text-dim);
    font-size: 0.7rem;
    color: var(--text-dim);
    text-transform: lowercase;
    letter-spacing: 0.02em;
  }

  .range-note {
    margin: 0 0 0.75rem;
    font-size: 0.8rem;
    line-height: 1.45;
    color: var(--text-secondary, #888);
  }
  .range-note strong {
    color: var(--text);
  }
  .range-blocked {
    color: #ef9a9a;
  }
  .range-warning {
    margin: -0.35rem 0 0.75rem;
    padding: 0.4rem 0.6rem;
    border-left: 2px solid #FFB74D;
    background: rgba(255, 183, 77, 0.08);
    font-size: 0.8rem;
    line-height: 1.45;
    color: var(--text-secondary, #888);
  }

  .commit-info {
    display: flex;
    flex-direction: column;
    justify-content: center;
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
    align-items: center;
    gap: 0.25rem;
    margin-left: 0.5rem;
    flex-shrink: 0;
  }
  .select-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
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

  /* Deliberately not red. Stopping a comparison destroys nothing — the
     overlay already on screen survives it — and a warning colour would say
     otherwise. It reads as the plain button it is, in the toolbar row and in
     the footer alike. */
  .stop-btn {
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    color: var(--text);
  }
  .picker-footer .stop-btn {
    padding: 0.5rem 1rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.9rem;
  }
  .stop-btn:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  /* `wait`, not `not-allowed`: the button is disabled because the thing it
     asked for is happening, which is the opposite of a refusal. */
  .stop-btn:disabled {
    opacity: 0.6;
    cursor: wait;
  }
</style>
