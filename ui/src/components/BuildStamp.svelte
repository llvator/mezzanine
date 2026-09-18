<script lang="ts">
  /**
   * Which build am I looking at?
   *
   * Three artifacts ship separately — the engine binary, `ui/dist` for the
   * browser UI, and `webview-dist` for the VS Code webview — refreshed by
   * different commands (`install.sh` does the binary and the webview,
   * `build.sh --ui` does the browser bundle). Nothing in the running app
   * named any of them, so "did my rebuild land?" was answered by guessing at
   * whether a changed string had appeared.
   *
   * Two placements. Standalone it is the right-hand end of the shortcut bar,
   * which is where a status line carries its build info and where it no longer
   * has to overlap anything. In the VS Code webview there is no such bar, so
   * it stays what it was: fixed to the bottom-right corner of the canvas,
   * which the webview leaves empty.
   */
  import { connection } from '../stores/connection';
  import { uiBuild, formatBuiltAt } from '../buildInfo';

  /** Laid out in flow rather than pinned to the corner. */
  export let inline = false;

  $: engine = $connection?.kind === 'ok' ? $connection : null;
  // An engine built before /api/hello carried a version answers the probe
  // fine and must not render as a blank or a crash — say so instead.
  $: engineLabel = engine
    ? (engine.commit ?? (engine.version ? `v${engine.version}` : 'pre-stamp'))
    : '—';

  $: title = [
    `UI    ${uiBuild.commit} (${uiBuild.target}) built ${formatBuiltAt(uiBuild.builtAt)}`,
    engine
      ? `engine ${engine.version ?? '?'} ${engine.commit ?? '(no commit reported)'} · ${engine.mode}`
      : 'engine not connected',
    '',
    'ui/dist comes from scripts/build.sh --ui;',
    'the webview and the binary come from scripts/install.sh.',
  ].join('\n');
</script>

<div class="build-stamp" class:inline {title} data-probe="build-stamp">
  <span class="part">ui&nbsp;{uiBuild.commit}</span>
  <span class="sep">·</span>
  <span class="part" class:stale={engine && engine.commit && engine.commit !== uiBuild.commit}>
    engine&nbsp;{engineLabel}
  </span>
</div>

<style>
  .build-stamp.inline {
    position: static;
    flex: none;
    padding-left: 4px;
  }

  .build-stamp {
    position: fixed;
    right: 8px;
    bottom: 2px;
    z-index: 5;
    display: flex;
    gap: 4px;
    font-size: 0.62rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: var(--text-disabled);
    pointer-events: auto;
    user-select: text;
    /* Deliberately quiet: it is reference material, not a status anyone
       needs to read while working. Full detail is in the tooltip. */
    opacity: 0.75;
  }

  .build-stamp:hover {
    opacity: 1;
    color: var(--text-dim);
  }

  .sep {
    opacity: 0.5;
  }

  /* The two halves disagreeing is the exact situation this exists to catch:
     a rebuilt binary against a stale bundle, or the reverse. */
  .part.stale {
    color: var(--danger-fg);
  }
</style>
