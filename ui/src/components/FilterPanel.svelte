<script lang="ts">
  import ColorChip from './ColorChip.svelte';
  import { NODE_COLORS, LINK_COLORS, LANGUAGE_COLORS } from '../types/graph';
  import { showGhostNodes, showBuiltinGhosts, structureOnly } from '../stores/graph';
  import {
    generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming,
    generalLanguages, allLanguages,
    levelOverrides, allEntityTypes, allRelTypes,
    toggleEntityType, toggleRelType, toggleLanguage, setAllLanguages,
    toggleLevelEnabled,
    cycleEntityTypeTriState, cycleRelTypeTriState, cycleDirectionTriState,
    searchTerm, commitAllMatches,
    searchInEntityNames, searchInFileNames, searchInFolderNames,
    searchEntityKinds, toggleSearchEntityKind, clearSearchEntityKinds,
    toggleLevelPeerEdges, showDirectEdges, showCrossLevelEdges,
  } from '../viewmodels/filterViewModel';
  // Display-search machinery now lives in displayPlan.ts (see comment in
  // filterViewModel.ts) — import directly to avoid a startup-time module cycle.
  import { displayPlan } from '../viewmodels/displayPlan';
  import { selectedNode, graphData, graphLevel, expandedScopes, collapseAllScopes, viewMode, shapePicture } from '../stores/graph';
  import {
    SHAPE_EDGE_COLORS,
    SHAPE_VERDICT_ORDER,
    VERDICT_TEXT,
    isViolation,
  } from '../viewmodels/shapeView';
  import { searchFocusRequest } from '../stores/keymap';
  import { get } from 'svelte/store';
  import { tick } from 'svelte';
  import type { D3Node, TriState } from '../types/graph';

  /** What `structureOnly` is holding back, and whose body is currently open.
   *  Counted off the drawn graph so the control can say nothing on a dataset
   *  with no bodies in it — a schema, a spec, a folder of notes. UI-113. */
  $: bodyEntities = $graphData.nodes.filter((n) => n.body_of !== undefined).length;
  $: openBody =
    $selectedNode && $graphData.nodes.some((n) => n.body_of === $selectedNode!.original_id)
      ? $selectedNode.name
      : null;
  import FileTree from './FileTree.svelte';
  import EntitySearchResults from './EntitySearchResults.svelte';
  import DisplaySearchResults from './DisplaySearchResults.svelte';
  import { visibleSearchResults } from '../viewmodels/searchResults';
  import ScopeTree from './ScopeTree.svelte';
  import RootPathPicker from './RootPathPicker.svelte';
  import AnalysisScopePanel from './AnalysisScopePanel.svelte';
  import SpecFilterSection from './SpecFilterSection.svelte';
  import SavedViewsSection from './SavedViewsSection.svelte';
  import { serveMode, activeRepo, backToPicker } from '../stores/serveMode';
  import { nodeEncoding, availableSizeChannels } from '../stores/encoding';
  import { sizeChannel, colorChannel, sizeCurve, sizeBoost, sizeBins, folderCohesion, showFolderHulls, hullDepth, HULL_DEPTHS, hullDepthLabels, groupGrain, demoteHubs, hubCount } from '../stores/settings';
  import { SIZE_CURVES, SIZE_BOOST_MIN, SIZE_BOOST_MAX, SIZE_BINS_MIN, SIZE_BINS_MAX, SIZE_BINS_OFF } from '../viewmodels/sizeCurve';
  import { HUB_COUNTS, hubNames } from '../viewmodels/hubs';
  import {
    COHESION_LEVELS, COHESION_LABELS,
    GROUP_GRAINS, GROUP_GRAIN_LABELS, groupGrainFor, type GroupGrain,
  } from '../utils/forceCohesion';
  import { NO_DATA_FILL, NO_DATA_FILL_OPACITY, COLOR_CHANNELS, R_MAX } from '../viewmodels/nodeEncoding';

  /** Names of the currently demoted hubs (UI-056), read off the plan rather
   *  than recomputed here — the panel must say exactly what the canvas did,
   *  and a second ranking could disagree with the first. */
  $: demotedNames = hubNames($graphData.nodes, [...$displayPlan.demotedHubIds]);

  /** The grain the canvas is actually grouping by (UI-103), which is the
   *  reader's choice only at Entity level. Everything the panel *says* — the
   *  hull checkbox's noun, the tier labels — follows this rather than the
   *  stored choice, or the sidebar would describe a grouping the canvas is
   *  not drawing. The pressed button still follows the choice, because that
   *  is what the reader picked and what returning to Entity level restores. */
  $: grainInForce = groupGrainFor($graphLevel, $groupGrain);
  $: depthLabels = hullDepthLabels(grainInForce);

  function grainTitle(grain: GroupGrain): string {
    if (grain === 'folder') return 'A region is the directory holding the file — the same grouping Folder level aggregates by';
    return $graphLevel === 'entity'
      ? 'A region is the file itself, so a region’s traffic says whether that file is cohesive'
      : 'Files group entities, so this applies at Entity level only — at this level every node already is a file or coarser';
  }

  /** Legend dots are drawn to scale with the canvas, shrunk to fit the
   *  sidebar: the largest stop gets a 22px radius, and every other stop the
   *  same factor. Three true-size dots would need ~180px of the ~220px of
   *  content width the panel has, and would make the legend the tallest
   *  block in it.
   *
   *  The denominator is the encoding's own top radius, not the `R_MAX`
   *  constant — since UI-106 the canvas range is a multiple of that constant,
   *  and a fixed denominator would let a ×3 legend overflow the panel while
   *  claiming to be to scale. Turning the scale up therefore keeps the
   *  largest dot at 22px and shrinks the smaller ones, which is exactly the
   *  discrimination it buys on the canvas. */
  $: legendDotScale = 22 / ($nodeEncoding.sizeLegend?.rMax ?? R_MAX);

  /** Group counts the picker offers. Two is the floor a "group" means
   *  anything at (big and small); above eight the classes are back to being
   *  finer than the eye separates, which is the continuous scale with extra
   *  steps. */
  const binChoices = Array.from(
    { length: SIZE_BINS_MAX - SIZE_BINS_MIN + 1 },
    (_, i) => SIZE_BINS_MIN + i,
  );

  /** Keep the size channel valid when the aggregation level changes under it:
   *  switching to File while size is WMC would otherwise leave a selected-but-
   *  unavailable channel, which silently falls back to kind sizing with no
   *  visible reason why. */
  $: if (!$availableSizeChannels.some((c) => c.id === $sizeChannel)) sizeChannel.set('loc');

  /** Master language toggle. Counted against `allLanguages` (the dataset)
   *  rather than the filter set, so a stale entry in `generalLanguages`
   *  can't make the box claim "all" while a row sits unchecked. */
  $: shownLangCount = $allLanguages.filter((l) => $generalLanguages.has(l)).length;
  $: langsAllShown = $allLanguages.length > 0 && shownLangCount === $allLanguages.length;
  $: langsSomeShown = shownLangCount > 0 && !langsAllShown;

  /** `owner__repo` → `owner/repo`; anything else passes through. */
  function displayName(slug: string): string {
    const parts = slug.split('__');
    return parts.length === 2 ? `${parts[0]}/${parts[1]}` : slug;
  }

  // Collapsed by default — a session-level setting, not a per-look control.
  let analysisScopeOpen = false;
  let filesOpen = false;
  let entitiesOpen = true;
  let relationshipsOpen = true;
  let searchScopeOpen = false;

  let searchInputEl: HTMLInputElement | undefined;
  let activeResult = -1;

  function selectMatch(node: D3Node) {
    selectedNode.set(node);
  }

  // Reset the keyboard cursor whenever the result set changes underneath it.
  $: if ($searchTerm !== undefined) activeResult = -1;

  /**
   * Arrow-key cursor over the ranked result list.
   *
   * Reads the same `visibleSearchResults` the list renders, so the cursor
   * walks the rows in the order they are shown. Out-of-scope hits are
   * skipped: `selectedNode` can only hold a node the loaded graph has, and
   * arrowing onto one would silently do nothing.
   */
  function onSearchKeydownNav(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      const rows = $visibleSearchResults.filter((r) => r.inScope);
      if (rows.length === 0) return;
      e.preventDefault();
      const delta = e.key === 'ArrowDown' ? 1 : -1;
      activeResult = (activeResult + delta + rows.length) % rows.length;
      selectMatch(rows[activeResult].node);
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      clearSearch();
      return;
    }
    onSearchKeydown(e);
  }

  function clearSearch() {
    searchTerm.set('');
    activeResult = -1;
    searchInputEl?.blur();
    // Hand focus back to the canvas so panning and zoom keys work again.
    (document.querySelector('.graph-container svg') as SVGElement | null)?.focus?.();
  }

  /**
   * Take the caret when the shortcut layer asks (`Cmd-K` anywhere, `/` in this
   * pane). The panel used to own that keystroke through its own window
   * listener; since UI-075 the keymap owns every key and this is the half of
   * the job no store can do — opening the disclosure and selecting the text.
   *
   * A counter rather than a flag, so two asks in a row are two events. `tick`
   * because the first ask may arrive with the section still closed, and the
   * input does not exist until it renders.
   */
  $: if ($searchFocusRequest > 0) grabSearchFocus();

  async function grabSearchFocus() {
    entitiesOpen = true;
    await tick();
    searchInputEl?.focus();
    searchInputEl?.select();
  }

  // Enter while focused on the search input commits every current match.
  // Shift+Enter is reserved for nothing yet but kept off the handler so a
  // future "add to selection" behavior can be layered without breaking.
  function onSearchKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      commitAllMatches();
    }
  }

  function cycleTriState(level: number, category: 'entityTypes' | 'relTypes', key: string) {
    if (category === 'entityTypes') cycleEntityTypeTriState(level, key);
    else cycleRelTypeTriState(level, key);
  }

  function triStateLabel(state: TriState): string {
    if (state === 'on') return '\u2713';
    if (state === 'off') return '\u2717';
    return 'G';
  }

  function toggleSection(id: string) {
    const el = document.getElementById(id);
    if (el) el.classList.toggle('open');
  }
</script>

{#if $serveMode && $activeRepo}
  <!-- Serve mode: the repo is a choice the user made, so name it and let
       them go back and change it. -->
  <div class="repo-bar">
    <button class="back" on:click={backToPicker} title="Back to the repo list">
      ← Repos
    </button>
    <span class="repo-name" title={$activeRepo.url ?? $activeRepo.slug}>
      {displayName($activeRepo.slug)}
    </span>
  </div>
{:else}
  <h1>Mezzanine</h1>
{/if}

<!--
  Three blocks, grouped by what a change costs — which is the thing the old
  flat list left to a comment nobody reading the app could see:

    Analysis            re-parses or refetches the dataset
      Root Path         which codebase
      Parsed Languages  which languages the analyzer reads at all
      Analysis Scope    which code loads into the canvas

    Graph Visualization redraws from the dataset already loaded
      Files             hide files from the picture only
      Languages         hide by language

    Post-Filtering      narrows or emphasises what is drawn
      Entities          search + entity kinds
      Relationships     direction + rel kinds
      Level Filters     traversal depth
      Legend            reference

  Names changed in UI-051: the old "Analysis Scope" filtered *languages* and
  is now "Parsed Languages"; the old bare "Scope" is now "Analysis Scope";
  "Files & Folders" is now the Graph Visualization block with "Files" inside.
-->

<!-- 0. Root Path: which codebase to analyze.
     0b. Analysis Scope: control what the analyzer even parses. Sits
     above every other filter because it shrinks the dataset; the
     visual filters below only hide nodes that the analyzer produced.

     Both re-run analysis on the server, which serve mode's repos don't
     support — they're analyzed once from a fixed checkout. Hidden there
     rather than left to fail against endpoints that don't exist. -->
<!-- Search is navigation, not a filter, and it must not drift below the
     fold as the scope tree grows — which is exactly what happened once the
     repo gained files (UI-016). Pinned above every section that can grow. -->
<div class="filter-section search-first">
  <div class="sub-title">
    <span class="search-label">Search in dataset</span>
    <span class="search-hint">— type to preview, Enter to filter</span>
  </div>
  <div class="filter-group">
    <input type="text"
      data-probe="search-input"
      bind:this={searchInputEl}
      bind:value={$searchTerm}
      on:keydown={onSearchKeydownNav}
      placeholder="Search entities  (⌘K)" />
  </div>
</div>

<!-- Block 1 of 3. The blocks are grouped by what a change *costs*, which is
     the distinction the flat list hid: everything here re-parses or refetches
     the dataset, everything in Graph Visualization redraws from the dataset
     already loaded, and everything in Post-Filtering narrows what is drawn. -->
<!-- Saved views (UI-082). Above block 1 because a view spans all three
     blocks — a scope from Analysis, a level and file exclusions from
     Visualization, the spec filter and a committed search from
     Post-Filtering. Inside any one of them it would read as a control over
     that block's settings, which is the one thing it is not. -->
<SavedViewsSection />

<div class="filter-block">
  <h2 class="block-title"><span class="block-step">1</span>Analysis</h2>
  <p class="block-note">What mezz parses and loads. Changing these re-runs analysis or refetches.</p>
</div>

{#if !$serveMode}
  <div class="filter-section">
    <h2>Root Path</h2>
    <RootPathPicker />
  </div>

  <div class="filter-section">
    <!-- A real <button> styled as the section header, rather than the
         click-handler-on-<h2> the sibling sections use. Same appearance,
         keyboard-reachable, and adds no a11y warnings. -->
    <h2 class="section-heading-wrap">
      <button
        type="button"
        class="section-header section-header-btn"
        aria-expanded={analysisScopeOpen}
        on:click={() => (analysisScopeOpen = !analysisScopeOpen)}
      >
        Parsed Sources <span class="toggle-arrow">{analysisScopeOpen ? '\u25BC' : '\u25B6'}</span>
      </button>
    </h2>
    <!-- Named for the section's contents rather than its first control. It
         held languages alone when it was called "Parsed Languages"; the spec
         folder then landed inside it and was unfindable, the heading having
         promised something narrower than what was there. -->
    <p class="layer-note">What the analyzer reads at all \u2014 languages, docs, and where the spec lives. Apply re-parses the repo.</p>
    <AnalysisScopePanel open={analysisScopeOpen} />
  </div>
{/if}

<!-- Still the most consequential control in the panel, and it used to be
     called just "Scope" next to a "Analysis Scope" that meant languages.
     The subtitle deliberately does NOT claim to move Quality: Quality reads
     `analysisScopes`, which defaults to the whole repo and is decoupled from
     this tree on purpose (scope.ts). An earlier version of this line said
     otherwise and was wrong (UI-051). -->
<div class="filter-section">
  <h2>Analysis Scope</h2>
  <p class="layer-note">Which code loads into the canvas. Quality measures its own population — see the Quality tab.</p>
  <ScopeTree />
</div>

<!-- Block 2 of 3. -->
<div class="filter-block">
  <h2 class="block-title"><span class="block-step">2</span>Graph Visualization</h2>
  <p class="block-note">What the canvas draws from the code above. The side panels keep the whole scope.</p>
</div>

<div class="filter-section">
  <h2 class="section-header" on:click={() => (filesOpen = !filesOpen)}>
    Files <span class="toggle-arrow">{filesOpen ? '\u25BC' : '\u25B6'}</span>
  </h2>
  <p class="layer-note">Hide files from the picture without changing what is analysed.</p>
  {#if filesOpen}
    <FileTree />
  {/if}
</div>

<!-- UI-057. The gesture has to be stated somewhere: nothing about a circle
     suggests it opens, and a reader who never finds out is stuck with one
     granularity for the whole canvas. It lives here rather than in the
     toolbar because the toolbar is capped at two rows (UI-013) and a ninth
     cluster pushed it to three — and because this is a Graph Visualization
     decision, which is the block it now sits in. -->
<div class="filter-section" data-probe="expansion">
  <h2>Expanded scopes</h2>
  <p class="layer-note">
    Shift + double-click a file or module on the canvas to open it in place.
    The rest of the view stays as it is.
  </p>
  {#if $expandedScopes.size > 0}
    <button
      type="button"
      class="seg-btn expand-collapse"
      data-probe="collapse-all"
      on:click={() => collapseAllScopes()}
    >Collapse {$expandedScopes.size} open {$expandedScopes.size === 1 ? 'scope' : 'scopes'}</button>
  {:else}
    <p class="layer-note" data-probe="expand-hint">Nothing is open.</p>
  {/if}
</div>

<!-- UI-052. Layout, not filtering — which is why it sits in Graph
     Visualization and not in Post-Filtering: it moves what is drawn, it
     never removes any of it. Nothing here refetches (ADR 0010). -->
<div class="filter-section" data-probe="cohesion">
  <h2 id="cohesion-label">Folder cohesion</h2>
  <p class="layer-note">How hard the folder tree pulls against the call graph. Off is the plain force layout.</p>
  <div class="seg-group" role="group" aria-labelledby="cohesion-label">
    {#each COHESION_LEVELS as lvl}
      <button
        type="button"
        class="seg-btn"
        class:active={$folderCohesion === lvl}
        aria-pressed={$folderCohesion === lvl}
        data-probe="cohesion-{lvl}"
        on:click={() => folderCohesion.set(lvl)}
      >{COHESION_LABELS[lvl]}</button>
    {/each}
  </div>
  <!-- UI-103. Which level of the declared tree is a group. Sits with cohesion
       rather than with the hulls because it changes what the FORCE groups by,
       not only which outlines get drawn — the outlines follow it, they do not
       own it.

       Entity level only. The button stays enabled and dims elsewhere,
       following the aggregation control (UI-090): at File level every node
       already is a file, so every file region would hold one member and draw
       nothing, and a reader who cannot press the control back through has
       been told less than one who can. -->
  <div class="seg-group" role="group" aria-label="Which level of the tree is a group">
    {#each GROUP_GRAINS as grain}
      <button
        type="button"
        class="seg-btn"
        class:active={$groupGrain === grain}
        class:grain-inert={grain !== grainInForce && $groupGrain === grain}
        aria-pressed={$groupGrain === grain}
        data-probe="group-grain-{grain}"
        title={grainTitle(grain)}
        on:click={() => groupGrain.set(grain)}
      >{GROUP_GRAIN_LABELS[grain]}</button>
    {/each}
  </div>
  <label class="checkbox-item">
    <input type="checkbox" bind:checked={$showFolderHulls} data-probe="hulls-toggle" />
    <span>Outline and name each {grainInForce === 'file' ? 'file' : 'folder'}</span>
  </label>
  <!-- UI-070. How much of the folder tree gets an outline. A separate control
       from the toggle rather than four states of one, because "should there be
       regions" and "how many tiers of them" are different questions and the
       first one has an established answer. Layout overlay only: it changes
       which outlines exist, never which nodes are drawn. -->
  {#if $showFolderHulls}
    <div class="seg-group" role="group" aria-label="How many tiers of the folder tree get an outline">
      {#each HULL_DEPTHS as d}
        <button
          type="button"
          class="seg-btn"
          class:active={$hullDepth === d}
          aria-pressed={$hullDepth === d}
          data-probe="hull-depth-{d}"
          on:click={() => hullDepth.set(d)}
        >{depthLabels[d]}</button>
      {/each}
    </div>
  {/if}

  <!-- UI-056. Hides edges, never entities — hence the wording, and hence the
       list below: a control that silently removed a node's edges would leave
       the reader believing nothing depends on it. -->
  <label class="checkbox-item">
    <input type="checkbox" bind:checked={$demoteHubs} data-probe="demote-toggle" />
    <span>Hide edges into the most-depended-on nodes</span>
  </label>
  {#if $demoteHubs}
    <div class="seg-group" role="group" aria-label="How many nodes to demote">
      {#each HUB_COUNTS as n}
        <button
          type="button"
          class="seg-btn"
          class:active={$hubCount === n}
          aria-pressed={$hubCount === n}
          data-probe="hub-count-{n}"
          on:click={() => hubCount.set(n)}
        >{n}</button>
      {/each}
    </div>
    <p class="layer-note" data-probe="demoted-list">
      {#if demotedNames.length}
        Demoted: {demotedNames.join(', ')}. They are still drawn and still
        selectable; only the arrows pointing at them are hidden.
      {:else}
        Nothing to demote in this view.
      {/if}
    </p>
  {/if}
  {#if $graphLevel === 'folder'}
    <p class="layer-note">No effect at Folder level — each node is already a folder.</p>
  {/if}
</div>

<!-- 3. Languages: narrow by source language -->
<div class="filter-section">
  <h2>Languages</h2>
  <label class="checkbox-item select-all">
    <input type="checkbox"
      checked={langsAllShown}
      indeterminate={langsSomeShown}
      on:change={(e) => setAllLanguages(e.currentTarget.checked)} />
    <span>{langsAllShown ? 'All' : langsSomeShown ? `${shownLangCount} of ${$allLanguages.length}` : 'None'}</span>
  </label>
  <div class="checkbox-group">
    {#each $allLanguages as lang}
      <label class="checkbox-item">
        <input type="checkbox" checked={$generalLanguages.has(lang)}
          on:change={(e) => toggleLanguage(lang, e.currentTarget.checked)} />
        <ColorChip color={LANGUAGE_COLORS[lang] || 'var(--text-muted)'} label={lang} />
      </label>
    {/each}
  </div>
</div>

<!-- Block 3 of 3. -->
<div class="filter-block">
  <h2 class="block-title"><span class="block-step">3</span>Post-Filtering</h2>
  <p class="block-note">Narrow or emphasise what is already drawn. Nothing here reloads data.</p>
</div>

<!-- Spec: narrow by what an Elevator entity declares. First in the block, and
     above Entities, because it filters by *meaning* rather than by shape — it
     is the one control here a reader reaches for knowing what they want rather
     than what it looks like. Renders nothing when the project has no `.elv`
     files. Its twin is the spec pane; either can drive the filter, and the
     filter outlives both (ADR 0011). -->
<SpecFilterSection />

<!-- Entities: search + entity-type filter. Search narrows entity
     visibility, so it belongs next to the entity-type toggles. -->
<div class="filter-section">
  <h2 class="section-header" on:click={() => (entitiesOpen = !entitiesOpen)}>
    Entities <span class="toggle-arrow">{entitiesOpen ? '\u25BC' : '\u25B6'}</span>
  </h2>
  {#if entitiesOpen}
    <!-- Primary search: two-phase workflow.
         · Type → live results list (preview). No graph filtering yet.
         · Enter or "Commit all" → all current matches filter the graph.
         · Per-row checkbox → individually commit/uncommit a match.
         · Clear the input → filter is removed. -->
    <!-- The field itself lives at the top of the panel; this section keeps
         the scope controls and the results list that go with it. -->

    <!-- Search scope: what the term is matched against. Collapsed by default
         since the all-on defaults match the old behavior. -->
    <div class="search-scope">
      <button type="button" class="search-scope-toggle"
        on:click={() => (searchScopeOpen = !searchScopeOpen)}>
        <span class="toggle-arrow">{searchScopeOpen ? '\u25BC' : '\u25B6'}</span>
        Search scope
        {#if $searchEntityKinds.size > 0}
          <span class="scope-badge">{$searchEntityKinds.size} kind{$searchEntityKinds.size === 1 ? '' : 's'}</span>
        {/if}
      </button>
      {#if searchScopeOpen}
        <div class="search-scope-body">
          <div class="sub-title">Match on</div>
          <div class="checkbox-group">
            <label class="checkbox-item">
              <input type="checkbox" bind:checked={$searchInEntityNames} />
              <span>Entity names</span>
            </label>
            <label class="checkbox-item">
              <input type="checkbox" bind:checked={$searchInFileNames} />
              <span>File names</span>
            </label>
            <label class="checkbox-item">
              <input type="checkbox" bind:checked={$searchInFolderNames} />
              <span>Folder names</span>
            </label>
          </div>

          <div class="sub-title">
            Restrict to entity kinds
            {#if $searchEntityKinds.size > 0}
              <button type="button" class="scope-clear-btn" on:click={clearSearchEntityKinds}>
                All kinds
              </button>
            {:else}
              <span class="search-hint">— empty = all kinds</span>
            {/if}
          </div>
          <div class="checkbox-group">
            {#each $allEntityTypes as type}
              <label class="checkbox-item">
                <input type="checkbox"
                  checked={$searchEntityKinds.has(type)}
                  on:change={() => toggleSearchEntityKind(type)} />
                <ColorChip color={NODE_COLORS[type] || 'var(--text-muted)'} label={type} />
              </label>
            {/each}
          </div>
        </div>
      {/if}
    </div>

    <!-- Mode toggle, status row and the ranked result list. Extracted to
         its own component: this file was already five times the ~300-line
         guideline, and the ranked list is where all the new query UI
         landed. -->
    <EntitySearchResults />

    <!-- The within-view search, its result list and the picks made in it.
         Extracted alongside `EntitySearchResults` and for the same reason:
         query UI belongs in its own component, and this list grew a second
         gesture (pick, as distinct from select) in UI-101. -->
    <DisplaySearchResults />

    <!-- Grain: declarations, or declarations and their insides. Above the
         kind checkboxes because that is where it applies, and because it is
         the control that makes most of them unnecessary — the reader who
         reaches for "untick Parameter, Branch, Loop" wants this instead, and
         gets to keep the calls made inside those branches. UI-113. -->
    {#if bodyEntities > 0}
      <div class="sub-title">Grain</div>
      <div class="checkbox-group">
        <label class="checkbox-item">
          <input type="checkbox" checked={$structureOnly}
            on:change={(e) => structureOnly.set(e.currentTarget.checked)} />
          <ColorChip color="#7E57C2" label="Structure only (hide function internals)" />
        </label>
        <div class="search-hint" style="padding-left: 22px;">
          {#if $structureOnly}
            {bodyEntities} inside a body, not drawn.
            {openBody ? `${openBody} is selected, so its own body is open.` : 'Select a function to open its body.'}
          {:else}
            Parameters, branch arms and loop bodies are drawn alongside what
            each file declares.
          {/if}
        </div>
      </div>
    {/if}

    <!-- Headings render nothing until a graph is loaded — an empty section
         title is pure vertical cost in a pane that has none to spare. -->
    {#if $allEntityTypes.length > 0}
      <div class="sub-title">Entity Types</div>
      <div class="checkbox-group">
        {#each $allEntityTypes as type}
          <label class="checkbox-item">
            <input type="checkbox" checked={$generalEntityTypes.has(type)}
              on:change={(e) => toggleEntityType(type, e.currentTarget.checked)} />
            <ColorChip color={NODE_COLORS[type] || 'var(--text-muted)'} label={type} />
          </label>
        {/each}
      </div>
    {/if}

    <div class="checkbox-group" style="margin-top: 6px; padding-top: 6px; border-top: 1px solid var(--border-subtle);">
      <label class="checkbox-item">
        <input type="checkbox" checked={$showGhostNodes}
          on:change={(e) => showGhostNodes.set(e.currentTarget.checked)} />
        <ColorChip color="#9E9E9E" label="Ghost nodes (external refs)" />
      </label>
      <label class="checkbox-item">
        <input type="checkbox" checked={$showBuiltinGhosts}
          on:change={(e) => showBuiltinGhosts.set(e.currentTarget.checked)} />
        <ColorChip color="#9E9E9E" label="Builtins (print, len, Vec, …)" />
      </label>
    </div>
  {/if}
</div>

<!-- 5. Relationships: how edges are drawn (direction + rel types) -->
<div class="filter-section">
  <h2 class="section-header" on:click={() => (relationshipsOpen = !relationshipsOpen)}>
    Relationships <span class="toggle-arrow">{relationshipsOpen ? '\u25BC' : '\u25B6'}</span>
  </h2>
  {#if relationshipsOpen}
    <div class="sub-title">Direction</div>
    <div class="checkbox-group">
      <label class="checkbox-item">
        <input type="checkbox" bind:checked={$generalOutgoing} />
        <ColorChip color="#2196F3" label="Outgoing" />
      </label>
      <label class="checkbox-item">
        <input type="checkbox" bind:checked={$generalIncoming} />
        <ColorChip color="#E91E63" label="Incoming" />
      </label>
    </div>

    {#if $allRelTypes.length > 0}
      <div class="sub-title">Relationship Types</div>
      <div class="checkbox-group">
        {#each $allRelTypes as type}
          <label class="checkbox-item">
            <input type="checkbox" checked={$generalRelTypes.has(type)}
              on:change={(e) => toggleRelType(type, e.currentTarget.checked)} />
            <ColorChip color={LINK_COLORS[type] || 'var(--text-muted)'} label={type} />
          </label>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<!-- Level Filters -->
<div class="filter-section">
  <h2>Level Filters</h2>
  <div class="level-hint">
    Tri-state: <span class="ts-g">G</span>=General, <span class="ts-on">{'\u2713'}</span>=On, <span class="ts-off">{'\u2717'}</span>=Off
  </div>

  <!-- Edge-kind toggles (only meaningful while a node is selected). Direct
       = edges incident to the selected node. Peer = edges between two
       nodes at the same level (e.g. two 1st-level neighbors calling each
       other). Decoupled so the user can see peer topology without the
       selected node's own edges, or vice versa. -->
  <div class="edge-kind-group">
    <label class="edge-kind-item">
      <input type="checkbox" bind:checked={$showDirectEdges} />
      <span>Direct edges to selected</span>
    </label>
    <label class="edge-kind-item">
      <input type="checkbox" bind:checked={$showCrossLevelEdges} />
      <span>Cross-level edges (L1↔L2↔L3)</span>
    </label>
  </div>

  {#each [1, 2, 3] as level}
    {@const lo = $levelOverrides[level]}
    {@const levelColors = ['', '#4CAF50', '#FF9800', '#E91E63']}
    {@const levelNames = ['', '1st Level (Direct)', '2nd Level (Indirect)', '3rd Level (3 hops)']}
    <div class="level-panel">
      <div class="level-header" on:click={() => toggleSection(`level-${level}-body`)}>
        <div class="level-header-left">
          <input type="checkbox" checked={lo.enabled} on:click|stopPropagation={() => toggleLevelEnabled(level)} />
          <ColorChip color={levelColors[level]} label={levelNames[level]} />
        </div>
        <span class="toggle-arrow">{'\u25B6'}</span>
      </div>
      <div id="level-{level}-body" class="level-body" class:disabled={!lo.enabled}>
        <div class="edge-kind-group inline">
          <label class="edge-kind-item">
            <input type="checkbox" checked={lo.peerEdges}
              on:change={() => toggleLevelPeerEdges(level)} />
            <span>Peer edges (between same-level nodes)</span>
          </label>
        </div>
        {#if $allEntityTypes.length > 0}
          <div class="sub-title">Entity Types</div>
          <div class="tri-state-group">
            {#each $allEntityTypes as type}
              <div class="tri-state-item" on:click={() => cycleTriState(level, 'entityTypes', type)}>
                <button class="tri-state-btn" data-state={lo.entityTypes[type] || 'general'}>
                  {triStateLabel(lo.entityTypes[type] || 'general')}
                </button>
                <ColorChip color={NODE_COLORS[type] || 'var(--text-muted)'} label={type} />
              </div>
            {/each}
          </div>
        {/if}
        <div class="sub-title">Direction</div>
        <div class="tri-state-group">
          <div class="tri-state-item" on:click={() => cycleDirectionTriState(level, 'outgoing')}>
            <button class="tri-state-btn" data-state={lo.outgoing}>{triStateLabel(lo.outgoing)}</button>
            <ColorChip color="#2196F3" label="Outgoing" />
          </div>
          <div class="tri-state-item" on:click={() => cycleDirectionTriState(level, 'incoming')}>
            <button class="tri-state-btn" data-state={lo.incoming}>{triStateLabel(lo.incoming)}</button>
            <ColorChip color="#E91E63" label="Incoming" />
          </div>
        </div>
        {#if $allRelTypes.length > 0}
          <div class="sub-title">Relationship Types</div>
          <div class="tri-state-group">
            {#each $allRelTypes as type}
              <div class="tri-state-item" on:click={() => cycleTriState(level, 'relTypes', type)}>
                <button class="tri-state-btn" data-state={lo.relTypes[type] || 'general'}>
                  {triStateLabel(lo.relTypes[type] || 'general')}
                </button>
                <ColorChip color={LINK_COLORS[type] || 'var(--text-muted)'} label={type} />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {/each}
</div>

<!-- Legend — UI-014: describes the ACTIVE encoding, not a fixed palette.
     Before, this explained the kind colours and nothing else, which was
     accurate only because kind was the only thing encoded. -->
{#if $allEntityTypes.length > 0}
<div class="filter-section" data-probe="legend">
  <h2>Legend</h2>

  <!-- The channel selector lives with the legend rather than beside the
       aggregation control in the toolbar. UI-013 fits nine clusters into two
       rows at 1280px with ~125px of slack, and two dropdowns need ~340px —
       measured at three rows, failing that ticket's probe. Here the control
       and the legend that explains it are one block, which is where a reader
       looks to answer "what does this circle mean?" anyway. -->
  <div class="encode-controls">
    {#if !$nodeEncoding.kindOnly}
      <label class="encode-field">
        <span class="encode-prefix">Size</span>
        <select class="encode-select" bind:value={$sizeChannel} aria-label="Metric driving node size">
          {#each $availableSizeChannels as c}
            <option value={c.id}>{c.label}</option>
          {/each}
        </select>
      </label>
      <label class="encode-field">
        <span class="encode-prefix">Colour</span>
        <select class="encode-select" bind:value={$colorChannel} aria-label="What node colour encodes">
          {#each COLOR_CHANNELS as c}
            <option value={c.id}>{c.label}</option>
          {/each}
        </select>
      </label>
      <!-- UI-106 — the two size controls. Disabled rather than hidden when
           the current channel has no value domain to shape (`Entity kind`,
           or a metric with no rollup at this level): a control that vanishes
           when you switch channel reads as a bug, and the reason it can't
           apply is worth stating once in a tooltip. -->
      <label class="encode-field">
        <span class="encode-prefix">Curve</span>
        <select
          class="encode-select"
          bind:value={$sizeCurve}
          disabled={!$nodeEncoding.sizeLegend}
          aria-label="How the metric maps onto node size"
          title={$nodeEncoding.sizeLegend
            ? 'How the value range is spread over the size range'
            : 'Needs a metric size channel — kind sizing has no range to shape'}
        >
          {#each SIZE_CURVES as c}
            <option value={c.id}>{c.label}</option>
          {/each}
        </select>
      </label>
      <!-- UI-110 — how many size classes the ramp collapses to. Continuous
           is a real option and the default, not a zero: it discards nothing,
           and grouping trades within-group differences for an answerable
           "which group is this in". Same disabled rule as the curve. -->
      <label class="encode-field">
        <span class="encode-prefix">Groups</span>
        <select
          class="encode-select"
          bind:value={$sizeBins}
          disabled={!$nodeEncoding.sizeLegend}
          aria-label="How many size groups node sizes snap to"
          title={$nodeEncoding.sizeLegend
            ? 'Snap every node to one of N sizes; the boundaries follow the curve'
            : 'Needs a metric size channel — kind sizing has no range to group'}
        >
          <option value={SIZE_BINS_OFF}>Continuous</option>
          {#each binChoices as n}
            <option value={n}>{n} groups</option>
          {/each}
        </select>
      </label>
    {/if}
    <!-- Offered even on a metric-free graph, where it is the only size
         control that still means something. -->
    <label class="encode-field">
      <span class="encode-prefix">Scale</span>
      <span class="encode-slider">
        <input
          type="range"
          min={SIZE_BOOST_MIN}
          max={SIZE_BOOST_MAX}
          step="0.1"
          bind:value={$sizeBoost}
          aria-label="Node size scale"
          title="Widens the gap between the smallest and the largest node"
        />
        <span class="encode-scale-value">{$sizeBoost.toFixed(1)}×</span>
      </span>
    </label>
  </div>

  {#if $nodeEncoding.sizeLegend}
    <div class="sub-title">Size — {$nodeEncoding.sizeLegend.label}</div>
    {#if $nodeEncoding.sizeLegend.bins === SIZE_BINS_OFF}
      <!-- Continuous: three samples off a smooth ramp, laid out along a row
           because their sizes are the comparison being made. -->
      <div class="size-ramp">
        {#each $nodeEncoding.sizeLegend.stops as stop}
          <div class="size-stop">
            <!-- Canvas radii shrunk by ONE shared factor, not clamped per dot.
                 `Math.min(r, 17)` capped every stop that mattered — the ramp's
                 radii run past 17 well before the midpoint — so all three dots
                 rendered at an identical 34px and the legend contradicted the
                 very channel it was explaining. Scaling keeps the ratio (and
                 so the area comparison) exactly the canvas's. -->
            <span
              class="size-dot"
              style="width: {stop.radius * legendDotScale * 2}px; height: {stop.radius * legendDotScale * 2}px"
            ></span>
            <span class="size-value">{stop.value.toLocaleString()}</span>
          </div>
        {/each}
      </div>
    {:else}
      <!-- Grouped: a row per class, stacked like the severity ramp rather
           than laid out along a row. Eight dots side by side do not fit
           ~220px of panel at any honest scale, and a class needs its value
           band next to it — which is a line of text, not a caption. -->
      <div class="size-groups">
        {#each $nodeEncoding.sizeLegend.stops as stop}
          <div class="size-group">
            <span class="size-group-dot">
              <span
                class="size-dot"
                style="width: {stop.radius * legendDotScale * 2}px; height: {stop.radius * legendDotScale * 2}px"
              ></span>
            </span>
            <span class="size-value">
              {#if stop.to === null}
                &gt; {(stop.from ?? 0).toLocaleString()}
              {:else if (stop.from ?? 0) === 0}
                ≤ {(stop.to ?? 0).toLocaleString()}
              {:else}
                {(stop.from ?? 0).toLocaleString()}–{(stop.to ?? 0).toLocaleString()}
              {/if}
            </span>
          </div>
        {/each}
      </div>
    {/if}
    <div class="legend-note">
      {$nodeEncoding.sizeLegend.unit} · {$nodeEncoding.sizeLegend.hint}{
        $nodeEncoding.sizeLegend.bins === SIZE_BINS_OFF
          ? ''
          : `, in ${$nodeEncoding.sizeLegend.bins} groups`}
    </div>
  {/if}

  {#if $nodeEncoding.colorLegend.kind === 'severity'}
    <div class="sub-title">Colour — refactor pressure</div>
    <div class="severity-ramp">
      {#each $nodeEncoding.colorLegend.steps ?? [] as step}
        <div class="severity-step">
          <span class="severity-swatch" style="background: {step.color}"></span>
          <span class="severity-band">
            {step.to === null ? `> ${step.from}` : `${step.from}–${step.to}`}
          </span>
          <span class="severity-tier tier-{step.tier}">{step.tier}</span>
        </div>
      {/each}
    </div>
    <div class="legend-item legend-nodata">
      <div
        class="legend-color"
        style="background: {NO_DATA_FILL}; opacity: {NO_DATA_FILL_OPACITY}"
      ></div>
      No metrics — not "zero"
    </div>
    <div class="legend-note">
      Same composite score the Quality tab ranks by. Lower is better.
    </div>
  {:else}
    <div class="sub-title">Colour — entity kind</div>
    <div class="legend">
      {#each $allEntityTypes as type}
        <div class="legend-item">
          <div class="legend-color" style="background: {NODE_COLORS[type] || '#9E9E9E'}"></div>
          {type}
        </div>
      {/each}
    </div>
  {/if}

  <!-- UI-108 — in the shape view an edge's colour means its VERDICT, not its
       relationship kind. That contradicts the Relationship Types block above,
       so the legend has to say which reading is live or it is explaining an
       encoding that is not on screen. Only rendered in that mode, for the
       same reason: a permanent key for six readings that do not apply is
       noise everywhere else. -->
  {#if $viewMode === 'shape' && $shapePicture}
    <div class="sub-title">Edges — how each one reads</div>
    <div class="legend">
      {#each SHAPE_VERDICT_ORDER as verdict}
        <div class="legend-item" title={VERDICT_TEXT[verdict]}>
          <div class="legend-color" style="background: {SHAPE_EDGE_COLORS[verdict]}"></div>
          <span class:shape-violation={isViolation(verdict)}>{verdict}</span>
        </div>
      {/each}
    </div>
    <div class="legend-note">
      Colour is the verdict here, not the relationship kind — every edge in
      this view is a merged dependency, so the kind palette would paint the
      whole picture one colour. Rows are levels: a dependency should step
      down exactly one.
    </div>
  {/if}
</div>
{/if}

<style>
  .search-first { margin-bottom: 4px; }

  /* The glyph already distinguishes the three states; the hue was doing
     redundant work at 2.01:1 and 2.15:1 on the light panel (UI-023). */
  .ts-g { color: var(--text-disabled); }
  .ts-on, .ts-off { color: var(--text-secondary); font-weight: 700; }

  /* Were #FFD54F and #4DD0E1 — 1.41:1 and 1.84:1 on the light theme's white
     panel. The two searches still read as different things via weight and
     the accent, not via low-contrast hues. The view search's own label
     moved to DisplaySearchResults.svelte with its list. */
  .search-label { color: var(--accent); font-weight: 600; }

  /* The three readings worth acting on carry weight as well as hue, so the
     key survives the light theme and a reader who cannot separate the
     oranges from the greys. */
  .shape-violation { font-weight: 700; }

  /* Section header rendered as a button so it is keyboard-operable; the
     wrapper keeps the <h2> for document outline without owning the click. */
  .section-heading-wrap { margin: 0; }
  .section-header-btn {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }

  h1 {
    font-size: 1.4rem;
    margin-bottom: 20px;
    color: var(--accent);
  }

  .repo-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 20px;
  }

  .repo-bar .back {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    border-radius: 4px;
    padding: 3px 8px;
    font: inherit;
    font-size: 0.75rem;
    cursor: pointer;
    flex-shrink: 0;
  }
  .repo-bar .back:hover { color: var(--text); border-color: var(--accent); }

  .repo-bar .repo-name {
    font-size: 1.1rem;
    font-weight: 600;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Section headings are subordinate to the block they sit in: the blocks
     are the workflow (analyse → visualise → post-filter) and the sections
     are steps inside one. They used to be the larger of the two, which read
     as nine peers under three labels. */
  h2 {
    font-size: 0.82rem;
    font-weight: 600;
    margin: 12px 0 6px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 4px;
    color: var(--text-secondary);
  }

  /* Block headers: a rule above and a brighter, smaller-caps title, so the
     three groups read as groups without adding a nesting level to every
     section heading below them. */
  .filter-block {
    margin: 18px 0 2px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }

  .filter-block:first-child {
    margin-top: 4px;
    padding-top: 0;
    border-top: none;
  }

  .block-title {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    /* 1.2rem bold = 19.2px, over the 18.66px bold threshold for WCAG "large
       text", where the floor is 3:1 rather than 4.5:1. That matters: --accent
       on --bg-surface measures 3.46 in obsidian and 4.15 in midnight, so an
       accent heading only clears the bar at this size. The pre-existing
       palette gap is UI-024's; this just avoids adding to it. */
    font-size: 1.2rem;
    font-weight: 700;
    color: var(--accent);
    /* Overrides the shared h2 rule above — a block heading carries its own
       rule via .filter-block's border-top and must not draw a second one. */
    border-bottom: none;
    padding-bottom: 0;
  }

  /* The step number: these are a sequence, not three unrelated groups.
     Outlined rather than filled. A filled badge needs a foreground that
     clears 4.5:1 against `--accent` in every theme, and `--accent-fg` does
     not: midnight's accent is #e94560 and its accent-fg is white, which
     measures 3.83:1 (caught by `ux-probe.mjs ui-023`). An outline reuses the
     accent-on-surface pair the block title already uses, so it introduces no
     new colour pair to audit. */
  .block-step {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    border-radius: 999px;
    border: 1px solid var(--accent);
    /* The digit is small text, so it gets no large-text allowance and must
       clear 4.5:1 on its own. --text-secondary does that in every theme
       (7.45 worst case); --accent would not. The ring stays accent — a
       border carries no contrast requirement. */
    color: var(--text-secondary);
    font-size: 0.7rem;
    font-weight: 700;
    flex-shrink: 0;
  }

  .block-note {
    margin: 3px 0 0;
    font-size: 0.66rem;
    line-height: 1.35;
    color: var(--text-dim);
  }

  .layer-note {
    margin: 0 0 6px;
    font-size: 0.68rem;
    line-height: 1.35;
    color: var(--text-dim);
  }

  /* Segmented control (UI-052). Mirrors the toolbar's `level-toggle` look —
     one bordered strip, the active segment carrying the accent — but the
     toolbar's rules are scoped to its own component, so the panel needs its
     own. `--text-secondary` for the inactive ink rather than `--accent`,
     which measured 3.46:1 on obsidian at this size (UI-051's table). */
  .seg-group {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
    width: fit-content;
  }

  .seg-btn {
    background: transparent;
    color: var(--text-secondary);
    border: none;
    border-right: 1px solid var(--border);
    padding: 3px 12px;
    font-size: 0.72rem;
    font-family: inherit;
    cursor: pointer;
  }
  .seg-btn:last-child { border-right: none; }
  .expand-collapse {
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .seg-btn:hover { background: var(--bg-hover); }
  .seg-btn.active {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--text);
    font-weight: 600;
  }
  /* UI-103. A grain the current level cannot honour still shows as the
     reader's choice — it is restored the moment they return to Entity level
     — but italicised so the canvas and the sidebar are not silently
     disagreeing. Same treatment as UI-090's redundant level button, and for
     the same reason: say it, do not disable it. */
  .seg-btn.grain-inert {
    font-style: italic;
    opacity: 0.6;
  }

  .filter-section { margin-bottom: 20px; }

  .filter-group { margin-bottom: 10px; }

  .filter-group input[type="text"] {
    width: 100%;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-body);
    color: var(--text);
  }

  .checkbox-group {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .checkbox-item {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.85rem;
    padding: 4px 8px;
    background: var(--bg-body);
    border-radius: 4px;
    cursor: pointer;
  }

  .checkbox-item:hover { background: var(--bg-hover); }

  /* Master toggle above a checkbox-group: transparent so it reads as a
     control over the tiles below rather than as one more tile. */
  .select-all {
    align-self: flex-start;
    display: inline-flex;
    background: transparent;
    color: var(--text-secondary);
    font-size: 0.8rem;
    font-weight: 500;
    margin-bottom: 4px;
    padding-left: 0;
  }

  .edge-kind-group {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 6px 0;
  }

  .edge-kind-group.inline { padding: 2px 0 6px; }

  .edge-kind-item {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8rem;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .edge-kind-item input[type="checkbox"] { cursor: pointer; margin: 0; }

  .search-hint { color: var(--text-disabled); font-weight: 400; font-size: 0.7rem; margin-left: 4px; }

  .search-scope { margin: 4px 0 10px; }
  .search-scope-toggle {
    background: transparent;
    color: var(--text-muted);
    border: none;
    font-size: 0.75rem;
    padding: 4px 0;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .search-scope-toggle:hover { color: var(--text-secondary); }
  .search-scope-body {
    margin-top: 4px;
    padding: 8px 10px;
    border: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
    border-radius: 4px;
    background: color-mix(in srgb, var(--bg-body) 50%, transparent);
  }
  .scope-badge {
    background: rgba(255, 213, 79, 0.15);
    color: #FFD54F;
    border-radius: 10px;
    padding: 1px 8px;
    font-size: 0.65rem;
  }
  .scope-clear-btn {
    margin-left: 8px;
    background: transparent;
    color: var(--text-secondary);
    border: 1px solid rgba(255, 213, 79, 0.4);
    border-radius: 3px;
    padding: 0 6px;
    font-size: 0.65rem;
    cursor: pointer;
  }
  .scope-clear-btn:hover { background: rgba(255, 213, 79, 0.12); }

  .sub-title {
    font-size: 0.8rem;
    color: var(--text-muted);
    margin: 10px 0 6px;
    padding-bottom: 3px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
  }

  .section-header {
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: space-between;
    user-select: none;
  }

  .toggle-arrow {
    font-size: 0.7rem;
    color: var(--text-dim);
  }

  .level-hint {
    font-size: 0.7rem;
    color: var(--text-dim);
    margin-bottom: 8px;
  }

  .level-panel {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-bottom: 12px;
    overflow: hidden;
  }

  .level-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    background: color-mix(in srgb, var(--bg-hover) 50%, transparent);
    cursor: pointer;
    user-select: none;
  }

  .level-header-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .level-body {
    padding: 10px 12px;
    display: none;
    background: color-mix(in srgb, var(--bg-body) 50%, transparent);
  }

  .level-body:global(.open) { display: block; }

  .level-body.disabled {
    opacity: 0.4;
    pointer-events: none;
  }

  .tri-state-group {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .tri-state-item {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.8rem;
    padding: 3px 6px;
    background: var(--bg-body);
    border-radius: 4px;
    cursor: pointer;
  }

  .tri-state-item:hover { background: var(--bg-hover); }

  .tri-state-btn {
    width: 22px;
    height: 18px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--bg-body);
    color: var(--text-disabled);
    cursor: pointer;
    font-size: 0.65rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-weight: bold;
  }

  .tri-state-btn[data-state="on"] { background: #1B5E20; color: #E8F5E9; border-color: #4CAF50; }
  .tri-state-btn[data-state="off"] { background: #B71C1C; color: #FFEBEE; border-color: #F44336; }
  .tri-state-btn[data-state="general"] { background: var(--bg-body); color: var(--text-disabled); border-color: var(--border); }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
  }

  .legend-item {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 0.75rem;
  }

  .legend-color {
    width: 12px;
    height: 12px;
    border-radius: 50%;
  }

  /* ── UI-014 encoding legend ─────────────────────────────────────────── */

  .encode-controls {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 8px 0 12px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .encode-field {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .encode-prefix {
    font-size: 0.72rem;
    color: var(--text-dim);
    min-width: 42px;
  }

  /* A DEFINITE width, not `flex: 1` — the sidebar sizes to its content, and a
     <select> takes its intrinsic width from its longest <option>. Left to
     shrink-to-fit these measured 271px each and pushed the panel to 319px,
     which cost the canvas enough width to wrap the toolbar to four rows and
     fail UI-020's layout check. `flex: 1; min-width: 0` does not help when
     the container itself has no definite width to divide up. */
  .encode-select {
    flex: 0 0 auto;
    width: 132px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 4px;
    padding: 4px 6px;
    font: inherit;
    font-size: 0.75rem;
    cursor: pointer;
  }
  .encode-select:hover { background: var(--bg-hover); }
  /* A curve with no value domain to shape still occupies its row — see the
     comment at the control. It has to *look* unavailable, not just refuse
     the click. */
  .encode-select:disabled {
    color: var(--text-disabled);
    cursor: not-allowed;
    opacity: 0.7;
  }
  .encode-select:disabled:hover { background: var(--bg-surface); }

  /* Same 132px budget as the selects above, for the same reason: the sidebar
     sizes to its content, so every control in this block has to declare a
     definite width or the panel grows to fit the widest one. */
  .encode-slider {
    flex: 0 0 auto;
    width: 132px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .encode-slider input[type='range'] {
    flex: 1 1 auto;
    min-width: 0;
    /* The one property that themes a native range track and thumb without
       rebuilding the control out of divs. */
    accent-color: var(--accent);
    cursor: pointer;
  }

  .encode-scale-value {
    flex: 0 0 auto;
    font-size: 0.7rem;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .legend-note {
    margin-top: 5px;
    font-size: 0.7rem;
    color: var(--text-dim);
  }

  .size-ramp {
    display: flex;
    align-items: flex-end;
    gap: 12px;
    margin-top: 8px;
  }

  .size-stop {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }

  /* Same ring the canvas draws, so a legend dot reads as the same object as
     a node — and so the smallest stop is still visible when its fill is
     close to the panel background. */
  .size-dot {
    display: block;
    border-radius: 50%;
    background: var(--text-dim);
    box-shadow: 0 0 0 1px var(--border-subtle);
  }

  .size-value {
    font-size: 0.7rem;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }

  /* Grouped legend (UI-110): one row per size class, stacked. */
  .size-groups {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 8px;
  }

  .size-group {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* A fixed-width column for the dot, so every band label starts at the same
     x whatever its class's radius is — a ragged left edge on the numbers
     makes eight rows unreadable, and the dots are already ordered by size. */
  .size-group-dot {
    flex: 0 0 auto;
    width: 46px;
    display: flex;
    justify-content: center;
    align-items: center;
  }

  .severity-ramp {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 8px;
  }

  .severity-step {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.72rem;
  }

  /* A bar rather than a dot: the ramp's job is to show ORDER, and stacked
     bars of equal width put the lightness progression on one edge where it
     can actually be compared. That progression is the channel that survives
     colour-blindness, so it is the one the legend has to make visible. */
  .severity-swatch {
    display: block;
    width: 34px;
    height: 12px;
    border-radius: 2px;
    box-shadow: 0 0 0 1px var(--border-subtle);
  }

  .severity-band {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    min-width: 56px;
  }

  .severity-tier { font-size: 0.66rem; text-transform: uppercase; letter-spacing: 0.03em; }
  .severity-tier.tier-ok { color: var(--tier-ok-fg); }
  .severity-tier.tier-warn { color: var(--tier-warn-fg); }
  .severity-tier.tier-bad { color: var(--tier-bad-fg); }

  .legend-nodata { margin-top: 8px; }
</style>
