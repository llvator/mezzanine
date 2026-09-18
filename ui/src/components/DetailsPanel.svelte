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
   * present; otherwise the panel follows the cursor — over a node, and since
   * UI-141 over a region's name too, which is the only folder on the canvas a
   * reader can point at.
   *
   * A region can be pinned here as well as pointed at since UI-148, so the
   * four candidates are ranked in one place — `detailSubject`, which is pure
   * and tested (`npm run test:region-subject`) because "what is this column
   * about" is the question every other rule here is a special case of.
   */
  import EntityInfo from './EntityInfo.svelte';
  import ContextScope from './ContextScope.svelte';
  import FileDiff from './FileDiff.svelte';
  import FolderInfo from './FolderInfo.svelte';
  import RelationInfo from './RelationInfo.svelte';
  import { selectedNode, hoveredNode, hoverLocked } from '../stores/graph';
  import { hoveredRegion, selectedRegion, unpinRegion } from '../stores/region';
  import { selectedChangedFile } from '../stores/changedFiles';
  import { relation } from '../stores/relate';
  import { detailSubject } from '../viewmodels/regionSubject';
  import type { D3Node } from '../types/graph';

  $: subject = detailSubject<D3Node>({
    selectedNode: $selectedNode,
    selectedRegion: $selectedRegion,
    hoveredNode: $hoveredNode,
    hoveredRegion: $hoveredRegion,
  });

  $: detailNode = subject?.kind === 'node' ? subject.node : null;
  /** A folder is a subject too (UI-141), and since UI-148 one a click can
   *  hold here rather than only point at. */
  $: regionSubject = subject?.kind === 'region' ? subject.region : null;

  /**
   * A file opened from the Changes tab wins the pane (UI-134).
   *
   * It is the more specific request: the row was clicked *here*, and for a
   * file outside the analysis there is no node that could show it at all.
   * Selecting any other node clears it — that rule lives in
   * `stores/changedFiles`, so it holds however the node was selected.
   */
  $: fileReading = $selectedChangedFile;

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

<!-- `file-mode` hands the pane's height to the file diff (UI-140). Only in that
     mode: everything else here is a stack of short sections that reads better
     scrolling as one block, which is what the pane has always done. -->
<div class="details-pane" class:file-mode={!!fileReading}>
  <header>
    <h2>Details</h2>
  </header>

  {#if $relation}
    <!-- The marked set outranks everything, including a pinned entity
         (UI-147). It is the only subject here the reader asked for by pressing
         a button rather than by pointing at something, it says so with a Close
         of its own, and the alternative — losing the reading to whichever node
         the cursor crossed on its way to this column — is the same failure the
         pin exists to prevent. It clears itself the moment fewer than two
         scopes are marked, so there is no way to get stuck behind it. -->
    <RelationInfo />
  {:else if fileReading}
    <FileDiff file={fileReading} />
  {:else if detailNode}
    <div class="detail-toolbar">
      <button
        type="button"
        class="pin-btn"
        class:pinned={!!$selectedNode}
        data-probe="pin-toggle"
        aria-pressed={!!$selectedNode}
        title={$selectedNode
          ? 'Unpin — follow the cursor again (P)'
          : 'Pin this entity so hovering elsewhere does not replace it (P)'}
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
  {:else if regionSubject}
    <!-- Pinned and hovering are different states with different exits, and
         the toolbar has to say which one this is. Pinned came from a click on
         the name and lasts until it is released; hovering lasts as long as the
         pointer stays, which `L` is what suspends. -->
    <div class="detail-toolbar">
      {#if $selectedRegion}
        <button
          type="button"
          class="pin-btn pinned"
          data-probe="region-pin-toggle"
          aria-pressed="true"
          title="Unpin — follow the cursor again"
          on:click={unpinRegion}
        >{'\u{1F4CC} Pinned'}</button>
        <span class="detail-mode-hint" data-probe="region-mode-hint">
          Showing the pinned {regionSubject.grain}
        </span>
      {:else}
        <!-- No pin button in this branch: there is nothing to toggle off, and
             the gesture that would turn it on lives on the canvas — clicking
             the name, which also focuses the region. A button here that only
             pinned would be a second, quieter meaning for the same word. -->
        <span class="detail-mode-hint" data-probe="region-mode-hint">
          Hovering a {regionSubject.grain} —
          {$hoverLocked ? 'frozen, L to release' : 'press L to freeze'}
        </span>
      {/if}
    </div>
    <FolderInfo region={regionSubject} />
  {:else}
    <p class="empty">
      Hover a node to preview it, or click one to pin it here. Hovering a
      file or folder's name on the canvas previews it; clicking that name
      focuses it and pins it here. Press
      <kbd>L</kbd> to freeze a preview so the cursor can leave the canvas.
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

  /* The pane stops scrolling and becomes the frame; the diff scrolls inside
     it. Two scrollbars for one document is the thing this avoids — the outer
     one moved the header off-screen while the inner one still held most of
     the file. The bottom padding goes too: it is breathing room under a stack
     that ends, and here the content is meant to reach the edge. */
  .details-pane.file-mode {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    padding-bottom: 10px;
  }
  .details-pane.file-mode > :global(.file-diff) { min-height: 0; }

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
