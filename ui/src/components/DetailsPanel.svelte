<script lang="ts">
  /**
   * The entity under the pointer, or the one pinned by a click — as its own
   * column.
   *
   * It used to be the bottom half of the left sidebar, where it competed for
   * height with the scope tree and the ranked problem table and had to
   * collapse itself when empty to hand that height back (UI-011). As a column
   * it keeps its width whether or not anything is selected: a pane that
   * appeared and vanished with the hover would relayout the canvas under the
   * cursor, which is worse than an empty pane.
   *
   * One panel for both hover and selection (UI-012). Selection wins when
   * present; otherwise the panel follows the cursor.
   */
  import EntityInfo from './EntityInfo.svelte';
  import ContextScope from './ContextScope.svelte';
  import { selectedNode, hoveredNode, hoverLocked } from '../stores/graph';

  $: detailNode = $selectedNode ?? $hoveredNode;

  /** The pin button already says which mode this is, so the hint next to it
   *  only has to carry what the button can't: whether `L` has frozen the
   *  preview. Three "Pinned"s in a 340px column was two too many. */
  $: modeHint = $selectedNode
    ? 'Showing the pinned entity'
    : $hoverLocked ? 'Hovering — frozen, L to release' : 'Following the cursor';

  /** Pin promotes the hovered node to the selection; unpin clears it and
   *  hands the panel back to the cursor. */
  function togglePin() {
    if ($selectedNode) selectedNode.set(null);
    else if ($hoveredNode) selectedNode.set($hoveredNode);
  }
</script>

<div class="details-pane">
  <header>
    <h2>Details</h2>
  </header>

  {#if detailNode}
    <div class="detail-toolbar">
      <button
        type="button"
        class="pin-btn"
        class:pinned={!!$selectedNode}
        data-probe="pin-toggle"
        aria-pressed={!!$selectedNode}
        title={$selectedNode
          ? 'Unpin — follow the cursor again'
          : 'Pin this entity so hovering elsewhere does not replace it'}
        on:click={togglePin}
      >{$selectedNode ? '\u{1F4CC} Pinned' : '\u{1F4CD} Pin'}</button>
      <span class="detail-mode-hint">{modeHint}</span>
    </div>

    {#if $selectedNode}
      <EntityInfo entity={detailNode} />
      <ContextScope entity={detailNode} />
    {:else}
      <!-- Hover preview stays compact, as the old hover panel did —
           relationships are noise while the cursor is still moving. -->
      <EntityInfo entity={detailNode} showRelationships={false} compact={true} />
    {/if}
  {:else}
    <p class="empty">
      Hover a node to preview it, or click one to pin it here. Press
      <kbd>L</kbd> to freeze the preview so the cursor can leave the canvas.
    </p>
  {/if}
</div>

<style>
  .details-pane {
    height: 100%;
    overflow-y: auto;
    padding: 10px 12px 20px;
    box-sizing: border-box;
    color: var(--text);
  }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 10px;
  }

  h2 {
    margin: 0;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    font-weight: 600;
  }

  .empty {
    color: var(--text-dim);
    font-style: italic;
    font-size: 0.82rem;
    line-height: 1.6;
  }

  kbd {
    font-family: 'Monaco', 'Menlo', monospace;
    font-style: normal;
    font-size: 0.72rem;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--bg-surface-alt);
  }

  .detail-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }

  .pin-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    font: inherit;
    font-size: 0.75rem;
    cursor: pointer;
  }
  .pin-btn:hover { background: var(--bg-hover); color: var(--text); }
  .pin-btn.pinned {
    border-color: var(--accent);
    color: var(--accent);
  }

  .detail-mode-hint {
    font-size: 0.7rem;
    color: var(--text-dim);
  }
</style>
