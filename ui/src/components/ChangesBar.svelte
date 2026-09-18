<script lang="ts">
  /**
   * The changes row: which branch this is, what to compare it against, what
   * the comparison found, and whether any of it is still current.
   *
   * Its own row under the columns since UI-150, not an overlay pinned to the
   * canvas floor. As an overlay it was a strip whose width was the canvas's
   * width, and every pane the reader opened took width away from it: the diff
   * badge wraps, so a narrowed canvas pushed the badge upward over the graph
   * it was describing, and dragging the graph pane's edge moved controls the
   * reader was aiming at. A real row spans the window instead. It costs the
   * height it occupies and nothing else — no pane resize can reach it, and it
   * covers no node.
   */
  import BranchChip from './BranchChip.svelte';
  import CommitPicker from './CommitPicker.svelte';
  import EngineActivity from './EngineActivity.svelte';
  import {
    diffActive, diffData, diffLevel, diffSeedFacet, diffChangedEdges, shownFromRef,
    diffDimOpacity, diffContextOpacity,
    CONTEXT_OPACITY_FLOOR, REST_OPACITY_CEILING, stopDiff,
  } from '../stores/diff';
  import { DIFF_LEVELS, SEED_FACETS, type DiffLevel, type DiffSeedFacet } from '../viewmodels/diffLevels';
  import {
    liveConnected, liveReloading, liveStatus, liveIsBroken,
    reconnectLiveReload, stopLiveReload,
  } from '../stores/liveReload';
  import { refreshData, refreshing } from '../stores/scope';
  import { serveMode } from '../stores/serveMode';
  import { changesBarOpen } from '../stores/panes';
  import { endpoint, forgetEndpoint } from '../endpoint';

  function toggleLiveMode() {
    if ($liveConnected) {
      stopLiveReload();
    } else {
      // Always a fresh start: the button is also how you recover from a
      // stream that gave up, and resuming a spent backoff would do nothing.
      reconnectLiveReload();
    }
  }

  /** What the mode indicator says when the stream isn't running.
   *  A refused origin is the one failure a user can't diagnose from the
   *  browser, and the remedy is a flag on a command they already ran — so
   *  the label names it. */
  const LIVE_STOP_LABEL: Record<string, string> = {
    refused: 'Not live — engine refused this page',
    token: 'Not live — pairing token needed',
    unreachable: 'Not live — no engine answering',
    'no-stream': 'Not live — no event stream',
  };

  const LIVE_STOP_HINT: Record<string, string> = {
    refused: 'Restart the engine with --allow-origin ' + window.location.origin,
    token: "Paste the pairing token from the engine's startup banner",
    unreachable: 'Nothing answered on this endpoint. Click to try again.',
    'no-stream': 'The API answers but /events does not. Click to try again.',
  };

  async function manualRefresh() {
    await refreshData();
  }

  /** Drop the stored endpoint and go back to the connect screen. */
  function disconnectEndpoint() {
    forgetEndpoint();
    window.location.reload();
  }

  /* Hover copy for the diff level ladder (UI-088). The two checkboxes this
     replaced read as near-synonyms and each needed a paragraph to say how it
     differed from the other; rungs on an ordered ladder only have to say what
     they add to the rung below. The slider says it is the way back to the
     parts of the graph the ladder took away. */
  const LEVEL_LABEL: Record<DiffLevel, string> = {
    edits: 'Edits',
    rewiring: 'Rewiring',
    neighbourhood: 'Neighbourhood',
  };
  const LEVEL_TIP: Record<DiffLevel, string> = {
    edits:
      'Edits — only what you actually edited.\n\n'
      + 'Entities whose own source or intrinsic metrics moved, plus everything '
      + 'added and removed. Between them, only the relationships that changed.\n\n'
      + 'Drops impact-only ripple: entities whose code is byte-for-byte '
      + 'identical and whose only movement is a fan-in / fan-out count. The '
      + 'narrowest rung, and the default — on most diffs the ripple outnumbers '
      + 'the real edits and drowns them.',
    rewiring:
      'Rewiring — the edits, plus what they now point at.\n\n'
      + 'Adds the far end of every relationship that appeared, even when that '
      + 'entity was never edited. This is the rung that shows a function you '
      + 'changed calling a helper you did not — the case a filter on entities '
      + 'alone can never draw.\n\n'
      + 'Still only changed relationships get a line.',
    neighbourhood:
      'Neighbourhood — the edits, plus everything one hop away.\n\n'
      + 'Adds every direct neighbour of a changed entity and draws all the '
      + 'wiring between what is shown, changed or not. Use it to see what your '
      + 'change sits next to; expect most of the lines to be untouched.',
  };
  /* The seed split (UI-109). A second control rather than a fourth rung: the
     ladder is ordered — each rung adds to the one below — and new code and
     pre-existing code are siblings, so they have no place on it. It sits to
     the LEFT of the ladder because that is the order the two apply in: this
     one chooses the seed, the ladder widens from it. */
  const FACET_LABEL: Record<DiffSeedFacet, string> = {
    all: 'All',
    new: 'New',
    existing: 'Existing',
  };
  const FACET_TIP: Record<DiffSeedFacet, string> = {
    all:
      'All — both halves of the change.\n\n'
      + 'The whole seed, and what the ladder drew before this control '
      + 'existed.',
    new:
      'New — only code that did not exist before.\n\n'
      + 'Entities the diff reports as added, plus — on a diff of the working '
      + 'tree — files created since it ran, which the diff never saw.\n\n'
      + 'Pair it with Neighbourhood to see what the new code plugs into.',
    existing:
      'Existing — only code that was already there.\n\n'
      + 'Entities that existed on the base side and changed in place. '
      + 'Deletions count as existing: they were there to be deleted.\n\n'
      + 'This is the half that needs reviewing against what it used to do.',
  };
  const CONTEXT_TIP =
    'Context — how strongly the entities this rung recruited are drawn, '
    + 'against the edits it grew from.\n\n'
    + 'Above Edits the ladder draws code you did not touch: the far end of a '
    + 'changed relationship at Rewiring, everything one hop out at '
    + 'Neighbourhood. At Neighbourhood that context usually outnumbers the '
    + 'changes several times over, and at full strength it is drawn exactly '
    + 'like them.\n\n'
    + 'This weights the two apart. It cannot remove anything — stepping down '
    + 'a rung is what does that.';
  const REST_TIP =
    'Rest — how visible the entities the ladder left out stay.\n\n'
    + 'At 0% everything below the current rung is gone from the canvas. Raise '
    + 'it to fade the rest of the graph back in as faint context around the '
    + 'changed nodes, so you can see what your changes sit next to without '
    + 'losing track of which nodes changed.\n\n'
    + `Up to ${Math.round(REST_OPACITY_CEILING * 100)}%, which is far enough to `
    + 'read a name rather than just see a shape. It stops short of full '
    + 'strength on purpose: the rest is the quietest of the three tiers, and '
    + 'drawn as loudly as an edit it would leave the diff colours as the only '
    + 'thing telling them apart.';

  /** Reported edge changes with nowhere to go on the canvas: the ones that
   *  disappeared (no line in the head graph) plus the ones whose far end the
   *  diff could not resolve. */
  $: undrawableEdges = $diffChangedEdges.removedCount + $diffChangedEdges.unplaceable;
</script>

<!-- A row, not an overlay. Folding it away leaves the strip behind, because a
     folded row that vanished would take the only route back to the comparison
     controls with it — and, while a diff is loaded, the only sign one is on. -->
<div class="changes-bar" class:collapsed={!$changesBarOpen} data-probe="changes-bar">
  {#if $changesBarOpen}
    <div class="stats" data-probe="canvas-stats">
      <!-- Which branch these circles are. First in the row because the
           controls after it all pick something to compare *against* it, and
           because with a comparison loaded every other ref on screen belongs
           to the overlay rather than to the canvas (UI-114). -->
      <BranchChip />
      <!-- Commit picker drives `POST /api/diff`, which serve mode doesn't
           expose. Hidden there rather than offering a button that 404s. -->
      {#if !$serveMode}
        <CommitPicker />
      {/if}
      {#if $diffActive && $diffData}
        <span class="diff-summary-badge" data-probe="diff-badge">
          🔀 {$shownFromRef}→{$diffData.to_ref}:
          <span style="color:#A5D6A7">+{$diffData.summary.added}</span>
          <span style="color:#EF9A9A">-{$diffData.summary.removed}</span>
          <span style="color:#FFCC80" title="{$diffData.summary.modified_source ?? $diffData.summary.modified} core, {$diffData.summary.modified_impact ?? 0} impact">
            ~{$diffData.summary.modified}
          </span>
          <!-- Entity counts say how much code moved; this says how much the
               graph rewired, which the three above cannot: a swapped call
               changes no count of entities at all. -->
          {#if ($diffData.summary.relationships_added ?? 0) + ($diffData.summary.relationships_removed ?? 0) > 0}
            <span
              class="diff-edge-counts"
              data-probe="diff-edge-counts"
              title="Relationships that appeared or disappeared. Select an entity to see which — the Details pane lists its own."
            >
              ⇄ <span style="color:#A5D6A7">+{$diffData.summary.relationships_added ?? 0}</span>
              <span style="color:#EF9A9A">−{$diffData.summary.relationships_removed ?? 0}</span>
            </span>
          {/if}
          <span class="diff-filter-group">
            <!-- The seed split (UI-109), before the ladder because it applies
                 before it: this picks which half of the change seeds the
                 rungs, and every rung then only ever adds to that seed. -->
            <span class="diff-level diff-facet" role="radiogroup" aria-label="Which changes to start from" data-probe="diff-facet">
              {#each SEED_FACETS as facet (facet)}
                <button
                  type="button"
                  role="radio"
                  aria-checked={$diffSeedFacet === facet}
                  class="diff-level-rung"
                  class:active={$diffSeedFacet === facet}
                  data-probe="diff-facet-{facet}"
                  title={FACET_TIP[facet]}
                  on:click={() => diffSeedFacet.set(facet)}
                >{FACET_LABEL[facet]}</button>
              {/each}
            </span>
            <!-- The ladder, narrow → wide (UI-088). A segmented control rather
                 than checkboxes because the rungs are ordered: the reader can
                 see which way each one moves the picture, which two
                 independent toggles could never say. -->
            <span class="diff-level" role="radiogroup" aria-label="Diff detail level" data-probe="diff-level">
              {#each DIFF_LEVELS as level (level)}
                <button
                  type="button"
                  role="radio"
                  aria-checked={$diffLevel === level}
                  class="diff-level-rung"
                  class:active={$diffLevel === level}
                  data-probe="diff-level-{level}"
                  title={LEVEL_TIP[level]}
                  on:click={() => diffLevel.set(level)}
                >{LEVEL_LABEL[level]}</button>
              {/each}
            </span>
            <!-- Only above the narrowest rung: at `edits` every drawn node is
                 an edit, so the control would have nothing to weight and
                 would read as a slider that does nothing. -->
            {#if $diffLevel !== 'edits'}
              <label class="diff-filter-toggle diff-opacity-control" title={CONTEXT_TIP}>
                <span class="diff-opacity-name">Context</span>
                <input type="range" min={CONTEXT_OPACITY_FLOOR * 100} max="100" step="5"
                  data-probe="diff-context-opacity"
                  value={$diffContextOpacity * 100}
                  on:input={(e) => diffContextOpacity.set(Number(e.currentTarget.value) / 100)} />
                <span class="diff-opacity-label">{Math.round($diffContextOpacity * 100)}%</span>
              </label>
            {/if}
            <label class="diff-filter-toggle diff-opacity-control" title={REST_TIP}>
              <span class="diff-opacity-name">Rest</span>
              <input type="range" min="0" max={REST_OPACITY_CEILING * 100} step="1"
                data-probe="diff-rest-opacity"
                value={$diffDimOpacity * 100}
                on:input={(e) => diffDimOpacity.set(Number(e.currentTarget.value) / 100)} />
              <span class="diff-opacity-label">{Math.round($diffDimOpacity * 100)}%</span>
            </label>
            <!-- Never let the canvas imply it drew every reported change. A
                 disappeared edge has no line in the head graph to colour, and
                 an unresolved far end has nowhere to attach — so they are
                 counted here rather than dropped in silence. -->
            {#if undrawableEdges > 0 && $diffLevel !== 'neighbourhood'}
              <span
                class="diff-undrawable"
                data-probe="diff-undrawable"
                title={'Relationships the diff reported but the canvas cannot draw.\n\n'
                  + `${$diffChangedEdges.removedCount} disappeared — a lost edge has no line in the `
                  + 'current graph, by construction.\n'
                  + `${$diffChangedEdges.unplaceable} could not be placed — the diff saw the change but `
                  + 'could not resolve the entity at the far end.\n\n'
                  + 'Select an entity to read its own gained and lost relationships in the Details pane.'}
              >{undrawableEdges} undrawn</span>
            {/if}
          </span>
          <!-- The way out. Diff mode is the one mode of this canvas that
               nothing else turns off: a `→ working` comparison is a
               subscription the engine keeps current on every save, and it
               outlived the page it was started from because the result is
               served to whoever reloads (UI-100). Sits at the end of the
               badge, so the strip that says a diff is on is also the strip
               that ends it. -->
          <button
            type="button"
            class="diff-stop"
            data-probe="diff-stop"
            on:click={() => void stopDiff()}
            title={$diffData.to_ref === 'working'
              ? 'Leave diff mode — stop following the working tree and clear the overlay'
              : 'Leave diff mode — clear the overlay'}
            aria-label="Leave diff mode"
          >×</button>
        </span>
      {/if}
    </div>

    <!-- Mode bar: static / live indicator + controls.
         The live toggle needs `/events`, which serve mode has no equivalent
         of (repos are analyzed once, not watched) — so it's hidden there and
         only the manual refresh remains, which works fine. -->
    <div class="mode-bar-bottom" data-probe="mode-bar">
      {#if !$serveMode}
      <!-- What the engine is doing, and what it has said (UI-138). Beside the
           live indicator rather than inside it: that one reports the health
           of the *connection*, this one reports the work coming down it, and
           a reader chasing a seventy-second diff needs the second. Watch mode
           only — `mezz serve` analyzes once and has no `/events`. -->
      <EngineActivity />
      <button
        type="button"
        class="mode-indicator"
        class:live={$liveConnected}
        class:broken={liveIsBroken($liveStatus)}
        class:reloading={$liveReloading || $refreshing}
        data-probe="live-indicator"
        data-live-state={$liveStatus.kind === 'stopped' ? $liveStatus.reason : $liveStatus.kind}
        on:click={toggleLiveMode}
        title={liveIsBroken($liveStatus) && $liveStatus.kind === 'stopped'
          ? LIVE_STOP_HINT[$liveStatus.reason]
          : $liveConnected
            ? 'Connected to watch server — click to disconnect'
            : 'Not connected — click to connect to watch server'}
      >
        {#if $liveReloading || $refreshing}
          <span class="mode-icon pulse">↻</span> Reloading…
        {:else if $liveConnected}
          <span class="mode-icon">●</span> Live
        {:else if $liveStatus.kind === 'stopped' && $liveStatus.reason !== 'off'}
          <!-- A stream that failed is not the same as one nobody started.
               "Static" for both is what made a refused origin read as
               "nothing is changing". -->
          <span class="mode-icon">⚠</span> {LIVE_STOP_LABEL[$liveStatus.reason]}
        {:else if $liveStatus.kind === 'retrying'}
          <span class="mode-icon pulse">○</span> Reconnecting…
        {:else}
          <span class="mode-icon">○</span> Static
        {/if}
      </button>
      {/if}
      {#if !$liveConnected}
        <button type="button" class="refresh-btn" on:click={manualRefresh} title="Manually reload data files">
          ↻ Refresh
        </button>
      {/if}
      <!-- Which engine this is. Hidden same-origin, where the answer is
           "the one that served this page" and a chip would be noise. -->
      {#if endpoint().base}
        <button
          type="button"
          class="endpoint-chip"
          data-probe="endpoint-chip"
          on:click={disconnectEndpoint}
          title="Connected to {endpoint().base} — click to disconnect and choose another"
        >
          ⇄ {endpoint().base.replace(/^https?:\/\//, '')}
        </button>
      {/if}
    </div>
  {:else}
    <!-- Folded. It still has to answer the one question the row exists for:
         whether what is drawn is a comparison. A refolded row that said
         nothing would leave a coloured canvas with no caption anywhere. -->
    <div class="folded-summary" data-probe="changes-bar-folded">
      {#if $diffActive && $diffData}
        <span class="diff-summary-badge folded">
          🔀 {$shownFromRef}→{$diffData.to_ref}:
          <span style="color:#A5D6A7">+{$diffData.summary.added}</span>
          <span style="color:#EF9A9A">-{$diffData.summary.removed}</span>
          <span style="color:#FFCC80">~{$diffData.summary.modified}</span>
        </span>
      {:else}
        <span class="folded-label">Changes</span>
      {/if}
    </div>
  {/if}

  <button
    type="button"
    class="bar-toggle"
    data-probe="changes-bar-toggle"
    aria-expanded={$changesBarOpen}
    title={$changesBarOpen ? 'Fold the changes row away' : 'Show the changes row'}
    on:click={() => changesBarOpen.set(!$changesBarOpen)}
  >{$changesBarOpen ? '▾' : '▴'}</button>
</div>

<style>
  /* A row of the app shell, spanning every column. Its two groups are laid
     out against each other, so neither can be drawn over by the other however
     wide the diff badge grows — and because the row owns its own height, a
     badge that wraps pushes the row taller instead of climbing over the
     canvas the way the old overlay did. */
  .changes-bar {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 12px;
    background: var(--bg-surface);
    border-top: 1px solid var(--border);
    /* A row that grew without limit would eat the canvas on a narrow window.
       Past this it scrolls, which costs the reader a gesture and costs the
       graph nothing. */
    max-height: 30vh;
    overflow-y: auto;
  }

  /* Folded, the row is a caption and a chevron. Same border, so the seam
     between canvas and row does not move when it opens. */
  .changes-bar.collapsed {
    padding: 2px 12px;
  }

  .folded-summary {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
  }

  .folded-label {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .bar-toggle {
    flex: none;
    appearance: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-surface-alt);
    color: var(--text-muted);
    font: inherit;
    font-size: 0.7rem;
    line-height: 1;
    padding: 3px 7px;
    cursor: pointer;
    align-self: center;
  }
  .bar-toggle:hover { background: var(--bg-hover); color: var(--text); }

  .stats {
    /* Wraps rather than pushes: on a narrow window the badge stacks upward
       into the row's own height, instead of shoving the mode bar off the
       right edge. */
    flex: 0 1 auto;
    min-width: 0;
    font-size: 0.8rem;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }

  .diff-summary-badge {
    font-size: 0.75rem;
    padding: 4px 8px;
    border-radius: 10px;
    background: rgba(255, 167, 38, 0.1);
    border: 1px solid rgba(255, 167, 38, 0.3);
    display: inline-flex;
    align-items: center;
    /* Wrapping is what keeps the badge inside the row. Without it the badge
       overflowed the box the row had shrunk it to and went on reaching right,
       back under the mode bar — measurably clear, visibly on top of it. Every
       group inside it wraps for the same reason. */
    flex-wrap: wrap;
    max-width: 100%;
    gap: 8px;
    row-gap: 6px;
  }

  .diff-summary-badge.folded {
    gap: 5px;
    padding: 2px 7px;
    white-space: nowrap;
  }

  .diff-edge-counts {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding-left: 6px;
    border-left: 1px solid var(--border);
    color: var(--text-muted);
  }

  .diff-filter-group {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    row-gap: 6px;
    padding-left: 8px;
    border-left: 1px solid rgba(255, 167, 38, 0.3);
  }

  .diff-filter-toggle {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    cursor: pointer;
    font-size: 0.7rem;
    color: var(--text-muted, #aaa);
    user-select: none;
  }
  .diff-filter-toggle:hover {
    color: var(--text, #e0e0e0);
  }
  /* One segmented track, not three buttons: the rungs are an ordered ladder,
     and a shared groove with a single lit segment says "pick one position"
     where separate chips would say "toggle each of these". */
  .diff-level {
    display: inline-flex;
    border: 1px solid rgba(255, 167, 38, 0.35);
    border-radius: 4px;
    overflow: hidden;
  }

  /* The seed split is the same shape as the ladder at lower contrast (UI-109).
     Two identically-drawn segmented controls side by side read as one control
     with six buttons — which would say the six are alternatives, and three of
     them are not. Its selected rung still lights up like a rung: a facet that
     is filtering has to be as visible as the rung it seeds. */
  .diff-facet {
    border-color: rgba(255, 167, 38, 0.18);
  }
  .diff-facet .diff-level-rung {
    border-left-color: rgba(255, 167, 38, 0.15);
  }

  .diff-level-rung {
    appearance: none;
    border: none;
    border-left: 1px solid rgba(255, 167, 38, 0.25);
    background: transparent;
    color: var(--text-muted, #aaa);
    font: inherit;
    font-size: 0.7rem;
    padding: 1px 7px;
    cursor: pointer;
    user-select: none;
  }
  .diff-level-rung:first-child {
    border-left: none;
  }
  .diff-level-rung:hover {
    background: rgba(255, 167, 38, 0.12);
    color: var(--text, #e0e0e0);
  }
  .diff-level-rung.active {
    background: rgba(255, 167, 38, 0.28);
    color: var(--text, #e0e0e0);
  }
  .diff-level-rung:focus-visible {
    outline: 1px solid #FFA726;
    outline-offset: -1px;
  }

  .diff-opacity-control input[type="range"] {
    width: 60px;
    height: 4px;
    cursor: pointer;
    accent-color: #FFA726;
  }

  /* Sits just past the ladder, so the slider reads as the counterpart to it
     rather than as an unlabelled control. */
  .diff-opacity-name {
    padding-left: 4px;
    border-left: 1px solid rgba(255, 167, 38, 0.3);
  }

  /* Deliberately plain — a count of what is NOT on screen should not compete
     with the +/− totals beside it, but it must not be invisible either. */
  .diff-undrawable {
    font-size: 0.65rem;
    color: var(--text-dim, #888);
    padding-left: 6px;
    border-left: 1px solid rgba(255, 167, 38, 0.3);
    cursor: help;
  }

  /* Quiet until reached for. It is the only control here that throws work
     away, so it should not read as the next thing to press — but it is also
     the only way out, so it must be findable without a tooltip. */
  .diff-stop {
    appearance: none;
    border: none;
    background: transparent;
    color: var(--text-dim, #888);
    font: inherit;
    font-size: 0.85rem;
    line-height: 1;
    padding: 1px 4px 1px 8px;
    margin-left: 2px;
    border-radius: 3px;
    cursor: pointer;
    border-left: 1px solid rgba(255, 167, 38, 0.3);
  }
  .diff-stop:hover {
    background: rgba(255, 167, 38, 0.2);
    color: var(--text, #e0e0e0);
  }
  .diff-stop:focus-visible {
    outline: 1px solid #FFA726;
    outline-offset: -1px;
  }

  .diff-opacity-label {
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.65rem;
    min-width: 28px;
    text-align: right;
  }

  /* Never squeezed: the live indicator, the refresh button and the endpoint
     chip are the controls that say whether what is on screen is current, and
     a diff badge is not worth losing them to. */
  .mode-bar-bottom {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 6px;
  }

  .mode-indicator {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.8rem;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.2s;
  }
  .mode-indicator:hover { border-color: var(--text-dim, #666); }
  .mode-indicator.live {
    color: #66BB6A;
    border-color: rgba(102, 187, 106, 0.4);
    background: rgba(102, 187, 106, 0.08);
  }
  .mode-indicator.reloading {
    color: #FFA726;
    border-color: rgba(255, 167, 38, 0.4);
  }
  /* A stream that failed reads differently from one nobody started —
     same weight as `.live`, opposite sign, so "not updating" is a state
     you notice rather than the absence of one. */
  .mode-indicator.broken {
    color: #EF5350;
    border-color: rgba(239, 83, 80, 0.4);
    background: rgba(239, 83, 80, 0.08);
  }

  .endpoint-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.75rem;
    font-family: inherit;
    cursor: pointer;
  }
  .endpoint-chip:hover { color: var(--text); border-color: var(--text-dim, #666); }

  .mode-icon { font-size: 0.9rem; }
  .mode-icon.pulse {
    animation: pulse 0.8s ease-in-out infinite;
  }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }

  .refresh-btn {
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.78rem;
    font-family: inherit;
    cursor: pointer;
  }
  .refresh-btn:hover { border-color: var(--text-dim, #666); color: #fff; }
</style>
