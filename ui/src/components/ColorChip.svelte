<script lang="ts">
  /**
   * A palette colour shown as a swatch beside themed text.
   *
   * The entity-kind, relationship-kind and language palettes are chosen to
   * separate nodes on the graph canvas. Using them as *text* colour in the
   * sidebar borrows a decision made for one purpose and applies it to
   * another, and it fails: JavaScript's brand yellow renders at 1.25:1 on the
   * light theme's white panel, DependsOn's lilac at 2.22:1 (UI-023).
   *
   * The colour still identifies the item — it just does so as a dot, while
   * the label stays legible in the theme's own text colour.
   */
  export let color: string;
  export let label: string;
  /** Muted variant for secondary rows (search results, legends). */
  export let dim = false;
</script>

<span class="chip" class:dim>
  <i class="dot" style="background: {color}"></i>{label}
</span>

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-secondary);
  }
  .chip.dim { color: var(--text-muted); }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    /* A hairline keeps a dark swatch visible on a dark panel and a light one
       visible on a light panel, without knowing which theme is active. */
    box-shadow: 0 0 0 1px var(--border-subtle);
  }
</style>
