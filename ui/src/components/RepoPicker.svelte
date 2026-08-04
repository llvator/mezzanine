<script lang="ts">
  /**
   * Serve-mode landing page (UI-007): choose which of the server's loaded
   * repos to visualize. Shown instead of the graph until a repo is active.
   */
  import { availableRepos, refreshRepos, type RepoSummary } from '../stores/serveMode';
  import SubmitRepo from './SubmitRepo.svelte';

  /** Called with the chosen slug. The parent owns loading that repo's data. */
  export let onSelect: (slug: string) => void;

  // SRV-004 made `/api/repos` report in-flight submissions too. Only ready
  // repos have data behind them — picking any other status just yields a
  // 409. UI-008 replaces this filter with real progress rows.
  $: readyRepos = $availableRepos.filter((r) => r.status === 'ready');

  let refreshing = false;

  async function refresh() {
    refreshing = true;
    try {
      await refreshRepos();
    } finally {
      refreshing = false;
    }
  }

  /** `owner__repo` → `owner/repo` for display; leave anything else alone. */
  function displayName(slug: string): string {
    const parts = slug.split('__');
    return parts.length === 2 ? `${parts[0]}/${parts[1]}` : slug;
  }

  function shortSha(sha: string | null): string {
    return sha ? sha.slice(0, 7) : '—';
  }

  function analyzedAt(repo: RepoSummary): string {
    return new Date(repo.ready_at * 1000).toLocaleString();
  }
</script>

<div class="picker">
  <header>
    <h1>Nao</h1>
    <p class="tagline">Pick a repository to explore its code graph.</p>
  </header>

  <SubmitRepo onReady={onSelect} />

  {#if readyRepos.length === 0}
    <div class="empty">
      <p>No repositories analyzed yet — paste a GitHub URL above to add one.</p>
    </div>
  {:else}
    <ul class="repo-list">
      {#each readyRepos as repo (repo.slug)}
        <li>
          <button class="repo" on:click={() => onSelect(repo.slug)}>
            <span class="name">{displayName(repo.slug)}</span>
            <span class="meta">
              {repo.entity_count.toLocaleString()} entities ·
              {repo.relationship_count.toLocaleString()} relationships ·
              <code>{shortSha(repo.sha)}</code>
            </span>
            <span class="when" title="Analyzed at">{analyzedAt(repo)}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <footer>
    <button class="refresh" on:click={refresh} disabled={refreshing}>
      {refreshing ? 'Refreshing…' : '↻ Refresh'}
    </button>
  </footer>
</div>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 24px;
    height: 100vh;
    width: 100vw;
    padding: 32px;
    overflow-y: auto;
    background: var(--bg-deep);
    color: var(--text);
  }

  header { text-align: center; }
  header h1 { margin: 0; color: var(--accent); }
  .tagline { margin: 6px 0 0; color: var(--text-muted); font-size: 0.9rem; }

  .repo-list {
    list-style: none;
    margin: 0;
    padding: 0;
    width: 100%;
    max-width: 620px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .repo {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas: 'name when' 'meta meta';
    gap: 4px 12px;
    width: 100%;
    text-align: left;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 14px 18px;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .repo:hover { background: var(--bg-hover); border-color: var(--accent); }

  .name { grid-area: name; font-size: 1rem; font-weight: 600; }
  .when { grid-area: when; color: var(--text-dim, #666); font-size: 0.75rem; }
  .meta { grid-area: meta; color: var(--text-muted); font-size: 0.78rem; }

  code {
    background: var(--bg-deep);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.75rem;
  }

  .empty { text-align: center; color: var(--text-muted); font-size: 0.85rem; }

  .refresh {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    padding: 6px 14px;
    border-radius: 20px;
    font: inherit;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .refresh:hover:not(:disabled) { color: var(--text); border-color: var(--text-dim, #666); }
  .refresh:disabled { opacity: 0.6; cursor: default; }
</style>
