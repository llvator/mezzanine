<script lang="ts">
  /**
   * "Add a repo" form plus one progress card per in-flight job (UI-008).
   * Lives inside the picker; the picker owns navigation.
   */
  import { onDestroy } from 'svelte';
  import {
    submissions, submitRepo, dismiss, setOnReady,
    rehydrateSubmissions, closeAllStreams,
  } from '../stores/submissions';

  /** Called with a slug when a repo is ready to open. */
  export let onReady: (slug: string) => void;

  let url = '';
  let error: string | null = null;
  let submitting = false;

  setOnReady(onReady);
  rehydrateSubmissions();
  onDestroy(closeAllStreams);

  async function submit() {
    if (submitting || !url.trim()) return;
    submitting = true;
    error = null;
    try {
      const result = await submitRepo(url);
      if (result.kind === 'error') {
        error = result.message;
      } else {
        url = '';
        // Already analyzed — straight to the graph, nothing to watch.
        if (result.kind === 'ready') onReady(result.slug);
      }
    } finally {
      submitting = false;
    }
  }

  /** Copy comes straight from the backend state name, so a future state
   *  renders as itself rather than disappearing. */
  const LABELS: Record<string, string> = {
    queued: 'Queued…',
    cloning: 'Cloning…',
    analyzing: 'Analyzing…',
    ready: 'Ready',
    failed: 'Failed',
  };
  const label = (s: string) => LABELS[s] ?? s;

  const STEPS = ['queued', 'cloning', 'analyzing', 'ready'];
  const stepIndex = (s: string) => STEPS.indexOf(s);

  function displayName(slug: string): string {
    const parts = slug.split('__');
    return parts.length === 2 ? `${parts[0]}/${parts[1]}` : slug;
  }
</script>

<section class="submit">
  <h2>Add a repo</h2>
  <form on:submit|preventDefault={submit}>
    <input
      type="text"
      bind:value={url}
      placeholder="https://github.com/owner/repo"
      aria-label="GitHub repository URL"
      disabled={submitting}
    />
    <button type="submit" disabled={submitting || !url.trim()}>
      {submitting ? 'Submitting…' : 'Analyze'}
    </button>
  </form>
  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#each $submissions as sub (sub.slug)}
    <div class="card" class:failed={sub.status === 'failed'}>
      <div class="card-head">
        <span class="repo">{displayName(sub.slug)}</span>
        <span class="state">{label(sub.status)}</span>
        <button class="dismiss" on:click={() => dismiss(sub.slug)} title="Dismiss">×</button>
      </div>
      {#if sub.status === 'failed'}
        <p class="reason">{sub.error ?? 'The job failed.'}</p>
      {:else}
        <ol class="steps">
          {#each STEPS as step}
            <li
              class:done={stepIndex(sub.status) > STEPS.indexOf(step)}
              class:current={sub.status === step}
            >{step}</li>
          {/each}
        </ol>
      {/if}
    </div>
  {/each}
</section>

<style>
  .submit {
    width: 100%;
    max-width: 620px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }

  form { display: flex; gap: 8px; }

  input {
    flex: 1;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 10px 12px;
    color: var(--text);
    font: inherit;
    font-size: 0.85rem;
  }
  input:focus { outline: none; border-color: var(--accent); }
  input:disabled { opacity: 0.6; }

  button[type='submit'] {
    background: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 6px;
    padding: 10px 18px;
    color: var(--accent-fg);
    font: inherit;
    font-size: 0.85rem;
    cursor: pointer;
  }
  button[type='submit']:disabled { opacity: 0.5; cursor: default; }

  .error { margin: 0; color: #EF9A9A; font-size: 0.8rem; }

  .card {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 12px 16px;
  }
  .card.failed { border-color: #c04040; }

  .card-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .repo { font-weight: 600; font-size: 0.9rem; }
  .state { color: var(--text-muted); font-size: 0.78rem; flex: 1; }

  .dismiss {
    background: none;
    border: none;
    color: var(--text-dim, #666);
    font-size: 1rem;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }
  .dismiss:hover { color: var(--text); }

  .reason {
    margin: 8px 0 0;
    color: #EF9A9A;
    font-size: 0.78rem;
    word-break: break-word;
  }

  .steps {
    display: flex;
    gap: 6px;
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    font-size: 0.7rem;
    color: var(--text-dim, #666);
  }
  .steps li {
    flex: 1;
    text-align: center;
    padding: 3px 0;
    border-top: 2px solid var(--border);
  }
  .steps li.done { color: var(--text-muted); border-top-color: var(--text-muted); }
  .steps li.current { color: var(--accent); border-top-color: var(--accent); }
</style>
