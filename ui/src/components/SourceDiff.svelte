<script lang="ts">
  /**
   * What changed in one entity's code, as a diff rather than as a wall of
   * source.
   *
   * The details pane used to print the head source and a metric delta list,
   * which answers "how much did this move" and never "what moved". This is
   * the same reading a review tool gives: markers and line numbers on both
   * sides, unchanged stretches folded away, and — for a reader who has widened
   * the column — before and after side by side.
   *
   * Add/remove colours are deliberately not theme-derived (see CONTRIBUTING):
   * they identify a state, and a hue that reads as "gone" has to keep reading
   * that way in all four themes.
   */
  import { computeLineDiff, changeCounts, chunkDiff, toSplitRows, type DiffLine } from '../utils/lineDiff';
  import { diffData, diffViewMode, diffFullContext, type ChangeStatus } from '../stores/diff';

  /** Source at the base ref. Absent for an entity that didn't exist there. */
  export let base: string | undefined = undefined;
  /** Source at the head ref. Absent for an entity that's gone. */
  export let head: string | undefined = undefined;
  export let status: ChangeStatus = 'modified';
  export let compact: boolean = false;

  /** A whole entity that arrived or left is still a diff — every line of it.
   *  Rendering it as plain source is what made "added" and "unchanged" look
   *  alike in the pane. */
  function wholeSide(text: string, kind: 'added' | 'removed'): DiffLine[] {
    return text.split('\n').map((t, i) => ({
      kind,
      text: t,
      baseLine: kind === 'removed' ? i + 1 : null,
      headLine: kind === 'added' ? i + 1 : null,
    }));
  }

  $: lines =
    base != null && head != null
      ? computeLineDiff(base, head)
      : head != null
        ? wholeSide(head, status === 'added' ? 'added' : 'removed')
        : base != null
          ? wholeSide(base, 'removed')
          : [];

  $: counts = changeCounts(lines);
  $: chunks = $diffFullContext ? [{ kind: 'lines' as const, lines }] : chunkDiff(lines);
  $: collapsible = chunks.some((c) => c.kind === 'gap') || $diffFullContext;
  $: fromLabel = $diffData?.from_ref ?? 'before';
  $: toLabel = $diffData?.to_ref ?? 'after';
</script>

<div class="diff-view" class:compact data-probe="source-diff" data-diff-mode={$diffViewMode}>
  <div class="diff-toolbar">
    <span class="counts">
      <span class="add">+{counts.added}</span>
      <span class="del">−{counts.removed}</span>
    </span>
    <span class="modes">
      <button
        type="button" class:on={$diffViewMode === 'unified'} data-probe="diff-mode-unified"
        title="One column, changes marked + and −"
        on:click={() => diffViewMode.set('unified')}
      >Unified</button>
      <button
        type="button" class:on={$diffViewMode === 'split'} data-probe="diff-mode-split"
        title="Before and after side by side — wants a wide pane"
        on:click={() => diffViewMode.set('split')}
      >Split</button>
    </span>
    {#if collapsible}
      <button
        type="button" class="ctx-btn" data-probe="diff-context-toggle"
        title={$diffFullContext ? 'Fold the unchanged stretches away' : 'Show every line, changed or not'}
        on:click={() => diffFullContext.update((v) => !v)}
      >{$diffFullContext ? 'Changes only' : 'Whole entity'}</button>
    {/if}
  </div>

  <div class="diff-lines" class:split={$diffViewMode === 'split'}>
    <!-- The ref labels sit inside the grid, not above it: a header laid out
         separately drifts off the columns the moment they size to content. -->
    {#if $diffViewMode === 'split'}
      <span class="side-label removed">{fromLabel}</span>
      <span class="side-label added">{toLabel}</span>
    {/if}
    {#each chunks as chunk}
      {#if chunk.kind === 'gap'}
        <button
          type="button" class="gap" data-probe="diff-gap"
          title="Show every line"
          on:click={() => diffFullContext.set(true)}
        >⋯ {chunk.hidden} unchanged {chunk.hidden === 1 ? 'line' : 'lines'}</button>
      {:else if $diffViewMode === 'unified'}
        {#each chunk.lines as line}
          <div class="diff-line diff-line-{line.kind}">
            <span class="gutter">{line.baseLine ?? ''}</span>
            <span class="gutter">{line.headLine ?? ''}</span>
            <span class="marker">{line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}</span>
            <pre class="text">{line.text}</pre>
          </div>
        {/each}
      {:else}
        {#each toSplitRows(chunk.lines) as row}
          <div class="split-row">
            <div class="cell diff-line-{row.base?.kind ?? 'empty'}">
              <span class="gutter">{row.base?.baseLine ?? ''}</span>
              <pre class="text">{row.base?.text ?? ''}</pre>
            </div>
            <div class="cell diff-line-{row.head?.kind ?? 'empty'}">
              <span class="gutter">{row.head?.headLine ?? ''}</span>
              <pre class="text">{row.head?.text ?? ''}</pre>
            </div>
          </div>
        {/each}
      {/if}
    {/each}
  </div>
</div>

<style>
  .diff-view {
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-top: 8px;
    overflow-x: auto;
  }
  .diff-view.compact { max-height: 300px; overflow-y: auto; }

  .diff-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    background: var(--bg-surface-alt);
    z-index: 1;
  }
  .counts {
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.7rem;
    display: flex;
    gap: 6px;
  }
  .counts .add { color: #A5D6A7; }
  .counts .del { color: #EF9A9A; }

  .modes { display: flex; margin-left: auto; }
  .modes button {
    font: inherit;
    font-size: 0.68rem;
    padding: 2px 8px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .modes button:first-child { border-radius: 10px 0 0 10px; }
  .modes button:last-child { border-radius: 0 10px 10px 0; border-left: none; }
  .modes button:hover { color: var(--text); background: var(--bg-hover); }
  .modes button.on { color: var(--accent); border-color: var(--accent); }

  .ctx-btn {
    font: inherit;
    font-size: 0.68rem;
    padding: 2px 8px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .ctx-btn:hover { color: var(--text); background: var(--bg-hover); }

  .side-label {
    font-size: 0.65rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 3px 8px;
    text-align: center;
    border-bottom: 1px solid var(--border);
  }
  .side-label.removed { color: #EF9A9A; }
  .side-label.added { color: #A5D6A7; }

  .diff-lines {
    font-family: 'Monaco', 'Menlo', 'Consolas', monospace;
    font-size: 0.75rem;
    line-height: 1.5;
  }

  .diff-line { display: flex; align-items: stretch; min-height: 1.5em; }

  /* Split is one grid over the whole body rather than a flex row per line,
     so the two columns stay the same width down the whole diff and the
     divider is a straight line rather than a per-row accident.

     Two failed shapes are worth not repeating: per-row flex let a long line
     run straight over the line beside it, and `max-content` columns made the
     before side as wide as its longest line, which pushes the after side off
     a 340px pane entirely — a split view showing one side. Equal columns
     with wrapped text is the version that fits the pane it lives in; the
     unified view is the one that keeps lines intact. */
  .diff-lines.split {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    align-items: stretch;
  }
  .diff-lines.split .split-row { display: contents; }
  .split-row .cell { display: flex; min-height: 1.5em; min-width: 0; }
  .split-row .cell:first-child { border-right: 1px solid var(--border); }
  .diff-lines.split .text { white-space: pre-wrap; overflow-wrap: anywhere; }
  .diff-lines.split .gap { grid-column: 1 / -1; }

  .diff-line-equal { color: var(--text-dim); }
  .diff-line-added { background: rgba(76, 175, 80, 0.1); color: #A5D6A7; }
  .diff-line-removed { background: rgba(244, 67, 54, 0.1); color: #EF9A9A; }
  /* Padding opposite an insertion or deletion — no line exists here. */
  .diff-line-empty { background: var(--bg-surface-alt); }

  .gutter {
    flex: 0 0 auto;
    min-width: 2.2em;
    padding: 0 4px;
    text-align: right;
    color: var(--text-disabled);
    user-select: none;
    font-size: 0.68rem;
  }
  .marker {
    flex: 0 0 auto;
    width: 1em;
    text-align: center;
    user-select: none;
  }
  .text {
    margin: 0;
    padding: 0 6px 0 2px;
    white-space: pre;
    font: inherit;
    background: none;
    border: none;
    flex: 1 1 auto;
    min-width: 0;
  }

  .gap {
    display: block;
    width: 100%;
    font: inherit;
    font-size: 0.68rem;
    text-align: center;
    padding: 2px 0;
    border: none;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-dim);
    cursor: pointer;
  }
  .gap:hover { color: var(--text-muted); }
</style>
