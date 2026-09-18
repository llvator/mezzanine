<script lang="ts">
  /**
   * What the engine is doing, in the page rather than in the terminal
   * (UI-138).
   *
   * Two things, deliberately in one control. The chip is the answer to "is
   * anything happening", which needs no click and no decision — a diff on a
   * large repository runs for over a minute, and the page used to sit there
   * looking finished for all of it. The panel behind it is the answer to
   * "what did it say", which is worth a click: parse warnings explain why
   * files a reader expected are missing from the graph, and until now the
   * only copy of them was on whichever terminal launched `mezz watch`.
   *
   * Idle is a state this still shows, rather than disappearing in: the feed
   * is most useful just *after* a run, when a reader wants to know what the
   * thing that finished actually did.
   */
  import { onDestroy } from 'svelte';
  import { engineBusy, engineFeed, engineRunning, engineWarnings } from '../stores/activity';
  import { clean, elapsed, type Notice } from '../viewmodels/activityFeed';

  let open = false;

  /**
   * Wall clock, ticked only while there is something to time. A permanent
   * one-second timer on a page that is idle most of the time is a wakeup
   * per second for a number nobody is looking at.
   */
  let now = Date.now();
  let ticker: ReturnType<typeof setInterval> | null = null;

  $: syncTicker($engineBusy);

  function syncTicker(busy: boolean): void {
    if (busy && !ticker) {
      now = Date.now();
      ticker = setInterval(() => (now = Date.now()), 1000);
    } else if (!busy && ticker) {
      clearInterval(ticker);
      ticker = null;
    }
  }

  onDestroy(() => {
    if (ticker) clearInterval(ticker);
  });

  function label(n: Notice): string {
    return `${new Date(n.at_ms).toLocaleTimeString()} · ${n.phase}`;
  }
</script>

<div class="engine-activity" class:open>
  <button
    type="button"
    class="activity-chip"
    class:busy={$engineBusy}
    class:warned={!$engineBusy && $engineWarnings > 0}
    data-probe="engine-activity"
    data-activity-state={$engineBusy ? 'busy' : 'idle'}
    aria-expanded={open}
    on:click={() => (open = !open)}
    title={$engineRunning
      ? `${clean($engineRunning.message)} — click for everything the engine has said`
      : 'What the engine has been doing'}
  >
    {#if $engineRunning}
      <span class="mode-icon pulse">⚙</span>
      <span class="activity-text">{clean($engineRunning.message)}</span>
      <span class="activity-elapsed">{elapsed($engineRunning.since_ms, now)}</span>
    {:else}
      <span class="mode-icon">☰</span>
      <span class="activity-text">Activity</span>
      {#if $engineWarnings > 0}
        <span class="activity-badge" title="{$engineWarnings} warning(s) in what the engine last said">
          {$engineWarnings}
        </span>
      {/if}
    {/if}
  </button>

  {#if open}
    <div class="activity-panel" data-probe="engine-activity-panel">
      {#if $engineFeed.length === 0}
        <p class="activity-empty">
          Nothing yet. The engine reports here as it walks, parses and diffs.
        </p>
      {:else}
        <!-- The store hands these back newest first, so the panel opens on
             what just happened rather than on what has scrolled furthest
             away. -->
        <ul>
          {#each $engineFeed as entry (entry.seq)}
            <li class="activity-row" class:warn={entry.kind === 'warn'} class:end={entry.kind === 'end'}>
              <span class="activity-meta">{label(entry)}</span>
              <span class="activity-message">{clean(entry.message)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .engine-activity {
    position: relative;
    display: inline-flex;
  }

  .activity-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 340px;
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.78rem;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.2s;
  }
  .activity-chip:hover { border-color: var(--text-dim, #666); color: #fff; }
  /* Same amber the reload indicator uses, so "the engine is working" reads
     as one state whichever chip a reader happens to be looking at. */
  .activity-chip.busy {
    color: #FFA726;
    border-color: rgba(255, 167, 38, 0.4);
    background: rgba(255, 167, 38, 0.08);
  }
  .activity-chip.warned { color: #FFCA28; }

  /* The message is the part that can be arbitrarily long — a warning names a
     full path — so it is the part that gives way. */
  .activity-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .activity-elapsed {
    flex: none;
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
  }
  .activity-badge {
    flex: none;
    padding: 0 6px;
    border-radius: 10px;
    background: rgba(255, 202, 40, 0.18);
    font-variant-numeric: tabular-nums;
  }

  .activity-panel {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    z-index: 30;
    width: min(560px, 70vw);
    max-height: 320px;
    overflow-y: auto;
    padding: 8px;
    border-radius: 10px;
    border: 1px solid var(--border, #0f3460);
    background: var(--bg-surface, #16213e);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
    text-align: left;
  }
  .activity-panel ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .activity-row {
    display: flex;
    gap: 10px;
    padding: 4px 6px;
    border-radius: 6px;
    font-size: 0.75rem;
    color: var(--text-muted, #aaa);
  }
  .activity-row + .activity-row { border-top: 1px solid rgba(255, 255, 255, 0.04); }
  .activity-row.warn { color: #FFCA28; }
  .activity-row.end { color: var(--text, #eee); }

  .activity-meta {
    flex: none;
    opacity: 0.6;
    font-variant-numeric: tabular-nums;
  }
  /* Wraps rather than truncates: in the panel the reader has asked to see
     the message, and a path cut off mid-way is the one thing that would
     make a parse warning useless. */
  .activity-message {
    flex: 1;
    overflow-wrap: anywhere;
  }

  .activity-empty {
    margin: 0;
    padding: 6px;
    font-size: 0.75rem;
    color: var(--text-muted, #aaa);
  }
</style>
