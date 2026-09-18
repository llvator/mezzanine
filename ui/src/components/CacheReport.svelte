<script lang="ts">
  /**
   * What mezz has cached on this machine, and removing part of it (UI-153).
   *
   * The one thing this panel must not let a reader assume: **these rows are
   * not this repository.** The parse store is machine-global, so a row here
   * can name a checkout this window has never opened, and clearing it slows
   * down a different project's next analysis. Hence the repository name on
   * every row, the full path under it, and the cache root spelled out.
   *
   * Clearing is safe at any moment — a missing entry degrades to a cold
   * parse, never to an error — so there is no modal in the way. What there
   * is instead is a second click: the button arms, then confirms. Enough to
   * stop a slip, not enough to be ceremony.
   */
  import { onMount } from 'svelte';
  import {
    cacheReport,
    cacheError,
    cacheBusy,
    cacheLoaded,
    loadCacheReport,
    clearCache,
    humanBytes,
    humanAge,
    humanCount,
    type ClearTarget,
  } from '../stores/cacheReport';

  /** Which button is armed, as a stable key. One at a time: arming a second
   *  disarms the first, so there is never a screen with two live triggers. */
  let armed: string | null = null;

  function keyOf(target: ClearTarget): string {
    if (target.scope === 'repo') return `repo:${target.generation}:${target.repo}`;
    if (target.scope === 'generation') return `gen:${target.generation}`;
    return target.scope;
  }

  async function press(target: ClearTarget) {
    const key = keyOf(target);
    if (armed !== key) {
      armed = key;
      return;
    }
    armed = null;
    await clearCache(target);
  }

  function label(target: ClearTarget, resting: string): string {
    return armed === keyOf(target) ? 'Really?' : resting;
  }

  onMount(loadCacheReport);
</script>

<div class="setting-section">
  <h2>Disk cache</h2>
  <p class="section-desc">
    Parsed files mezz keeps so an unchanged file is never parsed twice. This
    is <strong>shared by every repository on this machine</strong>, not just
    this one — a row below may well be a project this window has never opened.
    Clearing costs the next analysis some re-parsing and nothing else.
  </p>

  {#if $cacheError}
    <p class="cache-error">{$cacheError}</p>
  {/if}

  {#if $cacheReport}
    <p class="cache-root" title="The directory these numbers describe">
      <code>{$cacheReport.root}</code>
      <span class="cache-total">{humanBytes($cacheReport.bytes)} in total</span>
    </p>

    {#each $cacheReport.generations as gen (gen.tag)}
      <div class="gen" class:abandoned={!gen.current}>
        <div class="gen-head">
          <span class="gen-tag" title="Cache generation — bumped whenever a parser change makes older entries meaningless">
            {gen.tag}
          </span>
          {#if gen.current}
            <span class="badge current">in use</span>
          {:else}
            <span class="badge stale" title="Nothing reads this. It is kept for a week in case an older mezz binary is still installed and using it.">
              abandoned
            </span>
          {/if}
          <span class="gen-usage">
            {humanBytes(gen.bytes)} · {humanCount(gen.entries)} files
          </span>
          <button
            class="clear"
            class:armed={armed === keyOf({ scope: 'generation', generation: gen.tag })}
            disabled={$cacheBusy}
            on:click={() => press({ scope: 'generation', generation: gen.tag })}
          >
            {label({ scope: 'generation', generation: gen.tag }, gen.current ? 'Clear' : 'Reclaim')}
          </button>
        </div>

        {#if gen.repos.length}
          <table class="repos">
            <tbody>
              {#each gen.repos as repo (repo.id ?? repo.label)}
                <tr>
                  <td class="repo-name">
                    <span class="repo-label">{repo.label}</span>
                    {#if repo.common_dir}
                      <span class="repo-path" title={repo.common_dir}>{repo.common_dir}</span>
                    {/if}
                  </td>
                  <td class="num">{humanBytes(repo.bytes)}</td>
                  <td class="num dim">{humanCount(repo.entries)}</td>
                  <td class="num dim">{humanAge(repo.last_used)}</td>
                  <td class="act">
                    {#if repo.id}
                      <button
                        class="clear small"
                        class:armed={armed ===
                          keyOf({ scope: 'repo', generation: gen.tag, repo: repo.id })}
                        disabled={$cacheBusy}
                        on:click={() =>
                          press({ scope: 'repo', generation: gen.tag, repo: repo.id as string })}
                      >
                        {label({ scope: 'repo', generation: gen.tag, repo: repo.id }, 'Clear')}
                      </button>
                    {:else}
                      <span
                        class="dim"
                        title="Written before mezz grouped the cache by repository, so it cannot be attributed to one. Clear the generation to reclaim it."
                      >—</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="empty">Nothing cached under this generation.</p>
        {/if}
      </div>
    {/each}

    {#if $cacheReport.reshape.entries > 0}
      <div class="gen">
        <div class="gen-head">
          <span class="gen-tag" title="Baselines mezz reshape compares against">reshape baselines</span>
          <span class="gen-usage">
            {humanBytes($cacheReport.reshape.bytes)} ·
            {humanCount($cacheReport.reshape.entries)} files
          </span>
          <button
            class="clear"
            class:armed={armed === 'reshape'}
            disabled={$cacheBusy}
            on:click={() => press({ scope: 'reshape' })}
          >
            {label({ scope: 'reshape' }, 'Clear')}
          </button>
        </div>
      </div>
    {/if}

    <div class="bulk">
      <button class="link" disabled={$cacheBusy} on:click={loadCacheReport}>Refresh</button>
      {#if $cacheReport.generations.some((g) => !g.current)}
        <button
          class="clear"
          class:armed={armed === 'abandoned'}
          disabled={$cacheBusy}
          on:click={() => press({ scope: 'abandoned' })}
        >
          {label({ scope: 'abandoned' }, 'Reclaim every abandoned generation')}
        </button>
      {/if}
      <button
        class="clear danger"
        class:armed={armed === 'everything'}
        disabled={$cacheBusy}
        on:click={() => press({ scope: 'everything' })}
      >
        {label({ scope: 'everything' }, 'Clear everything')}
      </button>
    </div>
  {:else if $cacheLoaded && !$cacheError}
    <p class="empty">This server does not report a cache directory.</p>
  {:else if !$cacheLoaded}
    <p class="empty">Counting…</p>
  {/if}
</div>

<style>
  .section-desc {
    margin: 0 0 12px;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-secondary);
  }

  .cache-root {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: baseline;
    margin: 0 0 12px;
    font-size: 11px;
  }

  .cache-root code {
    color: var(--text-dim);
    word-break: break-all;
  }

  .cache-total {
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .gen {
    margin-bottom: 14px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }

  .gen.abandoned {
    opacity: 0.85;
  }

  .gen-head {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    background: var(--bg-surface-alt);
  }

  .gen-tag {
    font-family: var(--font-mono, monospace);
    font-size: 11px;
    color: var(--text);
  }

  .gen-usage {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .badge {
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .badge.current {
    background: var(--bg-body);
    color: var(--accent);
  }

  .badge.stale {
    background: var(--bg-body);
    color: var(--text-disabled);
  }

  .repos {
    width: 100%;
    border-collapse: collapse;
    font-size: 11px;
  }

  .repos td {
    padding: 6px 10px;
    border-top: 1px solid var(--border-subtle);
    vertical-align: baseline;
  }

  .repo-name {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .repo-label {
    color: var(--text);
  }

  .repo-path {
    font-size: 10px;
    color: var(--text-disabled);
    word-break: break-all;
  }

  .num {
    text-align: right;
    white-space: nowrap;
    color: var(--text-secondary);
  }

  .dim {
    color: var(--text-disabled);
  }

  .act {
    text-align: right;
  }

  .bulk {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }

  button.clear {
    padding: 3px 9px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-body);
    color: var(--text-secondary);
    font-size: 11px;
    cursor: pointer;
  }

  button.clear:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--text-dim);
  }

  button.clear.armed {
    border-color: var(--danger-fg);
    color: var(--danger-fg);
  }

  button.clear.danger {
    margin-left: auto;
  }

  button.clear:disabled {
    opacity: 0.5;
    cursor: default;
  }

  button.small {
    padding: 2px 7px;
  }

  button.link {
    padding: 0;
    border: none;
    background: none;
    color: var(--text-dim);
    font-size: 11px;
    cursor: pointer;
    text-decoration: underline;
  }

  .empty,
  .cache-error {
    margin: 0 0 10px;
    font-size: 11px;
    color: var(--text-dim);
  }

  .cache-error {
    color: var(--danger-fg);
  }
</style>
