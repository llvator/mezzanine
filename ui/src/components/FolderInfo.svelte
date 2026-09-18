<script lang="ts">
  /**
   * The region under the pointer's *name*, as the Details column reads it —
   * UI-141.
   *
   * `EntityInfo`'s counterpart for the one thing on the canvas that is drawn
   * without being a node. It deliberately answers the same four questions in
   * the same order, so a reader moving between a file and the folder around
   * it is not learning a second panel: what is this, how much of it is on
   * screen, what do its relationships do, and how healthy is it.
   *
   * Every number here is the engine's own scope rollup — the same row the
   * Quality panel's folder table lists. Nothing is re-derived from what the
   * canvas happens to be drawing, with one deliberate exception: membership
   * and traffic, which are *about* the picture on screen and say so.
   */
  import {
    folderRows, fileRows, tierFromScore, SCORE_SCALE_LABEL,
    baselineNoun, compareToBaseline, repoBaselines,
  } from '../stores/quality';
  import { regionClaimOf } from '../stores/region';
  import { clampDescription } from '../viewmodels/regionSpec';
  import { membershipTitle, trafficSentence } from '../viewmodels/regionTraffic';
  import { regionMembership, scopeRowFor, type HoveredRegion } from '../viewmodels/regionSubject';
  import SpecComposer from './SpecComposer.svelte';
  import FlowLadder from './FlowLadder.svelte';
  import { composerPhase, openComposer, specWritable } from '../stores/specDraft';

  export let region: HoveredRegion;

  $: isFile = region.grain === 'file';
  $: noun = isFile ? 'file' : 'folder';
  /** A file region reads the file rollup and a folder region the folder one.
   *  They are different populations — a folder's row holds its whole subtree —
   *  and crossing them would put a subtree's LOC under a file's name. */
  $: row = scopeRowFor(isFile ? $fileRows : $folderRows, region.path);
  $: scope = row?.scope ?? null;

  /** The repo's mean for this grain, so the Score cell below is readable as
   *  high or low. Files against files and folders against folders: the two are
   *  the same formula over different populations, and a folder rolls up a whole
   *  subtree, so its scores sit on a different part of the range. */
  $: baseline = $repoBaselines.baselines[isFile ? 'file' : 'folder'];
  $: comparison = row && baseline ? compareToBaseline(row.score, baseline) : null;
  $: baselineScope = $repoBaselines.whole ? 'Repo' : 'Shown';
  $: membership = regionMembership(region);
  /** Asked about the region this panel was handed, not about whatever the
   *  pointer is on — the two are the same while hovering and diverge the
   *  moment a region is pinned (UI-148). */
  $: claim = $regionClaimOf(region.path);
  $: claimText = clampDescription(claim?.description ?? null, 320);

  /** Percent, because cohesion is the one scope metric that is already a
   *  ratio and reads as one everywhere else in the app. */
  function pct(v: number | undefined): string {
    return v == null ? '—' : `${Math.round(v * 100)}%`;
  }
</script>

<div class="folder-info" data-probe="folder-info" data-region-path={region.path}>
  <div class="head">
    <span class="kind">{isFile ? 'File' : 'Folder'}</span>
    <span class="name" title={region.path}>{region.label || '(repo root)'}</span>
  </div>
  <div class="path" data-probe="folder-path">{region.path || '(repo root)'}</div>

  <!-- What is on screen, and what is not. The one pair of numbers here that
       describes the canvas rather than the repo — a region half hidden by a
       filter reads identically to a small one without it. -->
  <div class="detail-row">
    <div class="detail-label">On screen</div>
    <!-- `?? undefined`: the store carries "not counted yet" as null and the
         card's helper spells the same state `undefined`. Both mean the same
         sentence — "nodes from this folder currently drawn". -->
    <div class="line" data-probe="folder-membership" title={membershipTitle(region.traffic ?? undefined, noun)}>
      {#if membership.hidden > 0}
        {membership.drawn} of {membership.total} drawn
        <span class="dim">· {membership.hidden} hidden by a filter</span>
      {:else}
        {membership.total} drawn
      {/if}
    </div>
    {#if region.traffic}
      <div class="line dim" data-probe="folder-traffic"
        title="Counted over the relationships currently drawn — not the repo-wide cohesion below">
        {trafficSentence(region.label, region.traffic)}
      </div>
    {/if}
  </div>

  <!-- Which way the dependencies run (UI-146). Directly under the traffic
       counts, because the two are halves of one question: those say how much
       of this region's coupling stays inside it, this says what the inside
       coupling is SHAPED like. Unlike the counts above it, this reads the
       analysed graph rather than the canvas — a hierarchy that rearranged
       itself when a filter moved would not be one. -->
  <FlowLadder subject={{ grain: isFile ? 'file' : 'folder', path: region.path }} />

  <!-- What the spec says. Attributed, and labelled when the claim is
       inherited: `cr: "ui/"` covers this folder but describes `ui`, and
       printing it plain would be the panel putting words in the author's
       mouth (UI-090). The full text is in the Description pane. -->
  {#if claim}
    <div class="detail-row">
      <div class="detail-label">Spec</div>
      <div class="claim" data-probe="folder-spec" data-spec-id={claim.id}>
        <div class="claim-head">
          <span class="claim-kind">{claim.kind}</span>
          <span class="claim-name">{claim.name}</span>
        </div>
        {#if !claim.exact}
          <div class="line dim" data-probe="folder-spec-via">claims {claim.claimPath}</div>
        {/if}
        {#if claimText}
          <div class="line claim-desc">{claimText}</div>
        {/if}
      </div>
    </div>
  {:else if $specWritable}
    <!-- UI-145 — the same row, in the state it is in most of the time. A
         folder is the grain a Category or a Feature usually claims, so this
         is the offer that gets taken most: `cr: "ui/src/"` covers the
         subtree, which is how every folder-level claim in this repo's own
         spec is written. -->
    <div class="detail-row">
      <div class="detail-label">Spec</div>
      {#if $composerPhase.kind === 'closed'}
        <button
          type="button"
          class="add-spec"
          data-probe="folder-add-spec"
          title="Nothing in the spec claims {region.path || 'the repo root'}. Write the entity that would."
          on:click={() => openComposer({ grain: isFile ? 'file' : 'folder', path: region.path })}
        >+ Document this {noun}</button>
      {:else}
        <SpecComposer />
      {/if}
    </div>
  {/if}

  {#if scope}
    <div class="detail-row">
      <div class="detail-label">Quality</div>
      <div class="metrics-grid">
        <div class="metric metric-{row?.tiers.entity ?? 'na'}" title="Entities the engine measured in this {noun}">
          <span class="metric-label">Entities</span>
          <span class="metric-value">{scope.entity_count}</span>
        </div>
        <div class="metric metric-{row?.tiers.loc ?? 'na'}" title="Lines of code">
          <span class="metric-label">LOC</span>
          <span class="metric-value">{scope.loc}</span>
        </div>
        <div class="metric metric-{row?.tiers.cohesion ?? 'na'}"
          title="Share of this {noun}'s dependency edges that stay inside it — measured over the whole repo, not over what is drawn">
          <span class="metric-label">Cohesion</span>
          <span class="metric-value">{pct(scope.cohesion)}</span>
        </div>
        <div class="metric metric-na" title="Scopes depending on this one">
          <span class="metric-label">Fan-in</span>
          <span class="metric-value">{scope.fan_in}</span>
        </div>
        <div class="metric metric-{row?.tiers.fanOut ?? 'na'}" title="Scopes this one depends on">
          <span class="metric-label">Fan-out</span>
          <span class="metric-value">{scope.fan_out}</span>
        </div>
        {#if scope.instability != null}
          <div class="metric metric-na" title="Fan-out over total coupling — 0 is depended upon, 1 depends on everything">
            <span class="metric-label">Instab</span>
            <span class="metric-value">{scope.instability.toFixed(2)}</span>
          </div>
        {/if}
        {#if scope.in_cycle}
          <div class="metric metric-bad" title="This {noun} is part of a dependency cycle">
            <span class="metric-label">Cycle</span>
            <span class="metric-value">!</span>
          </div>
        {/if}
        {#if row}
          <div class="metric metric-{tierFromScore(row.score)}" title="Composite scope score — {SCORE_SCALE_LABEL}">
            <span class="metric-label">Score</span>
            <span class="metric-value">{row.score.toFixed(2)}</span>
          </div>
        {/if}
      </div>
      <!-- The frame the Score cell needs. Coloured by direction, not by tier:
           "worse than average" is the claim, and it can be true of a green
           score in a very green repo. -->
      {#if row && baseline && comparison}
        <div
          class="line score-vs"
          class:worse={comparison.verdict === 'above'}
          class:better={comparison.verdict === 'below'}
          data-probe="folder-score-vs-mean"
          title="Mean composite score of every {baselineNoun(baseline.grain)} in the {baselineScope === 'Repo' ? 'repo' : 'current view'} ({baseline.count} measured). {SCORE_SCALE_LABEL}."
        >
          <span class="dim">{baselineScope} mean ({baselineNoun(baseline.grain)}s): {baseline.mean.toFixed(2)}</span>
          <span class="vs-verdict">
            {comparison.verdict === 'at'
              ? 'about average'
              : `${comparison.delta > 0 ? '+' : '−'}${Math.abs(comparison.delta).toFixed(2)} · ${comparison.text}`}
          </span>
        </div>
      {/if}
      {#if row && row.aggregate.entityCount > 0}
        <div class="line dim" data-probe="folder-tally">
          {row.aggregate.okCount} ok · {row.aggregate.warnCount} warn · {row.aggregate.badCount} bad
          <span class="dim">of {row.aggregate.entityCount} entities in the current population</span>
        </div>
      {/if}
    </div>

    <!-- Folders only: a file has no children to draw a graph of, so it never
         carries a shape. -->
    {#if scope.shape}
      <div class="detail-row">
        <div class="detail-label">Shape</div>
        <div class="line" data-probe="folder-shape">
          {scope.shape.pattern}
          <span class="dim">· {pct(scope.shape.compliance)} compliance</span>
        </div>
      </div>
    {/if}
  {:else}
    <!-- Not a gap to fill with zeroes. The rollups follow the Quality panel's
         population, so a region outside it was never measured — which is a
         different statement from "measured, and empty". -->
    <p class="empty" data-probe="folder-unmeasured">
      No rollup for this {noun} in the current Quality population.
    </p>
  {/if}

  <!-- Both halves of what the click does, because the second half is the one
       that keeps this panel readable: focusing swaps the canvas out from under
       the pointer, and without the pin the hover it rode in on would go with
       it (UI-148). -->
  <div class="hint">Click its name on the canvas to focus this {noun} and pin it here.</div>
</div>

<style>
  .folder-info { font-size: 0.82rem; }

  .head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }

  .kind {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 5px;
    border-radius: 2px;
    background: var(--bg-surface-alt);
    color: var(--text-muted);
    border: 1px solid var(--border);
    flex-shrink: 0;
  }

  .name {
    font-weight: 600;
    font-size: 0.95rem;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .path {
    margin-top: 3px;
    font-size: 0.7rem;
    color: var(--text-dim);
    word-break: break-all;
  }

  .detail-row { margin-top: 12px; }

  .detail-label {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin-bottom: 5px;
  }

  .line {
    font-size: 0.78rem;
    line-height: 1.5;
    color: var(--text-secondary);
  }
  .line + .line { margin-top: 3px; }
  .dim { color: var(--text-dim); }

  .claim-head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 3px;
  }
  .claim-kind {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .claim-name { font-weight: 600; color: var(--text); }
  .claim-desc { color: var(--text-secondary); overflow-wrap: anywhere; }

  /* UI-145 — the offer that stands where a claim would have been. Dashed,
     because what it points at does not exist yet. */
  .add-spec {
    padding: 2px 8px;
    background: none;
    border: 1px dashed var(--border);
    border-radius: 4px;
    font: inherit;
    font-size: 0.72rem;
    color: var(--text-secondary);
    cursor: pointer;
    text-align: left;
  }
  .add-spec:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  /* The metric vocabulary is EntityInfo's, deliberately: the two panels sit
     in the same column one hover apart, and a folder's numbers reading in a
     different visual language would suggest they are a different kind of
     measurement. Svelte scopes styles per component, so the four tier rules
     are restated rather than shared. */
  .metrics-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(72px, 1fr));
    gap: 5px;
  }

  .metric {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 4px 6px;
    border-radius: 3px;
    border: 1px solid var(--border);
    background: var(--bg-surface-alt);
  }

  .metric-label {
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-dim);
  }

  .metric-value {
    font-size: 0.86rem;
    font-weight: 600;
    color: var(--text);
  }

  .metric-ok { background: rgba(76, 175, 80, 0.12); border-color: rgba(76, 175, 80, 0.4); }
  .metric-warn { background: rgba(255, 152, 0, 0.15); border-color: rgba(255, 152, 0, 0.5); }
  .metric-bad { background: rgba(244, 67, 54, 0.15); border-color: rgba(244, 67, 54, 0.5); }
  .metric-na { background: rgba(158, 158, 158, 0.1); border-color: rgba(158, 158, 158, 0.3); }

  .score-vs {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 8px;
    margin-top: 6px;
    font-size: 0.72rem;
    cursor: help;
  }
  .vs-verdict { font-weight: 600; color: var(--text-muted); }
  .score-vs.worse .vs-verdict { color: #E57373; }
  .score-vs.better .vs-verdict { color: #81C784; }

  .empty {
    margin: 12px 0 0;
    color: var(--text-dim);
    font-style: italic;
    font-size: 0.78rem;
  }

  .hint {
    margin-top: 14px;
    font-size: 0.7rem;
    color: var(--text-dim);
  }
</style>
