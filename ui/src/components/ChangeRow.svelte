<script lang="ts">
  /**
   * One file in the change, as both readings of the list draw it (UI-154).
   *
   * Split out of `ChangedFiles.svelte` when the folder tree arrived: the flat
   * list and the tree are two arrangements of the same row, and a row defined
   * twice is a row whose two copies drift — one gains the rename tooltip, the
   * other keeps lighting the canvas from a stale map. What differs between the
   * two is the folder column and the indent, so those are props and everything
   * else is here once.
   */
  import { DIFF_COLORS } from '../stores/diff';
  import {
    hoveredChangedFile, openChangedFile, selectedChangedFile,
  } from '../stores/changedFiles';
  import {
    agreementHint, agreementLabel, splitPath, statusChange, statusLetter, statusPhrase,
    type ChangedFileRow,
  } from '../viewmodels/changedFiles';

  export let row: ChangedFileRow;
  /** The dimmed folder column. Off in the tree, where the folder is a row of
   *  its own and repeating it on every leaf would say the same thing twice. */
  export let showDir = true;
  /** How far the tree has pushed this row in, in pixels. */
  export let indent = 0;

  $: file = row.file;
  $: parts = splitPath(file.path);

  /** Light a row's file while the pointer — or the keyboard focus — is on it.
   *  Both, because a control reachable only under a pointer is one a keyboard
   *  reader never finds. */
  function hover(path: string | null) {
    hoveredChangedFile.set(path);
  }
</script>

<button
  type="button"
  class="row"
  class:selected={$selectedChangedFile?.path === file.path}
  style="padding-left: {4 + indent}px"
  data-probe="changed-file"
  data-status={statusLetter(file)}
  title={`${statusPhrase(file)} — ${file.path}${file.old_path ? `\nwas ${file.old_path}` : ''}\n\n${agreementHint(row.agreement)}`}
  on:click={() => openChangedFile(file)}
  on:mouseenter={() => hover(file.path)}
  on:mouseleave={() => hover(null)}
  on:focus={() => hover(file.path)}
  on:blur={() => hover(null)}
>
  <span
    class="letter"
    style="color: {DIFF_COLORS[statusChange(file.status)]}"
    aria-label={statusPhrase(file)}
  >{statusLetter(file)}</span>
  <span class="name">{parts.name}</span>
  <!-- The LRM is load-bearing. The column is `direction: rtl` so a path too
       long for it loses its *head* rather than its tail, and in an RTL run a
       leading `.` is neutral: `.mezz` rendered as `mezz.`. A strong LTR mark
       in front pins the run. -->
  <span class="dir">{showDir ? '\u200E' + parts.dir : ''}</span>
  <span class="churn">
    {#if file.binary}
      <span class="binary">binary</span>
    {:else}
      <span class="add">+{file.additions}</span>
      <span class="del">−{file.deletions}</span>
    {/if}
  </span>
  <span class="agreement agreement-{row.agreement.kind}">
    {agreementLabel(row.agreement)}
  </span>
</button>

<style>
  .row {
    width: 100%;
    display: grid;
    grid-template-columns: 12px minmax(0, auto) minmax(0, 1fr) auto;
    grid-template-areas:
      'letter name dir churn'
      'letter agreement agreement agreement';
    align-items: baseline;
    column-gap: 6px;
    padding: 3px 4px;
    border: none;
    border-radius: 3px;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 0.78rem;
    text-align: left;
    cursor: pointer;
  }
  .row:hover { background: var(--bg-hover); }
  .row.selected { background: var(--bg-surface-alt); box-shadow: inset 2px 0 0 var(--accent); }

  .letter {
    grid-area: letter;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.72rem;
    font-weight: 700;
  }
  .name {
    grid-area: name;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dir {
    grid-area: dir;
    color: var(--text-dim);
    font-size: 0.7rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;      /* keep the tail of a deep path, not its head */
    text-align: left;
  }
  .churn {
    grid-area: churn;
    display: flex;
    gap: 5px;
    font-size: 0.7rem;
    font-family: 'Monaco', 'Menlo', monospace;
  }

  /* Add/remove hues identify a state and stay put across themes — the same
     rule SourceDiff follows, and the reason these are literals. */
  .add { color: #4CAF50; }
  .del { color: #F44336; }
  .binary { color: var(--text-dim); font-style: italic; }

  .agreement {
    grid-area: agreement;
    font-size: 0.68rem;
    color: var(--text-dim);
  }
  /* The two rows that say something a reader should act on. `drawn` is the
     ordinary case and stays quiet. */
  .agreement-unanalysed { color: var(--text-muted); font-style: italic; }
  .agreement-silent { color: var(--text-secondary); }
</style>
