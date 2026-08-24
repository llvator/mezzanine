<script lang="ts">
  /**
   * The strip above the canvas that says what is narrowing it (UI-099).
   *
   * One chip per filter that is actually biting, left to right in the order
   * the canvas applies them. Each chip has two halves and they do different
   * things on purpose: the label opens the Filters pane, where the full
   * control lives, and the `×` switches that one filter off without leaving
   * the canvas. A chip that only did the second would answer "make it stop"
   * and never "what else is this control holding".
   *
   * Drawn outside the collapsible part of the toolbar. Folding the view
   * controls is a request for canvas room; it is not a request to go back to
   * not knowing why half the graph is missing, and an active filter that can
   * be folded out of sight is the exact state this ticket exists to remove.
   *
   * The shape decisions live in `viewmodels/filterPipeline.ts`, which is pure
   * and unit-tested. This component's whole job is to read the stores into
   * that module's input and to map each stage id back to the action that
   * undoes it — the same separation `blockReason` and the search results list
   * already use for the per-node version of this question.
   */
  import { get } from 'svelte/store';
  import {
    allEntityTypes, allRelTypes, allLanguages, allFiles,
    generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming,
    hiddenLanguages, hiddenFiles,
    showGhostNodes, showBuiltinGhosts, showTemplateVars,
    showDirectEdges, showCrossLevelEdges, levelOverrides, treeMaxDepth,
    searchHidesNonMatches, selectedNode, graphData, structureOnly,
  } from '../stores/graph';
  import { isSpecNode } from '../types/graph';
  import {
    searchTerm, committedSearchIds, setAllLanguages, clearHiddenFiles,
  } from '../viewmodels/filterViewModel';
  import { crossFilterPaths, specSelectedNodes, clearSpecFocus } from '../stores/crossFilter';
  import { diffActive, diffFiltersEnabled, diffLevel, diffSeedFacet, diffDimOpacity } from '../stores/diff';
  import { demoteHubs, hubCount } from '../stores/settings';
  import { splitViewOpen, sidebarTab } from '../stores/panes';
  import { focusPane } from '../stores/keymap';
  import {
    filterPipeline, effectiveDepth, pipelineSummary,
    type FilterStage, type FilterStageId,
  } from '../viewmodels/filterPipeline';
  import { displayPlan } from '../viewmodels/displayPlan';

  /**
   * Which mode the canvas is *actually* drawing in — the plan's own answer,
   * not the `viewMode` store's.
   *
   * The two disagree in the states that matter here. `shape` with no picture
   * in hand yet draws a force graph, and `tree` with nothing selected does
   * too, because a tree has to be rooted at something. Reading the store would
   * make the strip go quiet about filters that are running in both.
   */
  $: view = $displayPlan.mode;

  /** A selection only filters when it is a node of *this* graph. A spec entity
   *  clicked in the split view sets the global selection without narrowing the
   *  code canvas — the cross-filter it applied is the filter, and it has its
   *  own chip. Same test `displayPlan` makes before it runs the BFS. */
  $: specSelected = $splitViewOpen && !!$selectedNode && isSpecNode($selectedNode);
  /** The shape view returns from `displayPlan` before the selection reach is
   *  ever computed: what is on screen there is one folder's drawn graph,
   *  chosen by the reader, and a selection inside it narrows nothing. Left
   *  unguarded the strip announced "Focus mod.rs · 1 hop" over a picture no
   *  focus had touched — which is precisely the lie it was built to prevent
   *  (UI-108), and worse than silence because it offers a `×` that would
   *  change nothing.
   *
   *  `RUNS_IN` would drop the chip anyway; this stays because the same flag
   *  drives `directional` in the viewmodel, and a focus nobody has means the
   *  direction and level rules have no BFS to steer. */
  $: focused =
    view !== 'shape'
    && $selectedNode && !specSelected && $graphData.nodes.some((n) => n.id === $selectedNode!.id)
      ? $selectedNode
      : null;

  /** Ghost and templating layers are only worth a chip when the graph holds
   *  something for them to hide. */
  $: hasGhosts = $graphData.nodes.some((n) => n.tags?.includes('ghost'));
  $: hasBuiltinGhosts = $graphData.nodes.some((n) => n.tags?.includes('ghost_stdlib'));
  $: hasTemplateVars = $graphData.nodes.some((n) => n.tags?.includes('template_var'));

  /** How much `structureOnly` is holding back, counted against the graph on
   *  screen. Zero on a document, spec or schema graph — nothing there has a
   *  body — and the chip stays away rather than naming a filter with nothing
   *  to filter. */
  $: bodyEntities = $graphData.nodes.filter((n) => n.body_of !== undefined).length;
  /** The one callable whose body is exempt: the selection, and only when its
   *  own internals are the ones on screen. A File rollup or a spec entity has
   *  no body to open, and naming it on the chip would promise a state the
   *  canvas is not in. */
  $: openBody =
    $selectedNode && $graphData.nodes.some((n) => n.body_of === $selectedNode!.original_id)
      ? $selectedNode.name
      : null;

  /** Exclusions counted against the *current* dataset. `hiddenFiles` remembers
   *  files from scopes the reader has left, on purpose (UI-047), and counting
   *  those would report a filter that is doing nothing here. */
  $: hiddenFileList = $allFiles.filter((f) => f !== '' && $hiddenFiles.has(f));
  $: hiddenLangList = $allLanguages.filter((l) => $hiddenLanguages.has(l));

  /** Per-level rules, rolled up. `off` only: a tri-state set to `on` widens
   *  the view, and this strip reports what narrows it. */
  $: peerHiddenLevels = [1, 2, 3].filter(
    (l) => $levelOverrides[l]?.enabled && $levelOverrides[l]?.peerEdges === false,
  );
  $: overridesOff = [1, 2, 3].reduce((total, l) => {
    const lo = $levelOverrides[l];
    if (!lo?.enabled) return total;
    const off = (r: Record<string, string>) => Object.values(r).filter((v) => v === 'off').length;
    return total + off(lo.entityTypes) + off(lo.relTypes)
      + (lo.outgoing === 'off' ? 1 : 0) + (lo.incoming === 'off' ? 1 : 0);
  }, 0);

  $: stages = filterPipeline({
    view,
    kinds: {
      hidden: $allEntityTypes.filter((t) => !$generalEntityTypes.has(t)),
      total: $allEntityTypes.length,
    },
    relations: {
      hidden: $allRelTypes.filter((t) => !$generalRelTypes.has(t)),
      total: $allRelTypes.length,
      outgoingHidden: !$generalOutgoing,
      incomingHidden: !$generalIncoming,
    },
    languages: { hidden: hiddenLangList, total: $allLanguages.length },
    files: { hidden: hiddenFileList, total: $allFiles.length },
    ghosts: {
      allHidden: !$showGhostNodes,
      builtinsHidden: !$showBuiltinGhosts,
      present: hasGhosts,
      builtinsPresent: hasBuiltinGhosts,
    },
    templateVars: { hidden: !$showTemplateVars, present: hasTemplateVars },
    structure: { on: $structureOnly, hidden: bodyEntities, open: openBody },
    spec: $crossFilterPaths === null
      ? null
      : { entities: $specSelectedNodes.map((n) => n.name), paths: $crossFilterPaths.length },
    search: $committedSearchIds.size > 0
      ? {
          term: $searchTerm.trim(),
          kept: $committedSearchIds.size,
          hides: $searchHidesNonMatches,
        }
      : null,
    // A diff that is only colouring the graph is not filtering it, and the
    // master toggle is what tells the two apart.
    diff: $diffActive && $diffFiltersEnabled
      ? { level: $diffLevel, facet: $diffSeedFacet, dims: $diffDimOpacity > 0 }
      : null,
    focus: focused
      ? {
          name: focused.name,
          depth: effectiveDepth($levelOverrides, $treeMaxDepth),
          mode: view === 'tree' ? 'tree' : 'force',
        }
      : null,
    levels: {
      directHidden: !$showDirectEdges,
      crossLevelHidden: !$showCrossLevelEdges,
      peerHiddenLevels,
      overridesOff,
    },
    hubs: $demoteHubs ? { count: $hubCount } : null,
  });

  /**
   * What each `×` undoes.
   *
   * Kept out of the viewmodel for the reason `blockReason` keeps its actions
   * out: the module that decides *what is running* should not also have to
   * know that the entity-kind filter is re-seeded from `allEntityTypes`.
   */
  const CLEAR: Record<FilterStageId, () => void> = {
    // Stepwise on purpose. With every ghost hidden, the first click restores
    // the library ones and the chip becomes "builtins hidden" — which is the
    // default, and is a different decision from the one just undone.
    ghosts: () => {
      if (!get(showGhostNodes)) showGhostNodes.set(true);
      else showBuiltinGhosts.set(true);
    },
    'template-vars': () => showTemplateVars.set(true),
    spec: () => clearSpecFocus(),
    structure: () => structureOnly.set(false),
    kinds: () => generalEntityTypes.set(new Set(get(allEntityTypes))),
    languages: () => setAllLanguages(true),
    files: () => clearHiddenFiles(),
    // Clearing the box is what drops the commit — `filterViewModel` enforces
    // that, so there is one path out of a search rather than two.
    search: () => searchTerm.set(''),
    // Stepwise, like `ghosts`. A seed split to one half is the narrower and
    // less expected of the two diff controls, so the first click widens it
    // back to the whole change; only then does the second turn the filtering
    // off — off, not unloaded: the colours and the Details pane's diff stay.
    diff: () => {
      if (get(diffSeedFacet) !== 'all') diffSeedFacet.set('all');
      else diffFiltersEnabled.set(false);
    },
    focus: () => selectedNode.set(null),
    relations: () => {
      generalRelTypes.set(new Set(get(allRelTypes)));
      generalOutgoing.set(true);
      generalIncoming.set(true);
    },
    levels: () => {
      showDirectEdges.set(true);
      showCrossLevelEdges.set(true);
      levelOverrides.update((lo) => {
        for (const l of [1, 2, 3]) {
          const level = lo[l];
          if (!level) continue;
          level.peerEdges = true;
          level.outgoing = 'general';
          level.incoming = 'general';
          for (const k of Object.keys(level.entityTypes)) level.entityTypes[k] = 'general';
          for (const k of Object.keys(level.relTypes)) level.relTypes[k] = 'general';
        }
        return { ...lo };
      });
    },
    hubs: () => demoteHubs.set(false),
  };

  function clearStage(stage: FilterStage): void {
    CLEAR[stage.id]();
  }

  /** Every chip's label goes to the same place: the Filters pane holds the
   *  full control for all but the diff ladder, which sits on the canvas
   *  itself and needs no travel. */
  function openControls(): void {
    sidebarTab.set('filters');
    focusPane('sidebar');
  }

  function clearAll(): void {
    for (const stage of stages) CLEAR[stage.id]();
  }
</script>

{#if stages.length > 0}
  <div
    class="pipeline"
    data-probe="filter-pipeline"
    data-stages={stages.length}
    role="group"
    aria-label={pipelineSummary(stages)}
  >
    <span class="pipeline-label" title="Applied in this order, left to right.">Filtered by</span>
    {#each stages as stage, i (stage.id)}
      {#if i > 0}
        <!-- The arrow is the claim: these run in sequence, and each one cuts
             what the one before it left. -->
        <span class="pipe-sep" aria-hidden="true">›</span>
      {/if}
      <span class="chip" class:dims={stage.dims} data-probe="filter-chip-{stage.id}">
        <button
          type="button"
          class="chip-body"
          title={`${stage.detail}\n\nClick to open the Filters pane.`}
          on:click={openControls}
        >
          <span class="chip-name">{stage.name}</span>
          <span class="chip-value">{stage.value}</span>
        </button>
        <button
          type="button"
          class="chip-off"
          data-probe="filter-off-{stage.id}"
          title={stage.clearHint}
          aria-label={stage.clearHint}
          on:click={() => clearStage(stage)}
        >×</button>
      </span>
    {/each}
    {#if stages.length > 1}
      <button
        type="button"
        class="clear-all"
        data-probe="filter-clear-all"
        title="Switch off all {stages.length} filters and show the whole scope"
        on:click={clearAll}
      >Clear all</button>
    {/if}
  </div>
{/if}

<style>
  /* Wraps rather than scrolls: with eight filters running, a strip that hid
     three of them behind an overflow would reproduce the problem it is here
     to fix. It is also the reason chips are as terse as they are. */
  .pipeline {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 3px 4px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border-subtle);
    font-size: 0.72rem;
    line-height: 1.4;
  }

  .pipeline-label {
    color: var(--text-dim);
    font-size: 0.64rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-right: 2px;
    white-space: nowrap;
    cursor: help;
  }

  .pipe-sep { color: var(--text-disabled); font-size: 0.7rem; }

  .chip {
    display: inline-flex;
    align-items: stretch;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
    background: var(--bg-surface);
    max-width: 100%;
  }

  .chip-body,
  .chip-off {
    background: none;
    border: none;
    font: inherit;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 2px 6px;
  }

  .chip-body {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    min-width: 0;
  }
  .chip-body:hover { background: var(--bg-hover); color: var(--text); }

  .chip-name { font-weight: 600; white-space: nowrap; }

  /* The one part allowed to lose characters — its full text is in the chip's
     own tooltip, and the name beside it still says which filter is being
     truncated. */
  .chip-value {
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A filter that dims rather than hides is a different promise: the node is
     still on the canvas. Stated in the border, so a reader hunting something
     missing can tell at a glance which chips could possibly be holding it. */
  .chip.dims { border-style: dashed; }

  .chip-off {
    border-left: 1px solid var(--border-subtle);
    color: var(--text-dim);
    font-size: 0.85rem;
    line-height: 1;
    padding: 2px 6px;
  }
  .chip-off:hover { background: var(--bg-hover); color: var(--text); }

  .clear-all {
    margin-left: 4px;
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    color: var(--text-dim);
    font: inherit;
    font-size: 0.68rem;
    padding: 2px 7px;
    cursor: pointer;
    white-space: nowrap;
  }
  .clear-all:hover { background: var(--bg-hover); color: var(--text); }
</style>
