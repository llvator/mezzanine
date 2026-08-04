<script lang="ts">
  import { activeTheme, THEMES, type ThemeId, autoFitView } from '../stores/settings';

  function selectTheme(id: ThemeId) {
    activeTheme.set(id);
  }
</script>

<div class="settings">
  <h1>Settings</h1>

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
</style>
