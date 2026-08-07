<script lang="ts">
  /**
   * What `.nao/settings.json` resolved to, and where each value came from.
   *
   * Read-only on purpose. The three tiers are not decoration — they are what
   * decides whether a key can be edited from a browser at all:
   *
   *  - analysis  — editable, but only by re-parsing. The Analysis Scope panel
   *                owns that, and can save its result back here.
   *  - view      — the filter panel and saved views already set these. A
   *                second control for one value is how "why does the graph
   *                look like this?" stops having an answer.
   *  - process   — consumed at startup. An editable port field on a page
   *                served over that port is a trap.
   */
  import { onMount } from 'svelte';
  import {
    settingsReport,
    settingsReportError,
    loadSettingsReport,
    rowsOf,
    warningsFor,
    generalWarnings,
    displayValue,
    ORIGIN_LABEL,
    ORIGIN_TITLE,
    TIER_TITLE,
    TIER_BLURB,
    type Tier,
  } from '../stores/settingsReport';

  const TIERS: Tier[] = ['analysis', 'view', 'process'];

  onMount(loadSettingsReport);
</script>

<div class="setting-section">
  <h2>Settings file</h2>
  <p class="section-desc">
    What nao resolved before drawing anything, and which of the four sources
    decided each value. Precedence runs command line → environment → this repo
    → your settings → default; the first one that says something wins.
  </p>

  {#if $settingsReportError}
    <p class="load-error">Could not read the settings — {$settingsReportError}</p>
  {:else if !$settingsReport}
    <p class="section-desc">Reading…</p>
  {:else}
    <div class="files">
      {#if $settingsReport.repo_scope_read}
        <div class="file-row">
          <span class="file-label">This repo</span>
          <code class:missing={!$settingsReport.repo_exists}>{$settingsReport.repo_path}</code>
          {#if !$settingsReport.repo_exists}<span class="absent">no file yet</span>{/if}
        </div>
      {:else}
        <div class="file-row">
          <span class="file-label">This repo</span>
          <span class="absent">
            not read — a repo submitted to <code>nao serve</code> arrived from a
            URL someone pasted, so it does not get to configure this server.
          </span>
        </div>
      {/if}
      <div class="file-row">
        <span class="file-label">Your settings</span>
        {#if $settingsReport.user_path}
          <code class:missing={!$settingsReport.user_exists}>{$settingsReport.user_path}</code>
          {#if !$settingsReport.user_exists}<span class="absent">no file yet</span>{/if}
        {:else}
          <span class="absent">no home directory — nothing machine-wide applies</span>
        {/if}
      </div>
    </div>

    {#each generalWarnings($settingsReport) as w}
      <div class="warning {w.severity}">
        <strong>{w.file}</strong>
        <span>{w.message}</span>
      </div>
    {/each}

    {#each TIERS as tier}
      <div class="tier">
        <h3>{TIER_TITLE[tier]}</h3>
        <p class="tier-blurb">{TIER_BLURB[tier]}</p>

        {#each rowsOf($settingsReport, tier) as row (row.key)}
          <div class="row">
            <div class="row-head">
              <code class="key">{row.key}</code>
              <span class="value" class:unset={row.value === null}>
                {displayValue(row.value)}
              </span>
              <span class="badges">
                {#each row.sources as source}
                  <span class="badge {source}" title={ORIGIN_TITLE[source]}>
                    {ORIGIN_LABEL[source]}
                  </span>
                {/each}
              </span>
            </div>

            {#if row.note}
              <p class="note">{row.note}</p>
            {/if}

            {#if row.entries?.length}
              <ul class="entries">
                {#each row.entries as entry}
                  <li>
                    <code>{entry.value}</code>
                    <span class="badge {entry.source}" title={ORIGIN_TITLE[entry.source]}>
                      {ORIGIN_LABEL[entry.source]}
                    </span>
                  </li>
                {/each}
              </ul>
            {/if}

            {#each warningsFor($settingsReport, row.key) as w}
              <div class="warning {w.severity}">
                <strong>{w.file}</strong>
                <span>{w.message}</span>
              </div>
            {/each}
          </div>
        {/each}
      </div>
    {/each}
  {/if}
</div>

<style>
  h2 {
    font-size: 1rem;
    margin: 15px 0 6px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 5px;
    color: var(--accent);
  }

  h3 {
    font-size: 0.82rem;
    margin: 16px 0 2px;
    color: var(--text-secondary);
  }

  .section-desc {
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-bottom: 14px;
  }

  .tier-blurb {
    font-size: 0.72rem;
    color: var(--text-dim);
    margin: 0 0 8px;
    line-height: 1.4;
  }

  .load-error {
    font-size: 0.75rem;
    color: var(--danger-fg);
  }

  .files {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-surface-alt);
  }

  .file-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 0.72rem;
  }

  .file-label {
    color: var(--text-dim);
    min-width: 5.5rem;
  }

  /* The file paths and the keys are the strings a reader retypes into an
     editor, so they stay monospace and selectable rather than prose. */
  code {
    font-family: inherit;
    color: var(--text-secondary);
    word-break: break-all;
  }

  code.missing {
    color: var(--text-disabled);
  }

  .absent {
    font-size: 0.7rem;
    color: var(--text-dim);
  }

  .row {
    padding: 6px 10px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    margin-bottom: 4px;
    background: var(--bg-body);
  }

  .row-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }

  .key {
    font-size: 0.78rem;
    color: var(--text);
    min-width: 9rem;
  }

  .value {
    flex: 1;
    min-width: 6rem;
    font-size: 0.75rem;
    color: var(--text-secondary);
    word-break: break-word;
  }

  .value.unset {
    color: var(--text-disabled);
  }

  .badges {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .badge {
    padding: 1px 6px;
    border-radius: 8px;
    font-size: 0.65rem;
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    white-space: nowrap;
  }

  /* The common case should not shout: a value nobody set is the least
     interesting thing on the row. */
  .badge.default {
    color: var(--text-disabled);
  }

  .badge.repo-file,
  .badge.flag {
    color: var(--accent);
  }

  .note {
    margin: 4px 0 0;
    font-size: 0.68rem;
    color: var(--text-dim);
    line-height: 1.4;
  }

  .entries {
    margin: 6px 0 0;
    padding: 0 0 0 14px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .entries li {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 0.7rem;
  }

  .warning {
    margin-top: 6px;
    padding: 5px 8px;
    border-radius: 4px;
    border-left: 2px solid var(--tier-warn-fg);
    background: var(--bg-surface-alt);
    font-size: 0.7rem;
    line-height: 1.4;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .warning strong {
    color: var(--text-dim);
    font-weight: normal;
    word-break: break-all;
  }

  .warning span {
    color: var(--text-secondary);
  }

  /* A repo asking for a capability is a different event from a typo, and the
     reader being attacked is the one who most needs to notice. */
  .warning.rejected,
  .warning.malformed {
    border-left-color: var(--danger-fg);
  }

  .warning.rejected span,
  .warning.malformed span {
    color: var(--danger-fg);
  }
</style>
