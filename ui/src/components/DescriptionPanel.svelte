<script lang="ts">
  /**
   * The graph read as prose — the standalone twin of the extension's native
   * "Description" view.
   *
   * Not a second details panel (UI-012 removed one of those): it carries no
   * metrics, no source, no fields. Just the hovered node's description
   * followed by its ancestors', so skimming the canvas narrates the graph.
   */
  import { description, describeOnHover } from '../stores/description';
  import { selectedNode, rawEntityGraph } from '../stores/graph';
  import { NODE_COLORS } from '../types/graph';

  $: chain = $description?.chain ?? [];
  $: source = $description?.source ?? null;

  /** Clicking a rung pins it. Climbing is the point: the pane is often the
   *  first place a parent becomes visible, and pinning it re-roots the
   *  chain there. Ancestors outside the loaded graph aren't selectable. */
  function select(entityId: string) {
    const node = $rawEntityGraph.nodes.find((n) => n.original_id === entityId);
    if (node) selectedNode.set(node);
  }

  function kindColor(kind: string): string {
    return NODE_COLORS[kind] ?? NODE_COLORS.Unknown;
  }
</script>

<div class="description-panel">
  <header>
    <h2>Description</h2>
    <label class="hover-toggle" title="When on, the pane follows the pointer. Off, it follows the selection only.">
      <input type="checkbox" bind:checked={$describeOnHover} />
      on hover
    </label>
  </header>

  {#if chain.length === 0}
    <p class="empty">
      Hover a node in the graph to read its description, and its parents'.
    </p>
  {:else}
    <div class="source-badge" class:hovering={source === 'hover'}>
      {source === 'hover' ? '● Hovering' : '● Selected'}
    </div>

    {#each chain as entry, i (entry.entityId)}
      <article class="rung" style="--rung-fade: {Math.max(0.6, 1 - entry.depth * 0.12)}">
        {#if i > 0}
          <div class="parent-of">parent</div>
        {/if}
        <div class="head">
          <span class="kind" style="background: {kindColor(entry.kind)}">{entry.kind}</span>
          <button
            type="button"
            class="name"
            title={entry.qualifiedName || entry.name}
            on:click={() => select(entry.entityId)}
          >{entry.name}</button>
        </div>
        {#if entry.documentation}
          <p class="doc">{entry.documentation}</p>
        {:else}
          <p class="doc missing">No description.</p>
        {/if}
        {#if entry.filePath}
          <div class="loc">{entry.filePath}:{entry.line}</div>
        {/if}
      </article>
    {/each}
  {/if}
</div>

<style>
  .description-panel {
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

  .hover-toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 0.72rem;
    color: var(--text-dim);
    cursor: pointer;
    white-space: nowrap;
  }
  .hover-toggle input { margin: 0; cursor: pointer; }

  .empty {
    color: var(--text-dim);
    font-style: italic;
    font-size: 0.82rem;
    line-height: 1.5;
  }

  .source-badge {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
    margin-bottom: 8px;
  }
  .source-badge.hovering { color: var(--accent); }

  /* Each rung fades as the chain climbs, so the node under the pointer
     stays the subject and its ancestors read as context. */
  .rung { opacity: var(--rung-fade, 1); padding-bottom: 10px; }
  .rung + .rung {
    border-top: 1px solid var(--border-subtle);
    padding-top: 10px;
  }

  .parent-of {
    font-size: 0.64rem;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-dim);
    margin-bottom: 4px;
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 4px;
  }

  .kind {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 5px;
    border-radius: 2px;
    color: #fff;
    text-shadow: 0 1px 1px rgba(0, 0, 0, 0.45);
    flex-shrink: 0;
  }

  .name {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-weight: 600;
    font-size: 0.88rem;
    color: var(--text);
    cursor: pointer;
    text-align: left;
  }
  .name:hover { color: var(--accent); text-decoration: underline; }

  .doc {
    margin: 0;
    font-size: 0.8rem;
    line-height: 1.55;
    color: var(--text-secondary);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .doc.missing { color: var(--text-disabled); font-style: italic; }

  .loc {
    margin-top: 3px;
    font-size: 0.68rem;
    color: var(--text-dim);
    word-break: break-all;
  }
</style>
