<script lang="ts">
  /**
   * The left column: one tab at a time, owning the full height.
   *
   * It used to be split, with entity details pinned to the bottom half. The
   * two never wanted the same thing — the scope tree and the ranked problem
   * table grow with the repo, details grows with the entity — so whichever
   * was on top got 65% and the scope tree showed 3 of 7 roots (UI-011).
   * Details is its own column now; this one is a plain tabbed pane.
   */
  import FilterPanel from './FilterPanel.svelte';
  import QualityReport from './QualityReport.svelte';
  import Settings from './Settings.svelte';
  import { sidebarTab } from '../stores/panes';

  /** Which tab is showing is a store rather than component state since
   *  UI-075: `f`, `q` and `s` switch tabs from the keyboard, and the handler
   *  that owns those keys is not inside this component. */
  $: activeTab = $sidebarTab;
</script>

<div class="sidebar">
  <div class="tab-bar">
    <button
      type="button"
      class="tab"
      class:active={activeTab === 'filters'}
      on:click={() => sidebarTab.set('filters')}
    >Filters</button>
    <button
      type="button"
      class="tab"
      class:active={activeTab === 'quality'}
      on:click={() => sidebarTab.set('quality')}
    >Quality</button>
    <button
      type="button"
      class="tab tab-icon"
      class:active={activeTab === 'settings'}
      on:click={() => sidebarTab.set('settings')}
      title="Settings"
    >
      <svg width="14" height="14" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="10" cy="10" r="3" />
        <path d="M10 1.5v2M10 16.5v2M3.4 3.4l1.4 1.4M15.2 15.2l1.4 1.4M1.5 10h2M16.5 10h2M3.4 16.6l1.4-1.4M15.2 4.8l1.4-1.4" />
      </svg>
    </button>
  </div>

  <!-- Quality manages its own internal scrolling (capped head + growing
       table), so the pane must not also be a scroll container or the inner
       flex heights collapse. The other tabs keep plain scrolling. -->
  <div
    class="sidebar-top"
    class:panelled={activeTab === 'quality'}
    data-probe="sidebar-top"
  >
    {#if activeTab === 'filters'}
      <FilterPanel />
    {:else if activeTab === 'quality'}
      <QualityReport />
    {:else}
      <Settings />
    {/if}
  </div>
</div>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .tab-bar {
    display: flex;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    background: var(--bg-surface-alt);
  }

  .tab {
    flex: 1;
    padding: 8px 12px;
    background: transparent;
    color: var(--text-dim);
    border: none;
    border-bottom: 2px solid transparent;
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    cursor: pointer;
    font-family: inherit;
  }
  .tab:hover { color: var(--text-secondary); }
  .tab.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .tab-icon {
    flex: 0 0 auto;
    padding: 8px 12px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .sidebar-top {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 20px;
  }

  .sidebar-top.panelled {
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
</style>
