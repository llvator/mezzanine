<!--
  Saved views, at the top of the Filters pane (UI-082).

  Above the three numbered blocks rather than inside one, because a view
  spans all of them — a scope from block 1, a level and file exclusions from
  block 2, the spec cross-filter and a committed search from block 3. Put it
  in any one block and the section would claim to be about that block's
  controls, which is the one thing it is not.

  The row is a button, and restoring is a single click: the feature's whole
  value is switching between two readings quickly, and a confirm step in
  front of a reversible action would spend exactly what it saves. Deleting
  gets one, because that is the action a stray click cannot undo.
-->
<script lang="ts">
  import {
    savedViews, activeViewId, viewsError, viewsBusy, viewStore, viewStoreLabel,
    saveCurrentView, updateView, renameView, deleteView, restoreView,
    stateSummary, droppedSummary,
  } from '../stores/savedViews';
  import type { SavedView } from '../stores/savedViews';

  let open = true;
  let newName = '';
  /** Id of the row being renamed, and the text in its box. */
  let editingId: string | null = null;
  let editingName = '';
  /** What the last restore had to skip, if anything. Cleared by the next one. */
  let note = '';

  async function save(): Promise<void> {
    if (newName.trim() === '') return;
    const ok = await saveCurrentView(newName);
    if (ok) newName = '';
  }

  async function restore(view: SavedView): Promise<void> {
    note = droppedSummary(await restoreView(view));
  }

  function startRename(view: SavedView): void {
    editingId = view.id;
    editingName = view.name;
  }

  async function commitRename(): Promise<void> {
    if (editingId) await renameView(editingId, editingName);
    editingId = null;
  }

  async function remove(view: SavedView): Promise<void> {
    if (!confirm(`Delete the view “${view.name}”?`)) return;
    await deleteView(view.id);
  }

  /** Local storage is a fallback, not a choice — say so once, where the
   *  reader is about to save something they may expect to find in git. */
  $: whereHint =
    $viewStore === 'local'
      ? 'Kept in this browser — this server has no view store.'
      : `Kept in ${$viewStoreLabel}, beside the code.`;
</script>

<div class="filter-section" data-probe="saved-views">
  <!-- A real <button> styled as the header, like the Parsed Languages
       section: same appearance, keyboard-reachable, no a11y warnings. -->
  <h2 class="section-heading-wrap">
    <button
      type="button"
      class="section-header"
      aria-expanded={open}
      on:click={() => (open = !open)}
    >
      <span>
        Views
        {#if $savedViews.length > 0}<span class="count-badge">{$savedViews.length}</span>{/if}
      </span>
      <span class="toggle-arrow">{open ? '▼' : '▶'}</span>
    </button>
  </h2>

  {#if open}
    <p class="hint">
      Save what the canvas is drawing — scope, level, filters, spec — and come
      back to it in one click. {whereHint}
    </p>

    {#if $viewsError}
      <!-- The one state where saving is refused: the file is there and could
           not be read, so writing would replace a list we never saw. -->
      <p class="error">{$viewsError}</p>
    {/if}

    <div class="save-row">
      <input
        type="text"
        placeholder="Name this view…"
        bind:value={newName}
        disabled={!!$viewsError}
        on:keydown={(e) => { if (e.key === 'Enter') save(); }} />
      <button
        type="button"
        class="save-btn"
        disabled={newName.trim() === '' || $viewsBusy || !!$viewsError}
        on:click={save}
      >Save</button>
    </div>

    {#if note}
      <p class="note">{note}</p>
    {/if}

    {#if $savedViews.length === 0}
      <p class="none">No saved views yet.</p>
    {:else}
      <div class="rows">
        {#each $savedViews as view (view.id)}
          <div class="view-row" class:active={$activeViewId === view.id}>
            {#if editingId === view.id}
              <input
                class="rename"
                type="text"
                bind:value={editingName}
                on:keydown={(e) => {
                  if (e.key === 'Enter') commitRename();
                  if (e.key === 'Escape') editingId = null;
                }}
                on:blur={commitRename} />
            {:else}
              <button
                type="button"
                class="restore"
                title={stateSummary(view.state)}
                on:click={() => restore(view)}
              >
                <span class="dot" aria-hidden="true">{$activeViewId === view.id ? '●' : ''}</span>
                <span class="name">{view.name}</span>
                <span class="summary">{stateSummary(view.state)}</span>
              </button>

              <div class="actions">
                <!-- Update is the answer to "I restored this and then improved
                     it": without it the only way to keep the change is a
                     second view with a nearly identical name. -->
                <button
                  type="button" class="icon" title="Replace with what is on screen now"
                  disabled={$viewsBusy || !!$viewsError}
                  on:click={() => updateView(view.id)}>⟳</button>
                <button
                  type="button" class="icon" title="Rename"
                  disabled={!!$viewsError}
                  on:click={() => startRename(view)}>✎</button>
                <button
                  type="button" class="icon danger" title="Delete"
                  disabled={$viewsBusy || !!$viewsError}
                  on:click={() => remove(view)}>✕</button>
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .section-heading-wrap { margin: 0; }

  .section-header {
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: space-between;
    user-select: none;
    width: 100%;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    text-align: left;
  }

  .toggle-arrow { font-size: 0.7rem; color: var(--text-dim); }

  .filter-section { margin-bottom: 20px; }

  .hint {
    font-size: 0.7rem;
    color: var(--text-muted);
    line-height: 1.45;
    margin: 0 0 8px;
  }

  /* A literal, and the palette has no token for it: the four themes carry
     text/surface/accent and nothing that means "this failed". Same value
     SubmitRepo already uses, rather than a `var(--danger, …)` fallback,
     which would silently pin the component to one theme via a property
     nothing defines. */
  .error {
    font-size: 0.7rem;
    color: #ef9a9a;
    line-height: 1.45;
    margin: 0 0 8px;
  }

  .note {
    font-size: 0.68rem;
    color: var(--text-secondary);
    margin: 0 0 8px;
  }

  .count-badge {
    background: var(--accent);
    color: var(--bg-body);
    border-radius: 8px;
    padding: 0 5px;
    font-size: 0.62rem;
    font-weight: 600;
    margin-left: 4px;
  }

  .save-row {
    display: flex;
    gap: 6px;
    margin-bottom: 8px;
  }

  .save-row input {
    flex: 1;
    min-width: 0;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--bg-body);
    color: var(--text);
    font-size: 0.72rem;
    font-family: inherit;
  }

  .save-row input::placeholder { color: var(--text-dim); }

  .save-btn {
    padding: 3px 10px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 0.72rem;
    font-family: inherit;
    cursor: pointer;
  }

  .save-btn:hover:not(:disabled) { background: var(--bg-hover); }
  .save-btn:disabled { opacity: 0.4; cursor: default; }

  /* Capped like the spec list: a long list of readings must not push the
     analysis controls off the panel. */
  .rows { max-height: 220px; overflow-y: auto; }

  .view-row {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: 3px;
    padding-right: 2px;
  }

  .view-row:hover { background: var(--bg-hover); }

  /* The view on screen. A left rule rather than a fill, so the marker
     survives the hover background instead of competing with it. */
  .view-row.active {
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .restore {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 3px 4px;
    border: none;
    background: none;
    font: inherit;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .dot {
    color: var(--accent);
    font-size: 0.5rem;
    width: 0.6em;
    flex-shrink: 0;
  }

  .name {
    font-size: 0.75rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 0;
    max-width: 55%;
  }

  /* What the view holds, at a glance — the name alone stops being enough at
     about four views. Truncated rather than wrapped: one row per view. */
  .summary {
    font-size: 0.62rem;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    text-align: right;
  }

  .rename {
    flex: 1;
    min-width: 0;
    padding: 2px 4px;
    border: 1px solid var(--accent);
    border-radius: 3px;
    background: var(--bg-body);
    color: var(--text);
    font-size: 0.75rem;
    font-family: inherit;
  }

  .actions { display: flex; gap: 1px; flex-shrink: 0; }

  .icon {
    border: none;
    background: none;
    color: var(--text-dim);
    font-size: 0.7rem;
    line-height: 1;
    padding: 3px 4px;
    cursor: pointer;
    border-radius: 3px;
  }

  .icon:hover:not(:disabled) { color: var(--text); background: var(--bg-surface); }
  .icon.danger:hover:not(:disabled) { color: var(--danger, #d16b6b); }
  .icon:disabled { opacity: 0.35; cursor: default; }

  .none {
    font-size: 0.68rem;
    color: var(--text-dim);
    font-style: italic;
    margin: 2px 0 0;
  }
</style>
