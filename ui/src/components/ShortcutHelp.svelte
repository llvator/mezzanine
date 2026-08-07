<script lang="ts">
  /**
   * The whole keymap at once, on `?`.
   *
   * The bar answers "what can I do here"; this answers "what is there", which
   * is a different question and worth a different surface. Panes are listed in
   * digit order and the focused one is marked, so the overlay doubles as an
   * answer to "where am I".
   */
  import {
    PANES, BINDINGS, bindingsForScope, displayKeys, type Binding, type Scope,
  } from '../viewmodels/keymap';
  import { focusedPane, shortcutHelpOpen } from '../stores/keymap';

  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/i.test(navigator.userAgent);

  interface Row { command: string; keys: string[]; label: string }

  /** One row per command, with its spellings joined — `+` and `=` are one way
   *  to zoom in, not two things to learn. */
  function rows(scope: Scope): Row[] {
    const byCommand = new Map<string, Row>();
    for (const b of bindingsForScope(scope)) {
      const row = byCommand.get(b.command);
      if (row) row.keys.push(b.keys);
      else byCommand.set(b.command, { command: b.command, keys: [b.keys], label: b.label });
    }
    return [...byCommand.values()];
  }

  /** The pane digits get one combined row above, so drop them here —
   *  listed individually they would bury the four global keys that have
   *  nowhere else to appear. */
  const globalRows = rows('global').filter((r) => !r.command.startsWith('pane.focus.'));

  const paneSections = PANES.map((p) => ({ pane: p, rows: rows(p.id) }));

  /** Cheap guard against the map growing a command nothing lists. */
  $: total = BINDINGS.length;
</script>

<!-- svelte-ignore a11y-click-events-have-key-events -->
<div
  class="backdrop"
  role="presentation"
  data-probe="shortcut-help"
  on:click={() => shortcutHelpOpen.set(false)}
>
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="sheet" role="dialog" aria-modal="true" tabindex="-1"
    aria-label="Keyboard shortcuts" on:click|stopPropagation>
    <header>
      <h2>Keyboard shortcuts</h2>
      <span class="count">{total} keys · a pane's keys are live while it holds focus</span>
      <button type="button" class="close" title="Close (Esc)" on:click={() => shortcutHelpOpen.set(false)}>✕</button>
    </header>

    <div class="columns">
      <section>
        <h3>Anywhere</h3>
        <dl>
          <div class="row">
            <dt>{#each PANES as p, i}<kbd>{p.digit}</kbd>{#if i < PANES.length - 1}<span class="sep">/</span>{/if}{/each}</dt>
            <!-- Read off the same list as the digits beside it: written out,
                 this row missed the spec pane for as long as that pane has
                 existed. -->
            <dd>Focus {PANES.map((p) => p.label).join(' / ')}</dd>
          </div>
          {#each globalRows as row}
            <div class="row">
              <dt>{#each row.keys as k, i}<kbd>{displayKeys(k, mac)}</kbd>{#if i < row.keys.length - 1}<span class="sep">/</span>{/if}{/each}</dt>
              <dd>{row.label}</dd>
            </div>
          {/each}
        </dl>
      </section>

      {#each paneSections as section}
        <section class:focused={$focusedPane === section.pane.id}>
          <h3>
            <kbd class="pane-digit">{section.pane.digit}</kbd>
            {section.pane.label}
            {#if $focusedPane === section.pane.id}<span class="here">focused</span>{/if}
          </h3>
          <p class="hint">{section.pane.hint}</p>
          <dl>
            {#each section.rows as row}
              <div class="row">
                <dt>{#each row.keys as k, i}<kbd>{displayKeys(k, mac)}</kbd>{#if i < row.keys.length - 1}<span class="sep">/</span>{/if}{/each}</dt>
                <dd>{row.label}</dd>
              </div>
            {/each}
          </dl>
        </section>
      {/each}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 45%);
    padding: 24px;
  }

  .sheet {
    display: flex;
    flex-direction: column;
    max-width: 900px;
    max-height: 100%;
    width: 100%;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 32px rgb(0 0 0 / 35%);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-surface-alt);
  }

  h2 {
    margin: 0;
    font-size: 0.95rem;
    color: var(--text);
  }

  .count {
    flex: 1;
    font-size: 0.72rem;
    color: var(--text-muted);
  }

  .close {
    background: transparent;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font-family: inherit;
    font-size: 0.85rem;
  }
  .close:hover { color: var(--text); }

  .columns {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 4px 20px;
    padding: 16px;
    overflow-y: auto;
  }

  section {
    padding: 8px 10px;
    border-radius: 4px;
    border: 1px solid transparent;
  }
  section.focused {
    border-color: var(--accent);
    background: var(--bg-surface-alt);
  }

  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 2px;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-secondary);
  }

  .here {
    font-size: 0.68rem;
    text-transform: none;
    letter-spacing: 0;
    color: var(--accent);
  }

  .hint {
    margin: 0 0 8px;
    font-size: 0.72rem;
    color: var(--text-muted);
  }

  dl { margin: 0; }

  .row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 0;
  }

  dt {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 2px;
    min-width: 3.4em;
  }

  dd {
    margin: 0;
    font-size: 0.78rem;
    color: var(--text-dim);
  }

  .sep { color: var(--text-muted); font-size: 0.7rem; }

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

  section.focused kbd { background: var(--bg-surface); }

  .pane-digit { color: var(--accent); border-color: var(--accent); }
</style>
