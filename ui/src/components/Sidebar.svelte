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
  import ChangedFiles from './ChangedFiles.svelte';
  import { sidebarTab } from '../stores/panes';
  import { diffActive } from '../stores/diff';

  /**
   * Which tab is showing is a store rather than component state since UI-075:
   * `f`, `q`, `g` and `s` switch tabs from the keyboard, and the handler that
   * owns those keys is not inside this component.
   *
   * `changes` is the one tab that can be unavailable. It is offered only while
   * a comparison is loaded — with no diff it would be a permanently empty tab,
   * and the picker that fills it is above the canvas, not in here — so a
   * reader standing on it when the diff is stopped lands back on Filters
   * rather than on nothing.
   */
  $: activeTab = $sidebarTab === 'changes' && !$diffActive ? 'filters' : $sidebarTab;
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
    {#if $diffActive}
      <button
        type="button"
        class="tab"
        class:active={activeTab === 'changes'}
        data-probe="tab-changes"
        title="The files git reports for the loaded comparison (G)"
        on:click={() => sidebarTab.set('changes')}
      >Changes</button>
    {/if}
    <button
      type="button"
      class="tab tab-icon"
      class:active={activeTab === 'settings'}
      on:click={() => sidebarTab.set('settings')}
      title="Settings"
    >
      <!-- A gear, not the sun this button used to wear: the pane behind it
           stopped being about appearance alone once the settings report, the
           view preferences and the mirror switch moved in. -->
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
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
    {:else if activeTab === 'changes'}
      <ChangedFiles />
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
