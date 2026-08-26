<script lang="ts">
  /**
   * The branch the canvas is drawing (UI-114).
   *
   * Sits at the head of the canvas strip, before the commit picker, because
   * that is the order the two are read in: this says which branch's code the
   * circles are, and the buttons after it choose what to compare *against*
   * it. Put after them, it reads as another thing you can pick.
   *
   * Renders nothing at all when the root is not a git checkout — see
   * `branchLabel`.
   */
  import { onMount } from 'svelte';
  import { branchInfo, fetchBranch } from '../stores/branch';
  import { branchLabel } from '../viewmodels/branchLabel';

  onMount(() => {
    void fetchBranch();
  });

  $: label = branchLabel($branchInfo);
</script>

{#if label}
  <span
    class="branch-chip"
    class:detached={label.detached}
    data-probe="branch-chip"
    title={label.title}
  >
    <!-- The git branch glyph, not an emoji: it is the mark git itself uses,
         and it stays legible at 0.75rem in all four themes. -->
    <span class="branch-glyph" aria-hidden="true">⎇</span>
    <span class="branch-name">{label.text}</span>
  </span>
{/if}

<style>
  .branch-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 22ch;
    font-size: 0.75rem;
    padding: 4px 8px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    /* A branch name is an identifier, and a long one has to be readable at
       the end as well as the start — `feat/…-chip` says more than `feat/th…`.
       The `direction` pair puts the ellipsis on the left without reordering
       the characters. */
    white-space: nowrap;
    overflow: hidden;
  }

  .branch-glyph {
    color: var(--text-dim);
    flex-shrink: 0;
  }

  .branch-name {
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl;
    text-align: left;
    unicode-bidi: plaintext;
  }

  /* Detached is not an error, so it is not red — but it is a state most
     readers arrived at without meaning to, and the chip is the only thing on
     screen that can say so. */
  .branch-chip.detached {
    border-style: dashed;
    color: var(--text-muted);
  }
</style>
