<script lang="ts">
  /**
   * The change as git reports it: one row per file, the way a source-control
   * pane lists them (UI-134).
   *
   * The canvas draws a diff of *entities*, which is the reading worth having
   * and is also unable to check itself — a file the analysis never loaded has
   * no row to be missing from, so "outside the configured languages" and
   * "nothing changed" reach the reader as the same silence. This list comes
   * from git, and every row says what the graph does with it, so the two can
   * be compared instead of trusted.
   *
   * It measures the whole change, always. The facet chips (UI-149) narrow
   * which rows are *drawn* and nothing else: the header keeps counting every
   * file git reported, and the residue keeps listing every disagreement. An
   * instrument that changed what it measures would not be one.
   *
   * What a click costs the canvas is the reader's to decide (UI-152). It has
   * always made the file's node the graph's *selection*, which re-roots the
   * tree on it and takes the rest of the change out of the picture — paid by
   * everyone who clicked a row to read its diff. `Filter on click` makes that
   * half optional, named as the spec pane names the same switch; off, the click
   * still opens the diff and still marks the row, and the file's circles are
   * *lit* instead, which answers where it sits without deleting what it sits
   * in.
   *
   * `Tree` folds the same rows into the folders they happened in (UI-154). The flat
   * list answers what changed and makes the reader reconstruct *where* from
   * fifty dimmed path prefixes; the tree answers it once, and gives every
   * folder in the change a control that points the canvas at it — which is the
   * question a shape like "three files, all under `ui/src/stores`" immediately
   * raises. The grouping is in `viewmodels/changeTree.ts`, the rows in
   * `ChangeRow.svelte`, which both readings draw so neither can drift.
   */
  import { diffData, DIFF_COLORS, knownCommits, stashes, shownFromRef } from '../stores/diff';
  import { refLabel } from '../viewmodels/refLabel';
  import {
    changedFilesReading, changedFilesError, selectedChangedFile, changeFilter,
  } from '../stores/changedFiles';
  import { changesAsTree, filterOnChangeClick } from '../stores/panes';
  import { graphData } from '../stores/graph';
  import { holdersOf } from '../viewmodels/changeHighlight';
  import { changeFacets, filterRows, toggleFacet } from '../viewmodels/changeFacets';
  import { buildChangeTree } from '../viewmodels/changeTree';
  import ChangeRow from './ChangeRow.svelte';
  import ChangeTreeNode from './ChangeTreeNode.svelte';

  /** How many circles the open row is lighting. Read off the *open* row and
   *  never the hover, so sweeping the list does not make this number flicker
   *  under the reader's own pointer — and so the pane can say "nothing on the
   *  canvas holds this file" as a fact about what they clicked. */
  $: openLit = $selectedChangedFile
    ? holdersOf([$selectedChangedFile.path], $graphData.nodes).size
    : 0;

  /** Every kind in the comparison, whether or not the filter is hiding it —
   *  the chips are how a reader sees what a change is *made of*, so they are
   *  counted over all the rows rather than the shown ones. */
  $: facets = changeFacets($changedFilesReading?.rows ?? [], $changeFilter);
  $: shown = filterRows($changedFilesReading?.rows ?? [], $changeFilter);
  /** Built from the *shown* rows, so a folder never badges a count that
   *  includes files the facet chips are hiding. */
  $: tree = buildChangeTree(shown);
</script>

<div class="changed-files" data-probe="changed-files">
  {#if !$diffData}
    <p class="empty">
      No comparison is loaded. Pick one from <strong>Diff</strong> above the
      canvas and the files it touched will be listed here.
    </p>
  {:else if $changedFilesError}
    <p class="error">{$changedFilesError}</p>
  {:else if !$changedFilesReading}
    <p class="empty">Asking git what changed…</p>
  {:else}
    {@const reading = $changedFilesReading}
    <!-- Hash plus subject, not the hash alone (UI-139): a hash is a name the
         reader has to go and look up, and the list they would look it up in is
         the one this app already holds. `knownCommits` rather than the
         picker's current listing, because a comparison across branches records
         two shas from two histories and only one of them can be in the list on
         screen (UI-143). -->
    <!-- `shownFromRef`, not `from_ref`: a range picked in the commit picker
         is sent as the *parent* of the oldest commit the reader wanted, so
         `diff.json` echoes back a hash and a subject they never chose
         (UI-151). The two differ by one commit, and only for that pick. -->
    {@const from = refLabel($shownFromRef, $knownCommits, $stashes)}
    {@const to = refLabel($diffData.to_ref, $knownCommits, $stashes)}
    <header class="summary">
      <span class="refs" title={`The pair this list and the canvas both describe.\n\nFrom — ${from.title}\n\nTo — ${to.title}`}>
        <span class="ref" class:live={from.live}>{from.text}</span>
        <span class="arrow">→</span>
        <span class="ref" class:live={to.live}>{to.text}</span>
      </span>
      <span class="counts">
        <span class="files">{reading.totals.files} {reading.totals.files === 1 ? 'file' : 'files'}</span>
        <span class="add">+{reading.totals.additions}</span>
        <span class="del">−{reading.totals.deletions}</span>
      </span>
    </header>

    <!-- What the click costs the canvas (UI-152), the switch the spec pane
         already has under the name it has there. Above the chips because it
         is about the whole list rather than about which rows are in it. -->
    <div class="modes">
      <button
        type="button"
        class="facet"
        class:on={$filterOnChangeClick}
        data-probe="changes-filter-on-click"
        aria-pressed={$filterOnChangeClick}
        title={'Whether clicking a row also makes its file the graph’s selection, which re-roots the canvas on it.\n\nOff, a click still opens the diff and still marks the row — the file’s circles are lit instead, so you can see where it sits without losing the rest of the change.'}
        on:click={() => filterOnChangeClick.update((v) => !v)}
      >Filter on click</button>
      <!-- The same rows, folded into the folders they happened in. A view
           switch and nothing more: it hides no file, changes no count, and
           the facet chips keep narrowing both readings alike. -->
      <button
        type="button"
        class="facet"
        class:on={$changesAsTree}
        data-probe="changes-tree-toggle"
        aria-pressed={$changesAsTree}
        title={'Group the changed files under the folders they live in, so the shape of the change is visible at a glance.\n\nEvery folder row carries a target that points the canvas at that folder — hold shift to add it to the current scope instead of replacing it.'}
        on:click={() => changesAsTree.update((v) => !v)}
      >Tree</button>
      <!-- Said whether or not the light is the only channel running: the row
           carries a `not drawn` / `not analysed` verdict of its own, and this
           is the same fact counted on the canvas rather than in the list. A
           reader looking for a ring that is not there is owed the reason. -->
      {#if $selectedChangedFile}
        <span class="lit" data-probe="changes-lit">
          <span class="swatch" aria-hidden="true"></span>
          {#if openLit > 0}
            {openLit} lit
          {:else}
            not drawn here
          {/if}
        </span>
      {/if}
    </div>

    <!-- One chip per kind of change in the comparison (UI-149). Multi-select,
         because "what have I added or not yet tracked" is one question and
         asking it as two clicks is the reason a single-select would be worse
         than none. Nothing selected is everything shown. -->
    {#if facets.length > 0}
      <div class="facets" data-probe="change-facets" role="group" aria-label="Filter by kind of change">
        {#each facets as facet (facet.letter)}
          <button
            type="button"
            class="facet"
            class:on={facet.selected}
            data-probe="change-facet"
            data-letter={facet.letter}
            aria-pressed={facet.selected}
            title={`${facet.selected ? 'Stop showing only' : 'Show only'} ${facet.phrase} files — ${facet.count} in this comparison.\n\nThe filter hides rows; it changes nothing about the comparison, the canvas or the counts above.`}
            on:click={() => changeFilter.update((s) => toggleFacet(s, facet.letter))}
          >
            <span class="facet-letter" style="color: {DIFF_COLORS[facet.change]}">{facet.letter}</span>
            <span class="facet-phrase">{facet.phrase}</span>
            <span class="facet-count">{facet.count}</span>
          </button>
        {/each}
        {#if $changeFilter.size > 0}
          <button
            type="button"
            class="facet clear"
            data-probe="change-facet-clear"
            title="Show every kind of change again"
            on:click={() => changeFilter.set(new Set())}
          >clear</button>
        {/if}
      </div>
    {/if}

    {#if reading.rows.length === 0}
      <p class="empty">Git reports no changed files for this comparison.</p>
    {:else if $changeFilter.size > 0}
      <!-- Said in words, and said whether or not the filter is hiding
           anything: the number above is the whole change and this one is what
           is on screen, and a reader who scrolls a list without being told it
           was narrowed has been given the wrong reading. -->
      <p class="narrowed" data-probe="changes-narrowed">
        Showing {shown.length} of {reading.rows.length}
        {reading.rows.length === 1 ? 'file' : 'files'}.
      </p>
    {/if}

    {#if $changesAsTree}
      <div class="tree" data-probe="changes-tree">
        {#each tree as node (node.kind + node.path)}
          <ChangeTreeNode {node} />
        {/each}
      </div>
    {:else}
      <ul class="rows">
        {#each shown as row (row.file.path)}
          <li><ChangeRow {row} /></li>
        {/each}
      </ul>
    {/if}

    {#if reading.onlyInGraph.length > 0}
      <!-- Empty on a healthy comparison. When it is not, the two readings
           disagree about what changed, and that is worth more than any row
           above it — a diff left over from a previous comparison, or a path
           spelling that stopped matching between diff.json and the tree. -->
      <section class="residue" data-probe="only-in-graph">
        <h3>Only in the graph</h3>
        <p>
          The diff reports changed entities in {reading.onlyInGraph.length}
          {reading.onlyInGraph.length === 1 ? 'file' : 'files'} git does not
          list for this pair.
        </p>
        <ul>
          {#each reading.onlyInGraph as path}
            <li title={path}>{path}</li>
          {/each}
        </ul>
      </section>
    {/if}
  {/if}
</div>

<style>
  .changed-files {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 0.8rem;
  }

  .empty, .error {
    color: var(--text-dim);
    line-height: 1.6;
    margin: 0;
  }
  .error { color: var(--text-secondary); }

  .summary {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
    color: var(--text-muted);
    font-size: 0.72rem;
  }
  .refs {
    display: flex;
    align-items: baseline;
    gap: 4px;
    min-width: 0;
    flex: 1 1 auto;
  }
  .ref {
    font-family: 'Monaco', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The working tree and the index move under the reader; a commit does not.
     Worth a different weight, since it is the difference between a reading
     that will still be true in a minute and one that will not. */
  .ref.live { font-family: inherit; font-style: italic; color: var(--text-secondary); }
  .arrow { flex: none; color: var(--text-dim); }
  .counts { display: flex; gap: 6px; }
  .files { color: var(--text-secondary); }

  /* Add/remove hues identify a state and stay put across themes — the same
     rule SourceDiff follows, and the reason these are literals. */
  .add { color: #4CAF50; }
  .del { color: #F44336; }

  .facets {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .facet {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    padding: 2px 6px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: 0.7rem;
    line-height: 1.4;
    cursor: pointer;
  }
  .facet:hover { background: var(--bg-hover); color: var(--text-secondary); }
  /* Selected reads as a filled chip rather than a coloured one: the letter
     already carries the diff hue, and a second colour on the same control
     would make the two compete for the same meaning. */
  .facet.on {
    background: var(--bg-surface-alt);
    border-color: var(--accent);
    color: var(--text);
  }
  .facet-letter {
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.68rem;
    font-weight: 700;
  }
  .facet-count { color: var(--text-dim); font-variant-numeric: tabular-nums; }
  .facet.on .facet-count { color: var(--text-secondary); }
  .facet.clear { color: var(--text-dim); font-style: italic; }

  .modes {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .lit {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 0.7rem;
    color: var(--text-dim);
  }
  /* The halo the canvas puts on a lit circle, at 10px. Not theme-derived for
     the reason the diff hues are not: it identifies a channel, and it has to
     go on meaning the same thing in all four themes. */
  .swatch {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #7C4DFF;
    box-shadow: 0 0 4px rgba(124, 77, 255, 0.9);
  }

  .narrowed {
    margin: 0;
    font-size: 0.7rem;
    color: var(--text-dim);
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }

  /* The tree draws its own rows; what belongs here is only the block they
     sit in, so the two readings share the pane's rhythm. */
  .tree {
    display: flex;
    flex-direction: column;
  }

  .residue {
    border-top: 1px solid var(--border);
    padding-top: 8px;
    color: var(--text-secondary);
  }
  .residue h3 {
    margin: 0 0 4px;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }
  .residue p { margin: 0 0 4px; font-size: 0.72rem; color: var(--text-dim); }
  .residue ul {
    margin: 0;
    padding-left: 14px;
    font-size: 0.72rem;
    font-family: 'Monaco', 'Menlo', monospace;
  }
  .residue li {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
