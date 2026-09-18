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
  import { focusNode, rawEntityGraph } from '../stores/graph';
  import { isRegionEntry } from '../viewmodels/regionSubject';
  import { NODE_COLORS } from '../types/graph';

  $: chain = $description?.chain ?? [];
  $: children = $description?.children ?? [];
  $: source = $description?.source ?? null;

  /** How many children show before the list is capped. A Feature has a
   *  handful; a File has every method in it, and the pane is a column. */
  const CHILD_PREVIEW = 12;

  /** Which subject the reader expanded, not a bare boolean: hovering the
   *  next node has to collapse the list again, and comparing ids does that
   *  without a second reactive statement to reset the flag. */
  let expandedFor: string | null = null;
  $: subjectId = chain[0]?.entityId ?? null;
  $: showAllChildren = subjectId !== null && subjectId === expandedFor;
  $: shownChildren = showAllChildren ? children : children.slice(0, CHILD_PREVIEW);

  /** Clicking a rung or a child pins it. Moving is the point: the pane is
   *  often the first place a parent — or the functionality under a feature —
   *  becomes visible, and pinning it re-roots the chain there. Entities
   *  outside the loaded graph aren't selectable. */
  function select(entityId: string) {
    const node = $rawEntityGraph.nodes.find((n) => n.original_id === entityId);
    if (node) focusNode(node);
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
      Hover a node in the graph to read its description, and its parents' —
      or a folder's name, for whatever the spec says about it.
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
          {#if isRegionEntry(entry)}
            <!-- A folder answers to nothing in the graph, so it gets no
                 button: an underline on hover would promise a selection this
                 pane cannot make (UI-141). Focusing it is the canvas's
                 gesture — a click on the same name out there. -->
            <span class="name static" title={entry.qualifiedName || entry.name}>{entry.name}</span>
          {:else}
            <button
              type="button"
              class="name"
              title={entry.qualifiedName || entry.name}
              on:click={() => select(entry.entityId)}
            >{entry.name}</button>
          {/if}
        </div>
        <!-- Whose sentence this is, when it isn't the rung's own (UI-141).
             A folder's description belongs to the spec entity that claims it,
             and unattributed it would read as the folder describing itself. -->
        {#if entry.attribution}
          <div class="attribution" data-probe="description-attribution">{entry.attribution}</div>
        {/if}
        {#if entry.documentation}
          <p class="doc">{entry.documentation}</p>
        {:else}
          <p class="doc missing">No description.</p>
        {/if}
        {#if entry.filePath}
          <div class="loc">{entry.filePath}:{entry.line}</div>
        {/if}
      </article>

      <!-- Children hang off the subject only. Deeper in the chain they
           would list the subject's own siblings, which is noise: the
           reader is climbing to find context, not browsing the tree. -->
      {#if i === 0 && children.length > 0}
        <section class="children" data-probe="description-children">
          <div class="children-head">
            <span>contains ({children.length})</span>
            {#if children.length > CHILD_PREVIEW}
              <button
                type="button"
                class="more-btn"
                on:click={() => (expandedFor = showAllChildren ? null : subjectId)}
              >{showAllChildren ? 'Show fewer' : `Show all ${children.length}`}</button>
            {/if}
          </div>
          {#each shownChildren as child (child.entityId)}
            <div class="child">
              <div class="head">
                <span class="kind" style="background: {kindColor(child.kind)}">{child.kind}</span>
                <button
                  type="button"
                  class="name"
                  title={child.qualifiedName || child.name}
                  on:click={() => select(child.entityId)}
                >{child.name}</button>
              </div>
              {#if child.documentation}
                <p class="doc child-doc">{child.documentation}</p>
              {/if}
            </div>
          {/each}
        </section>
      {/if}
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

  /* The children block sits between the subject rung and the first parent
     rung, so it has to carry the separator that `.rung + .rung` would
     otherwise have drawn there. */
  .children + .rung {
    border-top: 1px solid var(--border-subtle);
    padding-top: 10px;
  }

  .children {
    /* Indented and rule-marked so the list reads as "inside the entity
       above" rather than as more rungs of the chain. */
    margin: 0 0 10px 6px;
    padding-left: 8px;
    border-left: 2px solid var(--border-subtle);
  }

  .children-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    font-size: 0.64rem;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-dim);
    margin-bottom: 6px;
  }

  .more-btn {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
    text-transform: none;
    letter-spacing: normal;
    white-space: nowrap;
  }
  .more-btn:hover { text-decoration: underline; }

  .child + .child { margin-top: 6px; }

  /* A child is a lead, not the subject: name at body size, description
     clamped to two lines. Reading all of it is one click away. */
  .child .name { font-size: 0.8rem; font-weight: 500; }
  .child-doc {
    font-size: 0.74rem;
    line-height: 1.45;
    color: var(--text-muted);
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }

  .attribution {
    font-size: 0.68rem;
    color: var(--text-dim);
    margin-bottom: 3px;
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
  .name.static { cursor: default; }
  .name.static:hover { color: var(--text); text-decoration: none; }

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
