<script lang="ts">
  /**
   * The status bar along the bottom: where the keyboard is, and what it can do
   * from there.
   *
   * Lazygit's bargain, and the reason this is a permanent row rather than a
   * dialog behind `?`: single-letter keys are only affordable if the reader can
   * see which ones are live without asking. The pane chips carry the digits, so
   * the navigation map is on screen at all times; the right-hand list changes
   * with focus, which is also how the reader learns that focus is a thing.
   *
   * Every chip is a button. A key the mouse can also press is a key that
   * teaches itself — you click "Fit view", you see `F` next to it, and next
   * time you press `F`.
   */
  import BuildStamp from './BuildStamp.svelte';
  import { PANES, bindingsForScope, displayKeys, type Binding } from '../viewmodels/keymap';
  import { focusedPane, focusPane, shortcutHelpOpen } from '../stores/keymap';
  import { runCommand, type KeymapContext } from '../viewmodels/keymapActions';

  /** Carries `graphView`, which the viewport commands need and no store has. */
  export let ctx: KeymapContext = {};

  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/i.test(navigator.userAgent);

  /** `+` and `=` both zoom in; the bar shows the command once. First spelling
   *  wins, which is the one the help overlay leads with too. */
  function oneChipPerCommand(bindings: Binding[]): Binding[] {
    const seen = new Set<string>();
    return bindings.filter((b) => !seen.has(b.command) && (seen.add(b.command), true));
  }

  $: paneBindings = oneChipPerCommand(bindingsForScope($focusedPane));
</script>

<footer class="shortcut-bar" data-probe="shortcut-bar">
  <nav class="panes" aria-label="Panes">
    {#each PANES as pane}
      <button
        type="button"
        class="chip pane-chip"
        class:active={$focusedPane === pane.id}
        aria-pressed={$focusedPane === pane.id}
        data-probe="pane-chip-{pane.id}"
        title="{pane.hint} — press {pane.digit}"
        on:click={() => focusPane(pane.id)}
      >
        <kbd>{pane.digit}</kbd>{pane.label}
      </button>
    {/each}
  </nav>

  <div class="divider" aria-hidden="true"></div>

  <!-- The focused pane's own keys. Scrolls rather than wraps: a second row
       would move the canvas every time focus changed panes. -->
  <div class="pane-keys" data-probe="pane-keys">
    {#each paneBindings as binding (binding.command)}
      <button
        type="button"
        class="chip"
        title={binding.label}
        on:click={() => runCommand(binding.command, ctx)}
      >
        <kbd>{displayKeys(binding.keys, mac)}</kbd>{binding.label}
      </button>
    {/each}
  </div>

  <button
    type="button"
    class="chip help-chip"
    class:active={$shortcutHelpOpen}
    data-probe="shortcut-help-toggle"
    title="Every shortcut, by pane"
    on:click={() => shortcutHelpOpen.update((v) => !v)}
  >
    <kbd>?</kbd>Keys
  </button>

  <!-- Which build you are looking at. It used to float in the bottom-right
       corner, which is now this bar's corner; a status line is where it
       belonged anyway. -->
  <BuildStamp inline />
</footer>

<style>
  .shortcut-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    height: 28px;
    padding: 0 8px;
    box-sizing: border-box;
    background: var(--bg-surface-alt);
    border-top: 1px solid var(--border);
    font-size: 0.72rem;
    overflow: hidden;
  }

  .panes {
    display: flex;
    gap: 2px;
    flex: none;
  }

  .divider {
    width: 1px;
    height: 16px;
    background: var(--border);
    flex: none;
  }

  .pane-keys {
    display: flex;
    gap: 2px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .pane-keys::-webkit-scrollbar { height: 0; }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    padding: 2px 6px;
    background: transparent;
    border: none;
    border-radius: 3px;
    color: var(--text-dim);
    font-family: inherit;
    font-size: inherit;
    white-space: nowrap;
    cursor: pointer;
  }
  .chip:hover { background: var(--bg-hover); color: var(--text); }

  .chip.active { color: var(--accent); }

  kbd {
    display: inline-block;
    min-width: 1.1em;
    padding: 0 3px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-secondary);
    font-family: inherit;
    font-size: 0.95em;
    line-height: 1.4;
    text-align: center;
  }
  .chip.active kbd {
    border-color: var(--accent);
    color: var(--accent);
  }

  .help-chip { flex: none; }
</style>
