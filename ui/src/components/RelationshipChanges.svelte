<script lang="ts">
  /**
   * The edges this entity gained and lost between the two refs.
   *
   * The Relationships list below it draws the head graph, so a relationship
   * that disappeared is nowhere in it — the one change a reader of a diff most
   * wants to be told about was the one the pane could not show. These rows are
   * the diff's own `rel_deltas`, which is also why they catch the swap a
   * metric delta can't: dropping one call and adding another leaves `fan_out`
   * exactly where it was.
   *
   * Kept as its own list rather than badges on the list below: a removed edge
   * has no row there to badge, and splitting the same information across two
   * conventions would make the reader check both.
   */
  import ColorChip from './ColorChip.svelte';
  import { LINK_COLORS, type D3Node } from '../types/graph';
  import { graphData, focusNode } from '../stores/graph';
  import { normalizeEntityId, type RelationshipDelta } from '../stores/diff';
  import { relKindRaw } from '../transform';

  export let deltas: RelationshipDelta[] = [];

  $: appeared = deltas.filter((d) => d.status === 'added');
  $: disappeared = deltas.filter((d) => d.status === 'removed');

  /** Normalized backend ID → the node on the canvas, so a far end that is
   *  still drawn can be reached in one click. Built once per graph rather
   *  than scanned per row. */
  $: nodeById = new Map<string, D3Node>(
    $graphData.nodes.map((n) => [normalizeEntityId(n.original_id), n]),
  );

  /**
   * Where clicking the row goes: the far end itself, or — when the canvas is
   * drawing files or folders rather than entities — the scope holding it.
   *
   * The fallback is most of the value. A collapsed canvas has no node for
   * `attach_edge` at all, and without it every row on a file-level view is
   * dead, which is the view a diff is usually read in. It also gives a
   * *removed* entity somewhere to point: the file it used to live in.
   *
   * Still null for a far end the scope excludes entirely. The row stays —
   * "it no longer calls X" is true whether or not X is on screen.
   */
  function target(d: RelationshipDelta): D3Node | null {
    const byId = d.other_entity_id ? nodeById.get(normalizeEntityId(d.other_entity_id)) : undefined;
    return byId ?? nodeById.get(normalizeEntityId(d.other_file)) ?? null;
  }
</script>

{#if deltas.length > 0}
  <div class="detail-row">
    <div class="detail-label" title="Relationships this entity gained or lost between the two refs">
      Relationship changes
      <span class="rc-counts">
        <span class="rc-add">+{appeared.length}</span>
        <span class="rc-del">−{disappeared.length}</span>
      </span>
    </div>
  </div>
  <div class="rc-list" data-probe="rel-changes">
    {#each [...appeared, ...disappeared] as d}
      {@const node = target(d)}
      <button
        type="button"
        class="rc-item rc-{d.status}"
        class:clickable={!!node}
        data-probe="rel-change"
        disabled={!node}
        title={node
          ? `Show ${d.other_name} — ${d.status === 'added' ? 'now' : 'no longer'} ${d.label}`
          : `${d.other_name} (${d.other_kind}) in ${d.other_file} — not on the canvas`}
        on:click={() => node && focusNode(node)}
      >
        <span class="rc-sign">{d.status === 'added' ? '+' : '−'}</span>
        <!-- Plain arrows: the panel's own `&#11136;` renders as a box in the
             pane's monospace stack, and the label beside this already states
             the direction, so the glyph only has to be legible. -->
        <span class="rc-dir">{d.direction === 'outgoing' ? '→' : '←'}</span>
        <span class="rc-kind">
          <ColorChip color={LINK_COLORS[relKindRaw(d.kind)] || 'var(--text-dim)'} label={d.label} dim />
        </span>
        <span class="rc-name">{d.other_name}</span>
        <span class="rc-where">{d.other_kind}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  /* The panel's own row conventions, so this block reads as part of the
     Details pane rather than as a widget dropped into it. */
  .detail-row { margin-bottom: 6px; }
  .detail-label {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-dim);
    margin-bottom: 2px;
    display: flex;
    align-items: baseline;
    gap: 6px;
  }
  .rc-counts {
    font-family: 'Monaco', 'Menlo', monospace;
    letter-spacing: 0;
    display: flex;
    gap: 5px;
  }
  /* Add/remove hues are identity, not theme (see CONTRIBUTING). */
  .rc-add { color: #A5D6A7; }
  .rc-del { color: #EF9A9A; }

  .rc-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 8px;
  }

  .rc-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    text-align: left;
    font: inherit;
    font-size: 0.78rem;
    padding: 2px 6px;
    border: none;
    border-left: 2px solid transparent;
    border-radius: 3px;
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
  }
  .rc-item.clickable { cursor: pointer; }
  .rc-item.clickable:hover { background: var(--bg-hover); color: var(--text); }
  .rc-added { border-left-color: #4CAF50; }
  .rc-removed { border-left-color: #F44336; }
  /* A relationship that is gone: struck through, like the code line it went
     with, so status survives being read at a glance. */
  .rc-removed .rc-name { text-decoration: line-through; }

  .rc-sign {
    font-family: 'Monaco', 'Menlo', monospace;
    font-weight: 700;
    width: 0.8em;
  }
  .rc-added .rc-sign { color: #A5D6A7; }
  .rc-removed .rc-sign { color: #EF9A9A; }

  .rc-dir { color: var(--text-muted); font-size: 0.7rem; }
  .rc-kind { flex: 0 0 auto; }
  .rc-name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rc-where {
    flex: 0 0 auto;
    font-size: 0.68rem;
    color: var(--text-dim);
  }
</style>
