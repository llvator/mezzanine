<script lang="ts">
  import { onMount } from 'svelte';
  import ColorChip from './ColorChip.svelte';
  /**
   * Parent filter that controls *what the analyzer parses*. Distinct
   * from the visual Languages filter — toggling a checkbox here doesn't
   * just hide nodes, it stages a re-analysis. Apply commits the change:
   * the backend cancels any in-flight run and re-parses with the new
   * language set, then the SSE reload refreshes the graph.
   *
   * Default state is "no filter" (analyze everything). The user opts
   * into a narrower scope, clicks Apply, and the visual filters below
   * automatically re-seed from whatever the backend produced.
   */
  import {
    ALL_ANALYSIS_LANGUAGES,
    DEFAULT_ANALYSIS_LANGUAGES,
    stagedAnalysisLanguages,
    stagedIncludeDocs,
    setStagedIncludeDocs,
    loadAnalysisScope,
    OPT_IN_ANALYSIS_LANGUAGES,
    appliedAnalysisLanguages,
    analysisScopeDirty,
    analysisScopeApplying,
    analysisScopeError,
    toggleStagedLanguage,
    setAllStagedLanguages,
    stagedScopeEmpty,
    resetStaged,
    applyAnalysisScope,
    stagedSpecDir,
    appliedSpecDir,
    analysisScopeSaving,
    analysisScopeSaved,
    saveScopeAsRepoDefault,
  } from '../stores/analysisScope';
  import { serveMode } from '../stores/serveMode';
  import { LANGUAGE_COLORS } from '../types/graph';

  // A language is "checked" when either no filter is active (null =
  // everything) or it's in the staged set.
  function isChecked(staged: Set<string> | null, lang: string): boolean {
    return staged === null || staged.has(lang);
  }

  function onToggle(lang: string, e: Event) {
    const checked = (e.currentTarget as HTMLInputElement).checked;
    toggleStagedLanguage(lang, checked);
  }

  // Master toggle. Checked = everything staged (null, or a set that happens
  // to hold every language); indeterminate = a partial selection, so the box
  // doesn't claim "all" or "none" while the truth is neither.
  // `null` is the server default, which is *not* every language — it leaves
  // out the opt-in ones (Markdown). Counting it as all of them made the panel
  // report "All languages" over a scope that excluded docs.
  $: stagedCount = $stagedAnalysisLanguages === null
    ? DEFAULT_ANALYSIS_LANGUAGES.length
    : $stagedAnalysisLanguages.size;
  $: allStaged = stagedCount === ALL_ANALYSIS_LANGUAGES.length;
  $: someStaged = stagedCount > 0 && !allStaged;

  // Summary line: "All languages" when applied = null, otherwise the
  // sorted list. Helps the user remember what's *currently* analyzed
  // without scanning checkboxes.
  $: appliedSummary = $appliedAnalysisLanguages === null
    ? 'All languages'
    : [...$appliedAnalysisLanguages].sort().join(', ') || 'None';

  // Shown whether or not it is set, unlike the language summary above it.
  // The setting was invisible while unset — a reader with no spec folder saw
  // no line, so the field inside the collapsed section was findable only by
  // opening a section whose heading did not mention it. Naming the default
  // out loud is what says "there is a control here".
  $: specSummary = $appliedSpecDir.trim() || 'every .elv under the root';
  /** Collapsed by default in the sidebar: nineteen checkboxes that are all
   *  ticked and almost never changed were consuming ~175px above the scope
   *  tree (UI-011). The summary line stays visible either way, so the
   *  current state is never hidden — only the controls are. */
  export let open = false;

  // Seed from the server rather than assuming. A panel that guesses "no
  // filter, no docs" would, on its first Apply, send that guess as fact.
  onMount(loadAnalysisScope);

  /** An opt-in language is analyzed when the docs switch is on, whatever the
   *  list says — the backend ORs the two (`accepts_language`). Showing its
   *  box unticked while it is being analyzed would be the same class of lie
   *  the switch exists to end. */
  function impliedByDocs(lang: string, docsOn: boolean): boolean {
    return docsOn && OPT_IN_ANALYSIS_LANGUAGES.includes(lang);
  }
</script>

<div class="analysis-scope">
  <div class="summary">
    Analyzing: <span class="summary-value">{appliedSummary}</span>
  </div>
  <div class="summary">
    Spec from: <span class="summary-value">{specSummary}</span>
  </div>

  {#if open}
  <label class="checkbox-item select-all">
    <input
      type="checkbox"
      checked={allStaged}
      indeterminate={someStaged}
      disabled={$analysisScopeApplying}
      on:change={(e) => setAllStagedLanguages(e.currentTarget.checked)}
    />
    <span>{allStaged ? 'All languages' : someStaged ? `${stagedCount} of ${ALL_ANALYSIS_LANGUAGES.length}` : 'None'}</span>
  </label>

  <label class="checkbox-item docs-switch">
    <input
      type="checkbox"
      checked={$stagedIncludeDocs}
      disabled={$analysisScopeApplying}
      on:change={(e) => setStagedIncludeDocs(e.currentTarget.checked)}
    />
    <span>Include documentation</span>
  </label>
  <p class="layer-note">
    Markdown notes and the links between them, added to whatever is selected
    below. Off by default — a repo's READMEs and ADRs would otherwise
    outnumber its code.
  </p>

  <div class="checkbox-group">
    {#each ALL_ANALYSIS_LANGUAGES as lang}
      <label class="checkbox-item" class:implied={impliedByDocs(lang, $stagedIncludeDocs)}>
        <input
          type="checkbox"
          checked={isChecked($stagedAnalysisLanguages, lang) || impliedByDocs(lang, $stagedIncludeDocs)}
          disabled={$analysisScopeApplying || impliedByDocs(lang, $stagedIncludeDocs)}
          title={impliedByDocs(lang, $stagedIncludeDocs)
            ? 'Included by "Include documentation"'
            : undefined}
          on:change={(e) => onToggle(lang, e)}
        />
        <ColorChip color={LANGUAGE_COLORS[lang] || 'var(--text-muted)'} label={lang} />
      </label>
    {/each}
  </div>

  <label class="spec-dir" for="spec-dir-input">Spec folder</label>
  <input
    id="spec-dir-input"
    class="spec-dir-input"
    type="text"
    spellcheck="false"
    autocomplete="off"
    placeholder="every .elv under the root"
    disabled={$analysisScopeApplying}
    bind:value={$stagedSpecDir}
  />
  <p class="layer-note">
    Where this repo's Elevator spec lives — relative to the analyzed root, or
    an absolute path when the spec sits outside it. Set it and only `.elv`
    files under that folder are the spec; leave it empty and every `.elv` in
    the tree is. Applies to this session; the durable answer is
    <code>spec_dir</code> in <code>.nao/settings.json</code>.
  </p>

  <div class="actions">
    <button
      type="button"
      class="apply-btn"
      disabled={!$analysisScopeDirty || $analysisScopeApplying || $stagedScopeEmpty}
      on:click={applyAnalysisScope}
      title={$stagedScopeEmpty
        ? 'Pick at least one language to analyze'
        : 'Re-analyze with the staged language set'}
    >
      {#if $analysisScopeApplying}
        Analyzing…
      {:else}
        Apply
      {/if}
    </button>
    <button
      type="button"
      class="cancel-btn"
      disabled={!$analysisScopeDirty || $analysisScopeApplying}
      on:click={resetStaged}
      title="Discard staged changes"
    >
      Reset
    </button>
    <!-- Saves what is *applied*, so it is disabled while the panel is dirty:
         a button that quietly saved something the reader had not yet seen
         drawn would be a second Apply. Absent in serve mode, where a repo
         arrived from a URL a stranger pasted and its settings file is never
         read (ADR-0008). -->
    {#if !$serveMode}
      <button
        type="button"
        class="save-btn"
        disabled={$analysisScopeDirty || $analysisScopeApplying || $analysisScopeSaving}
        on:click={saveScopeAsRepoDefault}
        title={$analysisScopeDirty
          ? 'Apply the staged scope first — this saves what is currently drawn'
          : "Write this scope into .nao/settings.json so it survives a restart"}
      >
        {#if $analysisScopeSaving}
          Saving…
        {:else if $analysisScopeSaved && !$analysisScopeDirty}
          Saved ✓
        {:else}
          Save as this repo's default
        {/if}
      </button>
    {/if}
  </div>

  {#if $analysisScopeError}
    <div class="error">{$analysisScopeError}</div>
  {/if}
  {/if}
</div>

<style>
  .analysis-scope {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .summary {
    font-size: 0.75rem;
    color: var(--text-muted, #888);
  }
  .summary-value {
    color: var(--text-secondary, #ccc);
    font-weight: 500;
  }

  .checkbox-group {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .checkbox-item {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.8rem;
    padding: 3px 6px;
    background: var(--bg-body);
    border-radius: 4px;
    cursor: pointer;
  }
  .checkbox-item:hover { background: var(--bg-hover); }
  .checkbox-item input[disabled] { cursor: not-allowed; }

  /* The docs switch governs a layer, not a language, so it sits apart from
     the grid rather than becoming a 22nd chip in it. */
  .docs-switch {
    align-self: flex-start;
    background: transparent;
    color: var(--text-secondary);
    font-weight: 500;
  }

  .layer-note {
    margin: 0;
    font-size: 0.7rem;
    line-height: 1.35;
    color: var(--text-muted, #888);
  }
  .layer-note code {
    font-size: 0.95em;
    color: var(--text-secondary);
  }

  /* A path, not a toggle: the one control here that needs typing, so it gets
     a label of its own rather than sitting inline with the checkboxes. */
  .spec-dir {
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .spec-dir-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.3rem 0.4rem;
    font-family: inherit;
    font-size: 0.75rem;
    color: var(--text);
    background: var(--bg-body);
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .spec-dir-input::placeholder { color: var(--text-dim); }
  .spec-dir-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .spec-dir-input:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Ticked because the docs switch says so, not because the user picked it.
     Dimmed so "on but not yours to change here" is visible rather than
     something the reader has to discover by clicking. */
  .checkbox-item.implied {
    opacity: 0.65;
    cursor: default;
  }

  /* Master toggle: full-width and slightly louder than the per-language
     rows so it reads as governing them rather than as an 18th language. */
  .select-all {
    align-self: flex-start;
    background: transparent;
    color: var(--text-secondary);
    font-weight: 500;
  }

  .actions {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }

  .apply-btn,
  .cancel-btn,
  .save-btn {
    padding: 0.3rem 0.6rem;
    border-radius: 3px;
    cursor: pointer;
    font-size: 0.8rem;
    border: 1px solid var(--border);
  }

  /* Quieter than Apply: this one writes a file rather than changing what is
     on screen, and it is the rarer of the two actions. Full width on its own
     row because the label is a sentence. */
  .save-btn {
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    flex-basis: 100%;
  }
  .save-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text);
    border-color: var(--accent);
  }
  .save-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .apply-btn {
    background: var(--accent);
    border-color: transparent;
    color: var(--accent-fg);
    flex: 1;
  }
  .apply-btn:hover:not(:disabled) { filter: brightness(1.1); }
  .apply-btn:disabled {
    background: var(--bg-surface-alt);
    color: var(--text-disabled, #777);
    cursor: not-allowed;
  }

  .cancel-btn {
    background: var(--bg-surface-alt);
    color: var(--text-secondary, #888);
  }
  .cancel-btn:hover:not(:disabled) {
    background: var(--bg-hover, #333);
    color: var(--text, #e0e0e0);
  }
  .cancel-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error {
    padding: 0.3rem 0.5rem;
    background: rgba(244, 67, 54, 0.15);
    border: 1px solid #f44336;
    border-radius: 3px;
    color: #ef5350;
    font-size: 0.75rem;
  }
</style>
