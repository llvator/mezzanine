<!--
  The Spec filter, as a pick list, in the Filters pane's Post-Filtering block.

  The second surface for the same filter the spec pane drives, and the reason
  the filter no longer dies when that pane is collapsed. Collapsing a pane is a
  request for canvas room; it was never a request to stop filtering, but while
  the pane held the only control, a filter that outlived it would have been
  unreachable — so the filter was cancelled instead. With a control here, the
  filter can simply persist.

  The two surfaces are not redundant. The pane is spatial and answers "what is
  near this, and what does it contain"; this is a list and answers "check these
  four Features wherever they live". Only the list can express a selection that
  spans branches, which is why a click there replaces the selection and a
  checkbox here adds to it.
-->
<script lang="ts">
  import { KIND_CODES, NODE_COLORS } from '../types/graph';
  import ColorChip from './ColorChip.svelte';
  import {
    specGraph, visibleSpecGraph, specSelection, specScopeState,
    specPinnedHighlight, clearPinnedHighlight, togglePinnedHighlight,
    toggleSpecSelection, clearSpecFocus, setSpecSelection,
  } from '../stores/crossFilter';
  import { matchesSpecQuery, specOptions } from '../viewmodels/specGraph';
  import { splitViewOpen, followAnalysisScope } from '../stores/panes';

  let open = false;

  /**
   * Whether each tier offers only children of what is already selected.
   *
   * Off by default: the list's advantage over the pane is that it can reach an
   * entity without knowing which branch it sits in, and drill-down mode trades
   * exactly that away. On, it becomes the pane's progressive disclosure in list
   * form, which is the better shape when you are exploring rather than looking
   * for a name you already know.
   */
  let drillDown = false;

  /**
   * One filter string per tier, keyed by tier number.
   *
   * Reassigned rather than mutated in place on every keystroke: `queries[k] =
   * v` on a plain object does not invalidate in Svelte's legacy reactivity, so
   * the list would keep rendering the previous query's matches.
   */
  let queries: Record<number, string> = {};

  function setQuery(tier: number, value: string): void {
    queries = { ...queries, [tier]: value };
  }

  $: tiers = specOptions($visibleSpecGraph, $specSelection, drillDown).map((tier) => ({
    ...tier,
    shown: tier.nodes.filter((n) => matchesSpecQuery(n, queries[tier.tier] ?? '')),
  }));
  $: selectedCount = $specSelection.size;

  /** Marked entities claim nothing reachable, so checking one filters the
   *  canvas to nothing. Shown rather than hidden — the row is how you find
   *  out a Feature has no `cr:` — but flagged, so the empty result is not a
   *  surprise. See `specScopeState`. */
  function markFor(id: string): string {
    const state = $specScopeState.get(id);
    if (!state || state === 'in-scope') return '';
    if (state === 'out-of-scope') return 'out of the analysis scope';
    if (state === 'unanalyzed') return 'no analysed code at its cr: paths — drift, or an unparsed file type';
    return 'no cr: — claims no code';
  }
</script>

{#if !$specGraph.empty}
  <div class="filter-section" data-probe="spec-filter">
    <h2 class="section-header" on:click={() => (open = !open)}>
      Spec
      {#if selectedCount > 0}
        <span class="count-badge">{selectedCount}</span>
      {/if}
      <span class="toggle-arrow">{open ? '▼' : '▶'}</span>
    </h2>

    {#if open}
      <p class="hint">
        Narrow the canvas to the code an Elevator entity declares with
        <code>cr:</code>. Checking several shows the union of what they claim.
      </p>

      <div class="controls">
        <label class="opt" title="Offer only entities whose parent is already checked">
          <input type="checkbox" bind:checked={drillDown} />
          <span>Drill down</span>
        </label>
        <label class="opt" title="Offer only entities whose cr: reaches code the analysis scope loaded">
          <input type="checkbox" bind:checked={$followAnalysisScope} />
          <span>Follow scope</span>
        </label>
        {#if selectedCount > 0}
          <button type="button" class="link-btn" on:click={clearSpecFocus}>Clear</button>
        {/if}
      </div>

      {#if !$splitViewOpen && selectedCount > 0}
        <!-- The state this section exists to make survivable: a filter running
             with its pane collapsed. Say so, and offer the pane back. -->
        <p class="note">
          Filtering with the spec pane closed.
          <button type="button" class="link-btn" on:click={() => splitViewOpen.set(true)}>
            Show the pane
          </button>
        </p>
      {/if}

      {#if $specPinnedHighlight.size > 0}
        <!-- Pins are made in the pane and outlive it, so the same argument
             that put the filter's control here applies to them: rings on the
             canvas whose only explanation was a pane the reader has since
             collapsed. Shown whether or not the pane is open — unlike the
             filter, a pin has no row in the lists below to find it by. -->
        <p class="note">
          <strong>{$specPinnedHighlight.size}</strong> pinned, lighting their code on the graph.
          <button type="button" class="link-btn" on:click={clearPinnedHighlight}>Unpin</button>
        </p>
      {/if}

      {#if $visibleSpecGraph.empty}
        <p class="hint">
          Nothing reaches the analysis scope. Untick <em>Follow scope</em>, or widen it.
        </p>
      {:else}
        {#each tiers as tier (tier.tier)}
          <div class="tier">
            <div class="sub-title">
              <ColorChip color={NODE_COLORS[tier.kind] ?? 'var(--text-muted)'} label={tier.kind} />
              <span class="tier-count">
                {#if tier.shown.length !== tier.nodes.length}{tier.shown.length}/{/if}{tier.nodes.length}
              </span>
              <button
                type="button"
                class="link-btn only"
                title="Select exactly the rows listed here"
                on:click={() => setSpecSelection(tier.shown.map((n) => n.id))}
              >only</button>
            </div>

            <!-- One box per tier rather than one for the section: the lists are
                 independent and a reader narrowing Features has no reason to
                 lose sight of which Categories are checked. -->
            <input
              class="tier-filter"
              type="text"
              placeholder="Filter {tier.kind.toLowerCase()}…"
              value={queries[tier.tier] ?? ''}
              on:input={(e) => setQuery(tier.tier, e.currentTarget.value)} />

            {#if tier.shown.length === 0}
              <p class="none">No {tier.kind.toLowerCase()} matches that.</p>
            {:else}
              <!-- A row each, full width, like the file list: 43 Features as
                   wrapped inline tiles gave no column to scan down, and a name
                   moved every time the sidebar was resized. -->
              <div class="rows">
                {#each tier.shown as node (node.id)}
                  <!-- The label no longer wraps the whole row, because the row
                       now holds a second control: a click anywhere inside a
                       label activates its input, so a pin button under one
                       would tick the checkbox beside it every time. -->
                  <div class="row">
                    <label class="pick" title={markFor(node.id) || node.qualified_name}>
                      <input
                        type="checkbox"
                        checked={$specSelection.has(node.id)}
                        on:change={() => toggleSpecSelection(node.id)} />
                      <span class="kind-code">{KIND_CODES[node.kind_raw] ?? ''}</span>
                      <span class="name" class:marked={markFor(node.id) !== ''}>{node.name}</span>
                    </label>
                    <!-- The other half of the pane's shift-click, and the only
                         way to make a pin that a keyboard can reach: the pane
                         draws entities as 5px circles with no tab stop, so the
                         gesture there is a modifier on a pointer and cannot be
                         anything else. Same split the checkbox already rests
                         on — the pane is spatial and the list is explicit — and
                         the same reason it is a real <button> and not another
                         glyph on a canvas. -->
                    <button
                      type="button"
                      class="pin"
                      class:pinned={$specPinnedHighlight.has(node.id)}
                      aria-pressed={$specPinnedHighlight.has(node.id)}
                      aria-label="{$specPinnedHighlight.has(node.id) ? 'Unpin' : 'Pin'} {node.name}"
                      title="Light this entity's code on the graph and keep it lit"
                      on:click={() => togglePinnedHighlight(node.id)}
                    ><span class="ring" aria-hidden="true"></span></button>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {:else}
          <p class="hint">
            Nothing to offer at this level. Check a {tiers.length ? 'parent' : 'Category'} above,
            or untick <em>Drill down</em>.
          </p>
        {/each}
      {/if}
    {/if}
  </div>
{/if}

<style>
  .hint {
    font-size: 0.7rem;
    color: var(--text-muted);
    line-height: 1.45;
    margin: 0 0 8px;
  }

  .note {
    font-size: 0.7rem;
    color: var(--text-secondary);
    margin: 0 0 8px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }

  .opt {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.7rem;
    color: var(--text-secondary);
    cursor: pointer;
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

  .tier { margin-bottom: 12px; }

  .tier-count { font-size: 0.62rem; color: var(--text-dim); margin-left: 4px; }

  .only {
    margin-left: auto;
    font-size: 0.62rem;
  }

  .sub-title { display: flex; align-items: center; gap: 4px; margin-bottom: 4px; }

  .tier-filter {
    width: 100%;
    box-sizing: border-box;
    padding: 3px 6px;
    margin-bottom: 3px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--bg-body);
    color: var(--text);
    font-size: 0.7rem;
    font-family: inherit;
  }

  .tier-filter::placeholder { color: var(--text-dim); }

  /* Capped so four tiers cannot push the rest of Post-Filtering off the
     panel. Roughly nine rows — enough that scrolling is the exception once
     a filter has been typed, which is what the box above is for. */
  .rows {
    max-height: 200px;
    overflow-y: auto;
  }

  .row {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 2px 4px;
    border-radius: 3px;
  }

  .row:hover { background: var(--bg-hover); }

  /* The label keeps the row's old shape and its whole clickable width; only
     the pin sits outside it. `min-width: 0` so the truncation on `.name`
     still has something to truncate against once the row is a flex parent
     of a flex child. */
  .pick {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: 1;
    min-width: 0;
    cursor: pointer;
  }

  /* Quiet until it matters. Not hidden until hover: a control that appears
     only under a pointer is one a keyboard reader never finds, and this is
     the surface that exists *because* the pane's gesture needs a pointer.
     Faint, focusable, and loud once it is pinned or reached. */
  .pin {
    background: none;
    border: none;
    padding: 2px;
    margin-left: 4px;
    line-height: 0;
    cursor: pointer;
    opacity: 0.25;
    flex-shrink: 0;
  }

  .row:hover .pin,
  .pin:focus-visible,
  .pin.pinned { opacity: 1; }

  /* The same ring the entity wears in the spec pane and on the graph, so the
     three surfaces are recognisably one thing. The literal is `PIN_COLOR`
     from `SpecGraphView` — a channel identity rather than themed text, for
     the reason given where that constant is declared. */
  .ring {
    display: block;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    border: 2px solid var(--text-muted);
  }

  .pin.pinned .ring { border-color: #7C4DFF; }

  .none {
    font-size: 0.68rem;
    color: var(--text-dim);
    font-style: italic;
    margin: 2px 0 0;
  }

  .kind-code {
    font-size: 0.58rem;
    color: var(--text-dim);
    min-width: 1.4em;
    flex-shrink: 0;
  }

  /* Truncate rather than wrap: a wrapped name makes two rows look like two
     entities in a list whose whole job is one row per entity. */
  .name {
    font-size: 0.72rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Claims nothing reachable — checking it empties the canvas. Dimmed rather
     than disabled: finding out that a Feature has no `cr:` is a legitimate
     reason to click it. */
  .name.marked { color: var(--text-dim); font-style: italic; }
</style>
