<script lang="ts">
  /**
   * One changed file, both sides, from git (UI-134).
   *
   * The Details pane already diffs an *entity* — and a collapsed File node,
   * through the detail sidecar. This is the same reading for a file the
   * sidecar cannot answer for: anything outside the analysis, and anything
   * deleted. Asking git for both sides rather than reusing the sidecar is what
   * keeps the changed-files pane a check on the analysis instead of a second
   * view of it.
   *
   * The rendering is `SourceDiff`, unchanged — so the unified/split toggle,
   * the folded gaps and the whole-file switch are the ones the reader already
   * knows.
   */
  import SourceDiff from './SourceDiff.svelte';
  import { exceedsLcsBudget } from '../utils/lineDiff';
  import { loadFileDiff, selectedChangedFile, type ChangedFile, type FileDiffContent } from '../stores/changedFiles';
  import { statusChange, statusLetter, statusPhrase, splitPath } from '../viewmodels/changedFiles';

  export let file: ChangedFile;

  /** Re-fetched per file rather than cached: a working-tree head moves under
   *  the reader, and a stale side would render as a diff of a save ago. */
  $: content = loadFileDiff(file);
  $: parts = splitPath(file.path);

  /** A whole file can be too dissimilar to align line by line, and saying so
   *  beats presenting the fallback — one block replaced by another — as though
   *  it were the alignment. The same note `EntityInfo` shows for an entity. */
  function degraded(c: FileDiffContent): boolean {
    return !!c.base && !!c.head && exceedsLcsBudget(c.base.text, c.head.text);
  }
</script>

<div class="file-diff" data-probe="file-diff">
  <header>
    <div class="title">
      <span class="letter" data-status={statusLetter(file)}>{statusLetter(file)}</span>
      <span class="name">{parts.name}</span>
    </div>
    <button
      type="button"
      class="close"
      title="Back to the selected entity"
      on:click={() => selectedChangedFile.set(null)}
    >×</button>
  </header>
  <p class="path" title={file.path}>{file.path}</p>
  <p class="what">
    {statusPhrase(file)}
    {#if file.old_path}<br />was {file.old_path}{/if}
  </p>

  {#await content}
    <p class="note">Reading both sides…</p>
  {:then c}
    {#if c.binary}
      <p class="note">
        Binary file — git counts no lines here, and there is nothing to show
        side by side.
      </p>
    {:else if !c.base && !c.head}
      <p class="note">Neither side of this file could be read.</p>
    {:else}
      {#if c.base?.truncated || c.head?.truncated}
        <p class="note">Over 256 KB — shown up to the cut.</p>
      {/if}
      {#if degraded(c)}
        <p class="note">
          Too dissimilar to align line by line: shown as one block replaced by
          another.
        </p>
      {/if}
      <!-- Not `compact` (UI-140). That flag caps SourceDiff at 300px, which is
           right where it was written for — a per-entity diff sharing the pane
           with metrics, relationships and a description, none of which should
           be pushed off. Here the diff IS the pane, and a whole file is the
           one thing in the app most likely to be longer than 300px, so the cap
           left most of the pane empty and most of the file behind a second
           scrollbar. The scrolling moves out here instead, to the one box that
           knows how much room there actually is. -->
      <div class="diff-scroll">
        <SourceDiff
          base={c.base?.text}
          head={c.head?.text}
          status={statusChange(file.status)}
        />
      </div>
    {/if}
  {:catch err}
    <p class="note">Could not read this file: {err.message}</p>
  {/await}
</div>

<style>
  /* Fills the pane rather than the text. `min-height: 0` on both this and the
     scroller is the load-bearing half: a flex child's default `min-height:
     auto` refuses to shrink below its content, so without it the diff grows
     the column instead of scrolling inside it and the cap comes back wearing
     a different hat. */
  .file-diff {
    display: flex;
    flex-direction: column;
    gap: 4px;
    height: 100%;
    min-height: 0;
  }

  .diff-scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  /* Everything above the diff keeps its own height. A flex child shrinks by
     default, and these are the parts that say *which* file is on screen —
     the first thing a long diff would otherwise squeeze out. */
  header, .path, .what, .note { flex: none; }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .title {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }
  .name {
    font-size: 0.9rem;
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* State colours, not theme colours — the same three hues the canvas
     draws a diff in (CONTRIBUTING). */
  .letter {
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.75rem;
    font-weight: 700;
  }
  .letter[data-status='A'] { color: #4CAF50; }
  .letter[data-status='D'] { color: #F44336; }
  .letter:not([data-status='A']):not([data-status='D']) { color: #FFA726; }

  .close {
    border: none;
    background: transparent;
    color: var(--text-dim);
    font-size: 1rem;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }
  .close:hover { color: var(--text); }

  .path {
    margin: 0;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.68rem;
    color: var(--text-dim);
    overflow-wrap: anywhere;
  }
  .what {
    margin: 0 0 4px;
    font-size: 0.72rem;
    color: var(--text-muted);
  }
  .note {
    margin: 0 0 6px;
    font-size: 0.72rem;
    color: var(--text-dim);
    line-height: 1.5;
  }
</style>
