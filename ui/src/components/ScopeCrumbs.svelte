<script lang="ts">
  /**
   * Where the reader is, in the toolbar bar beside the wayback arrows
   * (UI-095).
   *
   * The arrows walk the history; these walk the folder tree. Putting them
   * side by side is the point — "back" and "up" are different questions, and
   * before this the second one lived in a sidebar button that climbed one
   * level per click.
   *
   * Every crumb is `drillIn`, not a bespoke scope write. That is what makes
   * the climb undoable (it records a frame) and what re-enables `autoLevel`,
   * so jumping two levels up opens the ancestor at the detail it can afford
   * rather than keeping the level the deep scope picked.
   *
   * The shape decisions live in `viewmodels/scopeCrumbs.ts`; this renders
   * them. In particular, an empty scope draws nothing at all: a bare `repo`
   * crumb over a blank canvas would claim the whole repo is on screen.
   */
  import { scopeRules, drillIn } from '../stores/scope';
  import {
    scopeAddress, elideCrumbs, hiddenTitle, crumbTitle,
  } from '../viewmodels/scopeCrumbs';

  $: address = scopeAddress($scopeRules);
  $: parts = address.kind === 'path' ? elideCrumbs(address.crumbs) : { hidden: [], shown: [] };
</script>

{#if address.kind !== 'empty'}
  <nav class="crumbs" aria-label="Current scope" data-probe="scope-crumbs">
    {#if address.kind === 'path'}
      {#if parts.hidden.length > 0}
        <span class="crumb-sep" aria-hidden="true">›</span>
        <span class="elided" title={hiddenTitle(parts.hidden)}>…</span>
      {/if}
      {#each parts.shown as crumb, i (crumb.path)}
        {#if i > 0 || parts.hidden.length > 0}
          <span class="crumb-sep" aria-hidden="true">›</span>
        {/if}
        {#if crumb.current}
          <!-- Not a button: there is nowhere for it to go, and a control that
               looks clickable and does nothing is the complaint UI-078 was
               opened for. -->
          <span class="crumb current" aria-current="location" title={crumbTitle(crumb)}
          >{crumb.label}</span>
        {:else}
          <button
            type="button"
            class="crumb"
            title={crumbTitle(crumb)}
            on:click={() => void drillIn(crumb.path)}
          >{crumb.label}</button>
        {/if}
      {/each}
    {:else}
      <!-- No single address: several paths, or a glob. Named rather than
           guessed at — showing the first of six marked paths would be a
           confident wrong answer. -->
      <span class="crumb current opaque" title="This scope is not one folder, so there is nothing to climb">
        {address.label}
      </span>
    {/if}
    {#if address.filtered}
      <span class="filtered" title="Part of this scope is excluded, so it is not shown whole">filtered</span>
    {/if}
  </nav>
{/if}

<style>
  /* Shrinks before the counts do: `min-width: 0` plus `flex-shrink` is what
     keeps this from being the widest thing in the bar at 1280, which is the
     budget the toolbar has been fighting for since UI-013. */
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
    font-size: 0.75rem;
  }

  .crumb {
    background: none;
    border: none;
    padding: 2px 4px;
    border-radius: 3px;
    font: inherit;
    color: var(--text-secondary);
    white-space: nowrap;
    cursor: pointer;
  }

  button.crumb:hover { color: var(--text); background: var(--bg-hover); }

  .crumb.current {
    color: var(--text);
    font-weight: 600;
    cursor: default;
    /* The one crumb allowed to lose characters — it is also the one whose
       full text is in its own tooltip. */
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .crumb.opaque { font-weight: 400; color: var(--text-secondary); }

  .crumb-sep { color: var(--text-dim); font-size: 0.7rem; }

  .elided { color: var(--text-dim); padding: 0 2px; cursor: default; }

  .filtered {
    margin-left: 4px;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--bg-hover);
    color: var(--text-muted);
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }
</style>
