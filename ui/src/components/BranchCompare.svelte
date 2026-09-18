<script lang="ts">
  /**
   * Compare two branches (UI-143).
   *
   * Its own component rather than a fourth arm of `CommitPicker`'s modal,
   * because it is the one mode with a *computed* base: the other three send
   * refs the reader picked, this one asks the server where two branches parted
   * and sends that. The panel is the sentence explaining which comparison is
   * about to run — see `viewmodels/branchCompare` for why there are two, and
   * why the wrong one is the one you get by pairing the tips.
   */
  import { onMount } from 'svelte';
  import {
    branches, branchesLoading, fetchBranches,
    fetchMergeBase, triggerDiff, diffComputing, diffApiError,
    type BranchRef, type Commit,
  } from '../stores/diff';
  import { orderBranches, defaultPair, branchPlan } from '../viewmodels/branchCompare';

  /** Called once a comparison has been accepted, so the modal can close. */
  export let onDone: () => void;

  let baseName = '';
  let compareName = '';

  /**
   * Whether the base is where the branches parted (on) or the base branch's
   * tip (off). On by default: it is the reading a code review wants, and the
   * other one silently reports the base branch's own recent work as deletions.
   */
  let fromDivergence = true;

  let divergedAt: Commit | null = null;
  let divergenceLoading = false;

  $: ordered = orderBranches($branches);
  $: local = ordered.filter((b) => !b.remote);
  $: remote = ordered.filter((b) => b.remote);
  $: base = ordered.find((b) => b.name === baseName);
  $: compare = ordered.find((b) => b.name === compareName);
  $: plan = branchPlan(base, compare, divergedAt, fromDivergence);

  onMount(async () => {
    await fetchBranches();
    const pair = defaultPair($branches);
    baseName = pair.base?.name ?? '';
    compareName = pair.compare?.name ?? '';
  });

  /**
   * Ask where the current pair diverged, once per pair.
   *
   * Latest-wins rather than debounced: the answer is a single git call, and
   * the failure worth guarding is not too many requests but an early one
   * landing late and labelling a pair the reader has already moved off.
   */
  let asked = '';
  let inFlight = 0;
  async function refreshDivergence(from: string, to: string): Promise<void> {
    const pair = `${from}…${to}`;
    if (pair === asked) return;
    asked = pair;
    divergedAt = null;
    if (!from || !to || from === to) return;
    const seq = ++inFlight;
    divergenceLoading = true;
    const found = await fetchMergeBase(from, to);
    if (seq !== inFlight) return;
    divergedAt = found;
    divergenceLoading = false;
  }

  $: void refreshDivergence(baseName, compareName);

  async function run(): Promise<void> {
    if (plan.refusal) return;
    await triggerDiff(plan.from, plan.to);
    if (!$diffApiError) onDone();
  }

  /** `main · 2 days` is not worth a date library; the server sends `YYYY-MM-DD`. */
  function meta(b: BranchRef): string {
    return `${b.tip_short} · ${b.date}`;
  }
</script>

<!-- The same thing the stash and index panels have to say, and for the same
     reason (SRV-019): the server only adopts a working-tree head as live
     state, so a comparison of two commits leaves the circles where they were.
     It reads as more surprising here — picking a branch feels like checking
     one out — which is why it is said before the comparison rather than
     after. -->
<p class="stash-note" data-probe="branch-note">
  Two branches, compared without checking either one out. The canvas keeps drawing
  your working tree — the overlay describes the branch.
</p>

{#if $branchesLoading}
  <div class="loading">Reading the branches…</div>
{:else if ordered.length < 2}
  <!-- One branch is the ordinary state of a fresh repository, and saying so is
       not reporting a failure. -->
  <div class="empty">
    This repository has {ordered.length === 1 ? 'one branch' : 'no branches'}. There is
    nothing to compare it against yet.
  </div>
{:else}
  <div class="ref-inputs">
    <div class="ref-input-group">
      <label for="base-branch">Base (what it is measured against)</label>
      <select id="base-branch" data-probe="branch-base" bind:value={baseName}>
        <optgroup label="Local">
          {#each local as b (b.name)}
            <option value={b.name}>{b.name}{b.is_head ? ' (checked out)' : ''}</option>
          {/each}
        </optgroup>
        {#if remote.length > 0}
          <optgroup label="Remote-tracking">
            {#each remote as b (b.name)}
              <option value={b.name}>{b.name}</option>
            {/each}
          </optgroup>
        {/if}
      </select>
      {#if base}<span class="branch-meta">{meta(base)}</span>{/if}
    </div>
    <span class="arrow">→</span>
    <div class="ref-input-group">
      <label for="compare-branch">Branch under review</label>
      <select id="compare-branch" data-probe="branch-compare" bind:value={compareName}>
        <optgroup label="Local">
          {#each local as b (b.name)}
            <option value={b.name}>{b.name}{b.is_head ? ' (checked out)' : ''}</option>
          {/each}
        </optgroup>
        {#if remote.length > 0}
          <optgroup label="Remote-tracking">
            {#each remote as b (b.name)}
              <option value={b.name}>{b.name}</option>
            {/each}
          </optgroup>
        {/if}
      </select>
      {#if compare}<span class="branch-meta">{meta(compare)}</span>{/if}
    </div>
  </div>

  <label class="divergence-toggle">
    <input type="checkbox" data-probe="branch-divergence" bind:checked={fromDivergence} />
    <span>
      Measure from where the two branches diverged
      <em>— what a review of the branch asks. Off, the two tips are compared.</em>
    </span>
  </label>

  <p class="plan" class:refused={plan.refusal !== null} data-probe="branch-plan">
    {#if divergenceLoading}
      Finding where {baseName} and {compareName} diverged…
    {:else}
      {plan.refusal ?? plan.note}
    {/if}
  </p>

  <div class="actions">
    <button
      type="button"
      class="compute-btn"
      data-probe="branch-run"
      on:click={run}
      disabled={plan.refusal !== null || divergenceLoading || $diffComputing}
      title={plan.refusal ?? plan.note}
    >
      {$diffComputing ? 'Computing…' : 'Compare branches'}
    </button>
  </div>
{/if}

<style>
  .stash-note {
    margin: 0 0 1rem;
    padding: 0.5rem 0.75rem;
    border-left: 2px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    font-size: 0.85rem;
    line-height: 1.45;
  }

  .loading, .empty {
    padding: 2rem;
    text-align: center;
    color: var(--text-secondary);
  }

  .ref-inputs {
    display: flex;
    align-items: flex-end;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }
  .ref-input-group {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }
  .ref-input-group label {
    font-size: 0.8rem;
    color: var(--text-secondary);
  }
  .ref-input-group select {
    padding: 0.5rem;
    background: var(--bg-surface-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-family: monospace;
    font-size: 0.9rem;
    max-width: 100%;
  }
  .ref-input-group select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .branch-meta {
    font-size: 0.75rem;
    color: var(--text-dim);
    font-family: monospace;
  }
  .arrow {
    color: var(--text-secondary);
    font-size: 1.2rem;
    padding-bottom: 1.4rem;
  }

  .divergence-toggle {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    font-size: 0.85rem;
    color: var(--text);
    cursor: pointer;
  }
  .divergence-toggle em {
    color: var(--text-secondary);
    font-style: normal;
  }

  /* The sentence, not a hint: it is the only thing on screen that says which
     of the two comparisons is about to run. */
  .plan {
    margin: 0.75rem 0 0;
    padding: 0.5rem 0.75rem;
    border-left: 2px solid var(--accent);
    background: var(--bg-surface-alt);
    color: var(--text);
    font-size: 0.85rem;
    line-height: 1.45;
  }
  /* A refusal is not an error — nothing broke, and the comparison it names
     would simply be empty — so it is bordered like a quiet aside rather than
     coloured like a failure. */
  .plan.refused {
    border-left-color: var(--border);
    color: var(--text-secondary);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 1rem;
  }
  .compute-btn {
    padding: 0.5rem 1rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.9rem;
    background: var(--accent);
    border: none;
    color: var(--accent-fg);
  }
  .compute-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .compute-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
