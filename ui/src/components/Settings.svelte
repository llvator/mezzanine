<script lang="ts">
  import { activeTheme, THEMES, type ThemeId, autoFitView } from '../stores/settings';
  import { focusExpand } from '../stores/panes';
  import { mirrorOn, mirrorPeers, openSecondWindow } from '../stores/mirror';
  import SettingsReport from './SettingsReport.svelte';
  import CacheReport from './CacheReport.svelte';

  function selectTheme(id: ThemeId) {
    activeTheme.set(id);
  }
</script>

<div class="settings">
  <h1>Settings</h1>

  <!-- First, because it is the only section describing something that was
       already true before the reader opened this panel. The rest are
       preferences they set here. -->
  <SettingsReport />

  <div class="setting-section">
    <h2>Theme</h2>
    <p class="section-desc">Choose a color theme for the interface.</p>

    <div class="theme-grid">
      {#each THEMES as theme (theme.id)}
        <button
          type="button"
          class="theme-card"
          class:active={$activeTheme === theme.id}
          on:click={() => selectTheme(theme.id)}
        >
          <!-- Color swatch preview -->
          <div class="theme-preview" style="
            background: {theme.colors.bgBody};
            border-color: {$activeTheme === theme.id ? theme.colors.accent : theme.colors.border};
          ">
            <div class="preview-sidebar" style="background: {theme.colors.bgSurface};">
              <div class="preview-line" style="background: {theme.colors.accent}; width: 60%;"></div>
              <div class="preview-line" style="background: {theme.colors.textDim}; width: 80%;"></div>
              <div class="preview-line" style="background: {theme.colors.textDim}; width: 50%;"></div>
            </div>
            <div class="preview-main" style="background: {theme.colors.bgBody};">
              <div class="preview-dot" style="background: {theme.colors.accent};"></div>
              <div class="preview-dot" style="background: #4CAF50;"></div>
              <div class="preview-dot" style="background: #2196F3;"></div>
              <div class="preview-dot" style="background: #FF9800;"></div>
              <svg class="preview-link" viewBox="0 0 40 20">
                <line x1="5" y1="10" x2="35" y2="10" stroke="{theme.colors.textDisabled}" stroke-width="1" />
              </svg>
            </div>
          </div>

          <div class="theme-label">
            <span class="theme-name" style="color: {theme.colors.text}">{theme.name}</span>
            <span class="theme-desc">{theme.description}</span>
          </div>

          {#if $activeTheme === theme.id}
            <div class="active-indicator" style="background: {theme.colors.accent};"></div>
          {/if}
        </button>
      {/each}
    </div>
  </div>

  <div class="setting-section">
    <h2>Behavior</h2>
    <p class="section-desc">Control automatic viewport adjustments.</p>

    <label class="toggle-row">
      <input type="checkbox" bind:checked={$autoFitView} />
      <div class="toggle-label">
        <span class="toggle-name">Auto Fit View</span>
        <span class="toggle-desc">Automatically fit the viewport after layout transitions (view mode switch, selection change).</span>
      </div>
    </label>
  </div>

  <div class="setting-section">
    <h2>Layout</h2>
    <p class="section-desc">How much of the window the pane you are reading gets.</p>

    <label class="toggle-row">
      <input type="checkbox" bind:checked={$focusExpand} />
      <div class="toggle-label">
        <span class="toggle-name">Expand the focused pane <kbd>z</kbd></span>
        <span class="toggle-desc">
          The pane the keyboard is in grows into whatever room the others can
          spare; they shrink to their minimum rather than disappearing. Widths
          you dragged are remembered, not overwritten — turning this off puts
          every pane back where you left it.
        </span>
      </div>
    </label>
  </div>

  <div class="setting-section">
    <h2>Second window</h2>
    <p class="section-desc">
      Two windows on the same reading, so two monitors can hold panes one
      window cannot.
    </p>

    <label class="toggle-row">
      <input type="checkbox" bind:checked={$mirrorOn} />
      <div class="toggle-label">
        <span class="toggle-name">
          Keep windows in sync
          {#if $mirrorOn}
            <span class="peers" class:alone={$mirrorPeers === 0}>
              {$mirrorPeers === 0 ? 'no other window' : `${$mirrorPeers} other window${$mirrorPeers > 1 ? 's' : ''}`}
            </span>
          {/if}
        </span>
        <span class="toggle-desc">
          Scope, level, filters, graph or tree, and the entity you select or
          point at all move together — so a pane on one screen narrates the
          canvas on the other. That includes the Spec pane: hovering an
          Elevator entity here rings the code it claims on the other screen's
          graph, which is the arrangement this whole thing is for. Pane widths,
          zoom and which panes are open stay each window's own; that is what
          makes the second screen worth having. While your pointer is over this
          window it wins, and the other window's takes over again when you
          leave. Works between windows of this browser on this machine; it
          cannot reach another browser or another computer.
        </span>
      </div>
    </label>

    <button type="button" class="second-window" on:click={openSecondWindow}>
      Open a second window
    </button>
  </div>

  <!-- Last, because it is the only section that is not about this repository
       and not a preference: it describes a directory shared by every repo on
       the machine, and its buttons delete from it. -->
  <CacheReport />
</div>

<style>
  .settings {
    padding: 0;
  }

  h1 {
    font-size: 1.4rem;
    margin-bottom: 20px;
    color: var(--accent);
  }

  h2 {
    font-size: 1rem;
    margin: 15px 0 6px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 5px;
    color: var(--accent);
  }

  .section-desc {
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-bottom: 14px;
  }

  .theme-grid {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .theme-card {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-body);
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    transition: border-color 0.15s, background 0.15s;
  }

  .theme-card:hover {
    background: var(--bg-hover);
  }

  .theme-card.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }

  .theme-preview {
    flex-shrink: 0;
    width: 80px;
    height: 50px;
    border-radius: 4px;
    border: 1px solid;
    display: flex;
    overflow: hidden;
  }

  .preview-sidebar {
    width: 30%;
    padding: 5px 4px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .preview-line {
    height: 3px;
    border-radius: 1px;
  }

  .preview-main {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    flex-wrap: wrap;
    padding: 6px;
  }

  .preview-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }

  .preview-link {
    position: absolute;
    width: 100%;
    height: 100%;
    top: 0;
    left: 0;
    opacity: 0.4;
  }

  .theme-label {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .theme-name {
    font-size: 0.9rem;
    font-weight: 600;
  }

  .theme-desc {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .active-indicator {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .toggle-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
    background: var(--bg-body);
    transition: background 0.15s;
  }
  .toggle-row:hover { background: var(--bg-hover); }
  .toggle-row input[type="checkbox"] {
    margin-top: 2px;
    accent-color: var(--accent);
    cursor: pointer;
  }
  .toggle-label { display: flex; flex-direction: column; gap: 2px; }
  .toggle-name { font-size: 0.85rem; color: var(--text); }
  .toggle-desc { font-size: 0.72rem; color: var(--text-dim); line-height: 1.4; }

  /* A mirror with nobody on the other end is the failure worth seeing, so the
     count is drawn even when it is zero — and drawn differently when it is. */
  .peers {
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: 8px;
    background: var(--bg-surface-alt);
    color: var(--text-dim);
    font-size: 0.68rem;
  }
  .peers.alone { color: var(--text-disabled); }

  .second-window {
    margin-top: 10px;
    padding: 6px 12px;
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-family: inherit;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .second-window:hover { background: var(--bg-hover); border-color: var(--accent); }

  /* Same chip the help overlay draws, so a key named in a setting and the same
     key in the cheat sheet read as one thing. */
  kbd {
    display: inline-block;
    min-width: 1.1em;
    padding: 0 4px;
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-secondary);
    font-family: inherit;
    font-size: 0.72rem;
    line-height: 1.5;
    text-align: center;
  }
</style>
