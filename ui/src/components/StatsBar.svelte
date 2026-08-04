<script lang="ts">
  /**
   * The canvas count read-out.
   *
   * Four different entity counts used to be on screen at once with nothing
   * naming any of them — the index total, the in-scope total, the rendered
   * node count, and Quality's full-detail count. Each was correct; together
   * they read as a product that cannot count. Worse, this bar reported
   * `graphData.nodes.length`, which is the pre-display graph: after switching
   * to tree view it claimed 41 while seven nodes were on screen.
   *
   * The vocabulary, used here and in the Quality panel (UI-010):
   *
   *   Indexed   — every entity nao knows about, whole repo
   *   In scope  — entities inside the current visual scope
   *   Shown     — nodes actually drawn, after aggregation collapse
   *   Analysed  — full-detail entities behind the quality metrics
   *
   * This component owns "Shown" and its relation to "In scope".
   */
  import { displayPlan } from '../viewmodels/displayPlan';
  import { graphLevel } from '../stores/graph';
  import { selectionStats, autoLevel } from '../stores/scope';

  /** What a drawn node represents at the current aggregation level. */
  const LEVEL_NOUN: Record<string, string> = {
    entity: 'entities',
    file: 'files',
    module: 'modules',
  };

  $: shownNodes = $displayPlan?.visibleNodeIds.size ?? 0;
  $: shownLinks = $displayPlan?.visibleLinkKeys.size ?? 0;
  $: inScope = $selectionStats.entities;
  $: noun = LEVEL_NOUN[$graphLevel] ?? 'nodes';

  /** True when the view aggregated away from individual entities. Here
   *  "N files drawn, from M entities in scope" is a real relationship: those
   *  M entities live in those N files. */
  $: collapsed = $graphLevel !== 'entity' && inScope > 0;

  /**
   * At entity level the two figures are NOT comparable and must not be joined
   * with "of". The canvas draws synthetic entities the index never counts —
   * measured on this repo's `ui` scope: 2010 nodes drawn against 1123 in
   * scope, the difference being 319 Parameter, 341 Property and 312 Constant
   * nodes. Printing "Shown: 1995 entities of 1,123 in scope" reproduces the
   * exact confusion UI-010 exists to remove, in new words.
   *
   * This is also the answer to the 3948-vs-9357 gap the ticket asked to
   * sanity-check: fine-grained entities, not double-counting.
   */
  $: comparable = collapsed;

  $: tip = collapsed
    ? `Shown: ${shownNodes} ${noun} drawn on the canvas.\n` +
      `In scope: ${inScope} entities inside the checked folders, collapsed to ${$graphLevel} level` +
      ($autoLevel ? ' automatically, because the scope exceeded the render budget.' : '.')
    : `Shown: ${shownNodes} ${noun} drawn on the canvas.\n` +
      'Includes parameters, properties and constants, which the in-scope ' +
      'figure in the Scope panel does not count — the two are not comparable.';
</script>

<span class="counts" data-probe="stats-counts" title={tip}>
  <strong>Shown:</strong> {shownNodes} {noun}
  {#if comparable}
    <span class="collapse-note">
      (from {inScope.toLocaleString()} entities in scope, collapsed to {$graphLevel} level{$autoLevel ? ' automatically' : ''})
    </span>
  {/if}
  <span class="sep">|</span>
  {shownLinks} relationships
</span>

<style>
  .counts {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    cursor: help;
  }
  .counts strong { font-weight: 600; color: var(--text-secondary); }
  .collapse-note { color: var(--text-dim); font-size: 0.92em; }
  .sep { color: var(--text-disabled); }
</style>
