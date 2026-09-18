<script lang="ts">
  import './app.css';
  import Sidebar from './components/Sidebar.svelte';
  import BuildStamp from './components/BuildStamp.svelte';
  import GraphView from './components/GraphView.svelte';
  import {
    graphData, selectedNode, viewMode, graphLevel,
    showLabels, showKindLabels, showLinkLabels,
    treeDensity, hoverDepth,
  } from './stores/graph';
  import type { GraphLevel } from './types/graph';
  import type { TreeDensity } from './stores/graph';

  import { setTreeDepth, levelOverrides, showGhostNodes, showBuiltinGhosts, showTemplateVars } from './stores/graph';
  import { activeTheme, applyTheme, autoFitView } from './stores/settings';
  import CanvasToolbar from './components/CanvasToolbar.svelte';
  import OverviewPanel from './components/OverviewPanel.svelte';
  import { publishGraph } from './viewmodels/filterViewModel';
  import { displayPlan } from './viewmodels/displayPlan';
  import { blankCanvasReason } from './viewmodels/emptyCanvas';
  import { searchMatchIds } from './viewmodels/filterViewModel';
  import { searchHidesNonMatches } from './stores/graph';
  import { connectLiveReload } from './stores/liveReload';
  import { loadDiff, diffActive, diffData, diffLevel, diffSeedFacet, diffChangedEdges, diffDimOpacity, diffContextOpacity, CONTEXT_OPACITY_FLOOR, knownCommits, stashes } from './stores/diff';
  import { branchInfo, fetchBranch } from './stores/branch';
  import { branchLabel } from './viewmodels/branchLabel';
  import { isDiffLevel } from './viewmodels/diffLevels';
  import ChangesBar from './components/ChangesBar.svelte';
  import {
    loadIndex, indexData, indexLoadError, selectedScopes, selectionStats,
    graphLoading, graphLoadError,
  } from './stores/scope';
  import { loadGraphData } from './transform';
  import { loadViews } from './stores/savedViews';
  import RepoPicker from './components/RepoPicker.svelte';
  import {
    serveMode, activeRepo, enterServeMode, selectRepo, slugFromHash, backToPicker,
  } from './stores/serveMode';
  import ConnectScreen from './components/ConnectScreen.svelte';
  import { checkCurrentEndpoint, connection, type Connection } from './stores/connection';
  import { endpoint } from './endpoint';

  let graphView: GraphView;

  /** True once boot has decided between picker and graph, so we don't
   *  flash the graph shell during the `/api/repos` probe. */
  let booted = false;

  // Panel state. Three columns flank the canvas: Filters/Quality on the
  // left, Details and Description on the right. Details moved out of the
  // sidebar's bottom half, where it was taking 35% of the height the scope
  // tree and the quality table needed (UI-011).
  // Whether the left column is open lives in `panes.ts` since UI-075: `0`
  // focuses it, and focusing a collapsed pane has to open it.
  $: leftCollapsed = !$sidebarPaneOpen;

  /** Window width, so the layout can decide whether a column still fits. */
  let winWidth = typeof window !== 'undefined' ? window.innerWidth : 1600;

  /**
   * Where the overview panel's bottom edge sits.
   *
   * A constant again since UI-150. It used to be measured, because the strip
   * it had to clear floated over the canvas and grew every time the diff badge
   * wrapped. That strip is a row of the shell now and covers nothing, so the
   * only thing left in this corner is the build stamp the webview pins there —
   * which is what this number cleared before the strip ever existed.
   */
  const overviewBottom = 60;

  // A window can't always hold three columns, and squeezing the canvas past
  // the point where the graph is readable defeats the purpose of having any
  // of them. So the right-hand side gets a budget — whatever is left after
  // the sidebar, the three 20px toggle strips and a canvas floor — and the
  // columns claim it in priority order.
  //
  // Details goes first: it's the one you keep an eye on while working, where
  // Description is read in bursts. It also shrinks to its minimum before it
  // gives up, so the narrow case is a narrower pane rather than no pane.
  // All three want ~1620px; below that Description drops out, and below
  // ~1220px so does Details. Collapsing the sidebar buys back its width.
  // The spec pane (ADR 0011) is a fourth column and claims its room *before*
  // the right-hand side, because it is the control surface the canvas is
  // being steered from: a cross-filter you cannot see the source of is worse
  // than a missing Description. It sits on the left, next to the sidebar, so
  // the reading order matches the direction of the interaction — pick a
  // concept, watch the code narrow to its right.
  /** 20px per collapse strip. The fourth appears with the spec pane, and the
   *  canvas earns one of its own now that it folds like the rest (UI-098). */
  $: specToggleShown = !isVscode() && !$specGraph.empty;
  $: canvasToggleShown = !isVscode();
  $: strips = (specToggleShown ? 4 : 3) + (canvasToggleShown ? 1 : 0);

  /**
   * Everything the budget needs, in one object (UI-093). The arithmetic itself
   * is `layoutPanes`, which is pure and tested — this is only the reading of
   * the stores it runs on.
   *
   * `focus` is null unless focus-expand is on, which is what makes the mode a
   * change of *inputs* rather than a second code path: with it off the
   * function behaves exactly as the reactive statements it replaced.
   */
  $: layoutInput = {
    winWidth,
    strips,
    sidebar: { present: !isVscode(), open: !leftCollapsed, want: $sidebarWidth, min: SIDEBAR_MIN_WIDTH },
    spec: { present: specToggleShown, open: $splitViewOpen, want: $specWidth, min: SPEC_MIN_WIDTH },
    details: { present: !isVscode(), open: $detailsPaneOpen, want: $detailsWidth, min: DETAILS_MIN_WIDTH },
    description: { present: !isVscode(), open: $describePaneOpen, want: $describeWidth, min: DESCRIPTION_MIN_WIDTH },
    // VS Code hosts the canvas and nothing else — the panes are native views
    // there — so the switch is a standalone-UI affair and the webview is
    // always drawing a graph.
    canvasOpen: !canvasToggleShown || $canvasPaneOpen,
    focus: $focusExpand ? $focusedPane : null,
  } satisfies LayoutInput;

  $: layout = layoutPanes(layoutInput);

  $: sidebarShownWidth = layout.widths.sidebar;
  $: specShownWidth = layout.widths.spec;
  $: detailsShownWidth = layout.widths.details;
  $: descriptionShownWidth = layout.widths.description;

  $: showSpec = specShownWidth > 0;
  $: showDetails = detailsShownWidth > 0;
  $: showDescription = descriptionShownWidth > 0;
  $: showCanvas = layout.widths.canvas > 0;

  /** Was a pane hidden by the window rather than by the user? The toggle
   *  says so, so a button that does nothing visible still explains itself. */
  $: specSquashed = layout.squashed.spec;
  $: detailsSquashed = layout.squashed.details;
  $: descriptionSquashed = layout.squashed.description;

  /** Details outranks Description in the budget, so asking for Description
   *  when Details has eaten the room has to close Details — otherwise the
   *  click produces nothing on screen. The reverse needs no help: opening
   *  Details already takes its share first. */
  function toggleDescription() {
    const next = !$describePaneOpen;
    describePaneOpen.set(next);
    if (!next) return;
    const opened = layoutPanes({ ...layoutInput, description: { ...layoutInput.description, open: true } });
    if (opened.widths.description === 0) detailsPaneOpen.set(false);
  }

  // Load data at startup: prefer embedded __GRAPH_DATA__ (HTML mode), otherwise
  // load the lightweight index so the user can pick a scope.
  // In HTML mode, seed filters before graphData.set so GraphView's initial
  // applyFilters sees the correct values (same reason as in scope.ts).
  // For scope-driven mode, applySelection() handles the filter seeding itself.
  import { onMount, onDestroy } from 'svelte';
  import {
    isVscode, onFocusFile, onFocusCursor, onSetScopes, onDrillIn, onCommand,
    reportSelection, reportQuality, reportFilters, reportLevelFilters, reportDiff, reportBranch, reportScopes, reportAnalysisScopes,
    reportDescription,
  } from './vscodeAdapter';
  import { changedFilesReading } from './stores/changedFiles';
  import { changedFilesPayload } from './viewmodels/changedFiles';
  import { description, describeOnHover } from './stores/description';
  import {
    detailsPaneOpen, describePaneOpen, detailsWidth, describeWidth, sidebarWidth,
    splitViewOpen, specWidth, focusExpand, canvasPaneOpen,
  } from './stores/panes';
  import {
    layoutPanes, maxWidthFor,
    DETAILS_MIN_WIDTH, SPEC_MIN_WIDTH, DESCRIPTION_MIN_WIDTH, SIDEBAR_MIN_WIDTH,
    type ColumnId, type LayoutInput,
  } from './viewmodels/paneLayout';
  import SpecGraphView from './components/SpecGraphView.svelte';
  import { specGraph, specSelection, clearSpecFocus } from './stores/crossFilter';
  import DescriptionPanel from './components/DescriptionPanel.svelte';
  import DetailsPanel from './components/DetailsPanel.svelte';
  import ShortcutBar from './components/ShortcutBar.svelte';
  import ShortcutHelp from './components/ShortcutHelp.svelte';
  import { sidebarPaneOpen } from './stores/panes';
  import { focusedPane, shortcutHelpOpen } from './stores/keymap';
  import { matchBinding, isTypingTarget, PANE_DIGIT, type PaneId } from './viewmodels/keymap';
  import { runCommand } from './viewmodels/keymapActions';
  import { focusScope, setScopes, drillIn, analysisScopes, setAnalysisScopes, ensureFullData, autoLevel } from './stores/scope';
  import { qualityRows, repoQuality, qualityAnalysisScope, qualitySortBy, currentEditorFile, tierFromScore } from './stores/quality';
  import type { QualityAnalysisScope, QualitySortKey } from './stores/quality';
  import {
    diffComputing, diffApiError, triggerDiff, stopDiff, diffFiltersEnabled, scopeToChanges,
    diffStopping, cancelDiff,
  } from './stores/diff';
  import { changedFileScope } from './viewmodels/diffScope';
  import { derived as svelteDerived, get, type Writable } from 'svelte/store';
  import {
    generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming, generalLanguages,
    allEntityTypes, allRelTypes, allLanguages,
    toggleEntityType, toggleRelType, toggleLanguage,
    toggleLevelEnabled, toggleLevelPeerEdges,
    cycleEntityTypeTriState, cycleRelTypeTriState, cycleDirectionTriState,
    showDirectEdges, showCrossLevelEdges,
  } from './viewmodels/filterViewModel';

  /**
   * The largest top-level folder that will actually render, for the
   * empty-state card.
   *
   * The card used to say "check the box next to one or more folders/files in
   * the Scope panel" while the panel showed 3 of 7 folders with the rest
   * below the fold — instructions pointing at something not on screen
   * (UI-017). Naming a concrete folder, and offering to select it, gives the
   * user a first graph to learn from.
   *
   * Used to skip anything at or above ENTITY_THRESHOLD, on the grounds that
   * those landed in the oversized-scope overlay. Since UI-061 they don't:
   * `pickLevel` collapses a large folder to file or module level and it
   * draws. The filter had two costs — it passed over the folder most worth
   * looking at, and on a repo whose top-level folders are all large it
   * returned null, leaving the card with no folder to name and the user with
   * the generic instruction UI-017 exists to avoid.
   */
  $: suggestedScope = (() => {
    const idx = $indexData;
    if (!idx) return null;
    const roots = Object.values(idx.nodes).filter(
      (n) => n.type === 'folder' && n.path !== '' && !n.path.includes('/'),
    );
    if (roots.length === 0) return null;
    return roots.reduce((a, b) => (b.entity_count > a.entity_count ? b : a));
  })();

  /** Next coarser aggregation level, or null at the coarsest. Drives the
   *  overflow card's first remedy — the one that helps most, because it
   *  divides the drawn count rather than trimming it. */
  $: coarserLevel = $graphLevel === 'entity' ? 'file' : $graphLevel === 'file' ? 'folder' : null;

  /** Collapse a level from the overflow card. Pins the level (autoLevel off)
   *  because the user asked for this one specifically — the same contract the
   *  toolbar's level buttons use. */
  function collapseOneLevel() {
    if (!coarserLevel) return;
    autoLevel.set(false);
    graphLevel.set(coarserLevel as GraphLevel);
  }

  /** Narrow the diff to the edits themselves — the remedy the overflow card
   *  offers. The master toggle comes too: the ladder is only consulted when
   *  it is on. */
  function showChangesOnly() {
    diffFiltersEnabled.set(true);
    diffLevel.set('edits');
    // The whole change, not whichever half was last selected: this remedy is
    // offered to a reader whose canvas is over the draw ceiling, and it has to
    // land them somewhere they can predict (UI-109).
    diffSeedFacet.set('all');
  }

  /** Stop the ladder filtering at all — the one-click way out of a canvas it
   *  emptied. The diff stays loaded and the colours stay on. */
  function clearDiffFilters() {
    diffFiltersEnabled.set(false);
  }

  /** Non-null when the scope produced nodes and every one of them is
   *  filtered out of sight (UI-064). The decision is in `emptyCanvas.ts`;
   *  what is left here is which remedies to offer. */
  $: blankCanvas = blankCanvasReason($displayPlan, $graphData.nodes.length);

  async function startHere() {
    if (!suggestedScope) return;
    await setScopes([suggestedScope.path]);
  }

  /** Bring the scope tree into view and flash it, so the card's instruction
   *  points at something the user can actually see. */
  function revealScopeTree() {
    const el = document.querySelector('[data-probe="scope-tree"]') as HTMLElement | null;
    if (!el) return;
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    el.classList.add('scope-tree-flash');
    setTimeout(() => el.classList.remove('scope-tree-flash'), 1600);
  }

  /** Find the best node for a cursor position: the innermost entity whose
   *  span covers the given line, scoped to the matching file. */
  function findNodeAtLine(
    nodes: readonly import('./types/graph').D3Node[],
    relativePath: string,
    line: number,
  ): import('./types/graph').D3Node | undefined {
    const pathMatchedNodes = nodes.filter((n) => {
      const fp = n.file_path;
      return (
        fp === relativePath ||
        fp.endsWith('/' + relativePath) ||
        relativePath.endsWith('/' + fp)
      );
    });
    // Synthetic entities (class fields, function parameters) inherit the
    // full span of their parent because the Rust-side Parameter model
    // doesn't carry a per-item span. Letting them compete as candidates
    // produces tie-breaking surprises: cursor on `class X:` would match
    // both X and every one of X's fields, and the smallest-span tie-break
    // can then pick a field — whose tree-view neighbourhood (one incoming
    // hop to the class) hides the class's actual methods. Exclude them
    // from cursor-follow so the container wins; they remain click-
    // selectable on the canvas.
    const realCandidates = pathMatchedNodes.filter(
      (n) =>
        n.kind_raw !== 'Parameter' &&
        n.kind_raw !== 'Branch' &&
        n.kind_raw !== 'Loop' &&
        !n.tags?.includes('class_field'),
    );
    const candidates = realCandidates.filter(
      (n) => n.line <= line && line <= n.end_line,
    );
    console.log(
      `[cursor-sync/svelte] findNodeAtLine relativePath=${relativePath} line=${line} totalNodes=${nodes.length} pathMatched=${pathMatchedNodes.length} real=${realCandidates.length} spanMatched=${candidates.length}`,
    );
    if (pathMatchedNodes.length === 0 && nodes.length > 0) {
      const samplePaths = Array.from(new Set(nodes.slice(0, 50).map((n) => n.file_path)));
      console.log(
        `[cursor-sync/svelte]   no path match — sample node.file_path values:`,
        samplePaths.slice(0, 8),
      );
    } else if (pathMatchedNodes.length > 0 && candidates.length === 0) {
      const spans = pathMatchedNodes.map((n) => `${n.name}[${n.line}..${n.end_line}]`);
      console.log(
        `[cursor-sync/svelte]   path matched ${pathMatchedNodes.length} node(s) but line ${line} is outside all spans:`,
        spans.slice(0, 10),
      );
    }
    if (candidates.length === 0) return undefined;
    // Pick the smallest span (innermost/most specific entity)
    const chosen = candidates.reduce((a, b) =>
      b.end_line - b.line < a.end_line - a.line ? b : a,
    );
    console.log(
      `[cursor-sync/svelte]   chose node ${chosen.name} (id=${chosen.id}, span=${chosen.line}..${chosen.end_line})`,
    );
    return chosen;
  }

  // Apply theme on startup and whenever the user changes it.
  activeTheme.subscribe((id) => applyTheme(id));

  let unsubFocusFile: (() => void) | undefined;
  let unsubFocusCursor: (() => void) | undefined;
  let unsubSetScopes: (() => void) | undefined;
  let unsubDrillIn: (() => void) | undefined;
  let unsubQuality: (() => void) | undefined;
  let unsubFilters: (() => void) | undefined;
  let unsubLevelFilters: (() => void) | undefined;
  let unsubDiff: (() => void) | undefined;
  let unsubBranch: (() => void) | undefined;
  let unsubScopes: (() => void) | undefined;
  let unsubAnalysisScopes: (() => void) | undefined;
  let unsubCommand: (() => void) | undefined;
  let unsubSelection: (() => void) | undefined;
  let unsubDescription: (() => void) | undefined;
  /** Coalesces the hover firehose into one post per settled pointer
   *  position — a fast drag across the canvas crosses dozens of nodes and
   *  each one would otherwise be a webview→host round trip. */
  let descriptionTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(async () => {
    // Attach VS Code listeners FIRST so any messages the extension sends
    // (setCompact, focusFile, setScopes) while we're loading the index are
    // queued and processed correctly. The extension buffers messages until
    // we post 'ready' below.
    if (isVscode()) {
      // Focus-file: scope to a single file.
      //   • If the cursor is inside an entity → tree view rooted on that entity
      //   • Else (cursor at file scope: imports, between entities, module-level) →
      //     graph view of the whole file, no specific selection
      unsubFocusFile = onFocusFile(async ({ relativePath, line }) => {
        currentEditorFile.set(relativePath);
        const ok = await focusScope(relativePath);
        if (!ok) return;
        const match =
          line !== undefined ? findNodeAtLine($graphData.nodes, relativePath, line) : undefined;
        if (match) {
          viewMode.set('tree');
          selectedNode.set(match);
        } else {
          viewMode.set('graph');
          selectedNode.set(null);
        }
      });

      // Focus-cursor: user moved the cursor. Try to select the matching
      // entity in the current view; if the view is collapsed (file/module
      // level) and doesn't contain the cursor's entity, re-scope to the
      // cursor's file so `applySelection` re-picks the level — the new
      // (single-file) scope almost always fits under RENDER_BUDGET and
      // comes back at entity level, at which point we can select.
      unsubFocusCursor = onFocusCursor(async ({ relativePath, line }) => {
        console.log(
          `[cursor-sync/svelte] onFocusCursor fired relativePath=${relativePath} line=${line} currentSelected=${$selectedNode?.id ?? '(none)'} viewMode=${$viewMode} graphDataNodes=${$graphData.nodes.length}`,
        );
        currentEditorFile.set(relativePath);
        let match = findNodeAtLine($graphData.nodes, relativePath, line);
        if (!match) {
          // Check whether the current view is collapsed (no entity node
          // anywhere in this file). If yes, auto-drill so the user sees
          // entities without having to click "Drill in" manually.
          const hasEntitiesForFile = $graphData.nodes.some(
            (n) => n.kind_raw !== 'File' && n.kind_raw !== 'Folder' && (
              n.file_path === relativePath ||
              n.file_path.endsWith('/' + relativePath) ||
              relativePath.endsWith('/' + n.file_path)
            ),
          );
          if (!hasEntitiesForFile) {
            console.log(
              `[cursor-sync/svelte]   → view is collapsed for this file; auto-drilling scope to ${relativePath}`,
            );
            const ok = await focusScope(relativePath);
            if (ok) match = findNodeAtLine($graphData.nodes, relativePath, line);
          }
        }
        if (match) {
          if (match.id !== $selectedNode?.id) {
            console.log(
              `[cursor-sync/svelte]   → switching to tree + selecting ${match.name} (id=${match.id})`,
            );
            viewMode.set('tree');
            selectedNode.set(match);
          } else {
            console.log(`[cursor-sync/svelte]   → same node already selected, no-op`);
          }
        } else if ($selectedNode) {
          // Cursor left the last entity — drop back to the whole-file graph.
          console.log(
            `[cursor-sync/svelte]   → no match; clearing selection and dropping to graph view`,
          );
          viewMode.set('graph');
          selectedNode.set(null);
        } else {
          console.log(`[cursor-sync/svelte]   → no match and no prior selection, no-op`);
        }
      });

      // Scope changes from the native TreeView: replace the selected scopes
      // and re-render the graph.
      unsubSetScopes = onSetScopes((paths) => {
        void setScopes(paths);
      });

      // Drill-in from the native Selection panel: same as double-clicking
      // a collapsed node on the canvas — narrow AND reset auto-level so
      // the view expands to whichever level fits the new (smaller) scope.
      unsubDrillIn = onDrillIn((path) => {
        void drillIn(path);
      });

      // Generic commands from the native "View Options" panel.
      unsubCommand = onCommand((command, value) => {
        switch (command) {
          case 'setViewMode':
            viewMode.set(value as 'graph' | 'tree');
            break;
          case 'setGraphLevel':
            // Manual level pick — pin it so applySelection stops auto-escalating.
            autoLevel.set(false);
            graphLevel.set(value as GraphLevel);
            break;
          case 'setTreeDepth':
            setTreeDepth(value as number);
            break;
          case 'setTreeDensity':
            treeDensity.set(value as TreeDensity);
            break;
          case 'setHoverDepth':
            hoverDepth.set(value as number);
            break;
          case 'setDescribeOnHover':
            describeOnHover.set(!!value);
            break;
          case 'setShowLabels':
            showLabels.set(!!value);
            break;
          case 'setShowKindLabels':
            showKindLabels.set(!!value);
            break;
          case 'setShowLinkLabels':
            showLinkLabels.set(!!value);
            break;
          case 'setAutoFit':
            autoFitView.set(!!value);
            break;
          case 'setShowGhosts':
            showGhostNodes.set(!!value);
            break;
          case 'setShowBuiltinGhosts':
            showBuiltinGhosts.set(!!value);
            break;
          case 'setShowTemplateVars':
            showTemplateVars.set(!!value);
            break;
          case 'clearSelection':
            selectedNode.set(null);
            break;
          case 'zoomIn':
            graphView?.zoomIn();
            break;
          case 'zoomOut':
            graphView?.zoomOut();
            break;
          case 'resetZoom':
            graphView?.resetZoom();
            break;
          case 'fitView':
            graphView?.fitView();
            break;
          case 'fitWidth':
            graphView?.fitWidth();
            break;
          case 'toggleViewMode':
            graphView?.toggleViewMode();
            break;
          case 'selectEntityById': {
            const id = value as string;
            const match = $graphData.nodes.find((n) => n.id === id);
            if (match) selectedNode.set(match);
            break;
          }
          case 'toggleEntityType': {
            const v = value as { name: string; enabled: boolean };
            toggleEntityType(v.name, v.enabled);
            break;
          }
          case 'toggleRelType': {
            const v = value as { name: string; enabled: boolean };
            toggleRelType(v.name, v.enabled);
            break;
          }
          case 'toggleLanguage': {
            const v = value as { name: string; enabled: boolean };
            toggleLanguage(v.name, v.enabled);
            break;
          }
          case 'setOutgoing':
            generalOutgoing.set(!!value);
            break;
          case 'setIncoming':
            generalIncoming.set(!!value);
            break;
          case 'selectAllEntityTypes':
            generalEntityTypes.set(new Set(value as string[]));
            break;
          case 'clearAllEntityTypes':
            generalEntityTypes.set(new Set());
            break;
          case 'selectAllRelTypes':
            generalRelTypes.set(new Set(value as string[]));
            break;
          case 'clearAllRelTypes':
            generalRelTypes.set(new Set());
            break;
          case 'toggleLevelEnabled':
            toggleLevelEnabled(value as number);
            break;
          case 'toggleLevelPeerEdges':
            toggleLevelPeerEdges(value as number);
            break;
          case 'cycleLevelEntityType': {
            const v = value as { level: number; key: string };
            cycleEntityTypeTriState(v.level, v.key);
            break;
          }
          case 'cycleLevelRelType': {
            const v = value as { level: number; key: string };
            cycleRelTypeTriState(v.level, v.key);
            break;
          }
          case 'cycleLevelDirection': {
            const v = value as { level: number; dir: 'outgoing' | 'incoming' };
            cycleDirectionTriState(v.level, v.dir);
            break;
          }
          case 'setShowDirectEdges':
            showDirectEdges.set(!!value);
            break;
          case 'setShowCrossLevelEdges':
            showCrossLevelEdges.set(!!value);
            break;
          case 'triggerDiff': {
            const v = value as { fromRef: string; toRef: string };
            void triggerDiff(v.fromRef, v.toRef);
            break;
          }
          case 'clearDiff':
            // Goes through the server, like the badge's own button: clearing
            // the four stores locally left the engine still following the
            // working tree, so the next save pushed the overlay back (UI-100).
            void stopDiff();
            break;
          case 'cancelDiff':
            // Not `clearDiff`, whatever the two names look like side by side.
            // That one leaves diff mode; this one ends the computation and
            // leaves the overlay the reader already had (UI-141).
            void cancelDiff();
            break;
          case 'scopeToChangedFiles': {
            // The button half of UI-135. Both rules that make this correct —
            // edits only, never the impact ripple; leaf files only, or
            // `minimizeSelection` widens the scope straight back to the repo —
            // live in `changedFileScope`, so this path and the automatic one
            // in `triggerDiff` cannot answer differently.
            const data = $diffData;
            if (!data) break;
            const { paths, droppedImpactOnly, droppedNonLeaf } =
              changedFileScope(data.entities, $indexData);
            console.log(
              `[mezz] scopeToChangedFiles: ${paths.length} files`, paths,
            );
            if (droppedNonLeaf.length > 0) {
              console.log('[mezz] scopeToChangedFiles: dropped non-file paths (folders / ghosts):', droppedNonLeaf);
            }
            if (droppedImpactOnly > 0) {
              console.log(`[mezz] scopeToChangedFiles: ignored ${droppedImpactOnly} impact-only / unchanged entities (not scoped)`);
            }
            void scopeToChanges(paths);
            break;
          }
          case 'setDiffLevel':
            if (isDiffLevel(value)) diffLevel.set(value);
            break;
          // The two toggles the ladder replaced (UI-088). Still accepted so an
          // extension host that hasn't been rebuilt alongside the webview
          // keeps working: `coreOnly` was the narrow rung, `changesOnly`
          // without it was the one that also kept impact-only ripple, which
          // `rewiring` now shows for a reason rather than by blanket.
          case 'setDiffCoreOnly':
            if (value) diffLevel.set('edits');
            break;
          case 'setDiffChangesOnly':
            if (value) diffLevel.set('rewiring');
            break;
          case 'setDiffDimOpacity':
            diffDimOpacity.set(Number(value));
            break;
          case 'setDiffContextOpacity':
            diffContextOpacity.set(Math.max(CONTEXT_OPACITY_FLOOR, Number(value)));
            break;
          case 'setDiffFiltersEnabled':
            diffFiltersEnabled.set(!!value);
            break;
          case 'setQualityAnalysisScope':
            console.log('[mezz] setQualityAnalysisScope →', value);
            qualityAnalysisScope.set(value as QualityAnalysisScope);
            break;
          case 'setQualitySortBy':
            qualitySortBy.set(value as QualitySortKey);
            break;
          case 'setAnalysisScopes':
            console.log('[mezz] setAnalysisScopes →', value);
            setAnalysisScopes(value as string[]);
            break;
          case 'setCurrentFile':
            console.log('[mezz] setCurrentFile →', value);
            currentEditorFile.set(value as string);
            break;
        }
      });

      // Broadcast node selection to the extension host so the native
      // Selection side view stays in sync with the graph.
      unsubSelection = selectedNode.subscribe((node) => {
        if (!node) {
          reportSelection(null);
          return;
        }
        reportSelection({
          entityId: node.id,
          originalId: node.original_id,
          name: node.name,
          qualifiedName: node.qualified_name,
          kind: node.kind,
          filePath: node.file_path,
          line: node.line,
          endLine: node.end_line,
          language: node.language,
          sourceCode: node.source_code,
          parameters: node.parameters,
          returnType: node.return_type,
          metrics: node.metrics as unknown as Record<string, unknown> | undefined,
        });
      });

      // Broadcast the hovered/selected node's description chain to the
      // native Description side view.
      unsubDescription = description.subscribe((state) => {
        clearTimeout(descriptionTimer);
        descriptionTimer = setTimeout(() => {
          reportDescription(state ? { source: state.source, chain: state.chain } : null);
        }, 90);
      });

      // Broadcast the current filter state to the native Filters view
      // whenever any of the filter stores change. svelteDerived coalesces
      // updates so we only fire once per "settled" state, not N times.
      const filterState = svelteDerived(
        [generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming, generalLanguages,
         allEntityTypes, allRelTypes, allLanguages, showGhostNodes, showBuiltinGhosts],
        ([$eTypes, $rTypes, $out, $in, $langs, $allE, $allR, $allL, $ghosts, $builtinGhosts]) => ({
          entityTypes: ($allE as string[]).map((name) => ({ name, enabled: ($eTypes as Set<string>).has(name) })),
          relTypes: ($allR as string[]).map((name) => ({ name, enabled: ($rTypes as Set<string>).has(name) })),
          directions: { outgoing: $out as boolean, incoming: $in as boolean },
          languages: ($allL as string[]).map((name) => ({ name, enabled: ($langs as Set<string>).has(name) })),
          showGhosts: $ghosts as boolean,
          showBuiltinGhosts: $builtinGhosts as boolean,
        }),
      );
      unsubFilters = filterState.subscribe((state) => reportFilters(state));

      // Broadcast per-level filter state (depth-1/2/3 tri-state overrides,
      // enabled/peerEdges, plus the Edge Display toggles).
      const levelState = svelteDerived(
        [levelOverrides, allEntityTypes, allRelTypes, showDirectEdges, showCrossLevelEdges],
        ([$lo, $allE, $allR, $direct, $cross]) => ({
          allEntityTypes: $allE,
          allRelTypes: $allR,
          levels: {
            1: { ...$lo[1] },
            2: { ...$lo[2] },
            3: { ...$lo[3] },
          },
          showDirectEdges: $direct,
          showCrossLevelEdges: $cross,
        }),
      );
      unsubLevelFilters = levelState.subscribe((state) => reportLevelFilters(state));

      // Broadcast diff state — active flag, refs, summary counts, filter
      // toggles, compute/error status. The native Diff view mirrors this.
      const diffState = svelteDerived(
        [diffActive, diffData, diffLevel, diffChangedEdges, diffDimOpacity, diffContextOpacity,
         diffComputing, diffStopping, diffApiError, selectedScopes, diffFiltersEnabled, selectedNode],
        ([$act, $data, $lvl, $edges, $dim, $ctx, $comp, $stopping, $err, $sel, $filtEn, $selNode]) => {
          // Match the filter used by `scopeToChangedFiles` — files with
          // real (core) changes only, not impact-only ripples.
          const changedFiles = $data
            ? new Set(
                $data.entities
                  .filter((e) =>
                    e.status === 'added' || e.status === 'removed'
                    || (e.status === 'modified' && e.source_changed === true)
                  )
                  .map((e) => e.file_path)
                  .filter((p) => typeof p === 'string' && p.length > 0),
              )
            : new Set<string>();
          return {
            active: $act,
            fromRef: $data?.from_ref,
            toRef: $data?.to_ref,
            summary: $data
              ? {
                  added: $data.summary.added,
                  removed: $data.summary.removed,
                  modified: $data.summary.modified,
                  modifiedSource: $data.summary.modified_source,
                  modifiedImpact: $data.summary.modified_impact,
                  unchanged: $data.summary.unchanged,
                }
              : undefined,
            level: $lvl,
            // How many reported edge changes the canvas cannot draw at any
            // rung: a disappeared edge has no line in the head graph, and an
            // endpoint the diff couldn't resolve has nowhere to attach. Sent
            // so the native view can say so rather than imply full coverage.
            undrawableEdges: $edges.removedCount + $edges.unplaceable,
            dimOpacity: $dim,
            // The second tier (UI-112): how loudly the rung's recruits are
            // drawn against the edits. Sent unconditionally — the native view
            // decides whether the current rung has anything to weight.
            contextOpacity: $ctx,
            computing: $comp,
            // Sent beside `computing` because the native view's Stop button
            // has three states, not two: absent, offered, and asked-for. The
            // engine ends at its next checkpoint, so a button that snapped
            // back to `Stop` the instant it was pressed would invite a second
            // press at the one moment nothing has happened yet (UI-141).
            stopping: $stopping,
            error: $err,
            hasScope: $sel.size > 0,
            changedFileCount: changedFiles.size,
            filtersEnabled: $filtEn,
            hasSelection: !!$selNode,
          };
        },
      );
      // Git's file list for the same comparison, already joined against the
      // graph, for the native Changes tree (UI-137). It goes out *inside* the
      // diff state rather than on a channel of its own, so the rows and the
      // ref pair they describe cannot arrive out of step — a row list labelled
      // with the previous comparison's refs would fetch every side against the
      // wrong pair and open a diff that looks entirely plausible.
      const changedFilesState = svelteDerived(
        [changedFilesReading, diffData, knownCommits, stashes],
        ([$reading, $diff, $known, $stashes]) => changedFilesPayload(
          $reading,
          $diff ? { from_ref: $diff.from_ref, to_ref: $diff.to_ref } : null,
          // Every commit the picker has been told about, so a hash can be
          // named by its subject (UI-139) — including one from a branch that
          // is not the listing on screen (UI-143). Absent lists are not an
          // error: the labels fall back to the ref itself, which is what the
          // header said before.
          { commits: $known, stashes: $stashes },
        ),
      );
      unsubDiff = svelteDerived(
        [diffState, changedFilesState],
        ([$diff, $files]) => ({ ...$diff, changedFiles: $files }),
      ).subscribe((state) => reportDiff(state));

      // Which branch the canvas is drawing. The webview has no canvas strip
      // to put the chip in, so the native Diff view renders it — see
      // `reportBranch` (UI-114). Sent as the finished label rather than the
      // server's answer, so both surfaces word it identically.
      void fetchBranch();
      unsubBranch = branchInfo.subscribe((info) => reportBranch(branchLabel(info)));

      // Broadcast the selected scope paths so the native Scopes tree can
      // sync its checkbox state. Needed whenever scope changes from outside
      // the tree (e.g. "Scope to changes" in the Diff view).
      unsubScopes = selectedScopes.subscribe((set) => reportScopes([...set]));

      // Same pattern for the separate analysis scope that drives Quality.
      unsubAnalysisScopes = analysisScopes.subscribe((set) => reportAnalysisScopes([...set]));

      // Broadcast scope-level quality (aggregate summary + top refactor
      // candidates) whenever the scoped graph changes. Combining the two in
      // one derived store guarantees the summary and rows arrive together.
      const qualityPayload = svelteDerived(
        [qualityRows, repoQuality, qualityAnalysisScope, qualitySortBy, currentEditorFile, diffData, selectedScopes, displayPlan],
        ([$rows, $summary, $analysisScope, $sortBy, $curFile, $diff, $selectedScopes, $plan]) => {
          // Descending sort by the chosen metric. Missing values sink to
          // the bottom so they don't crowd the top of the list.
          const MINUS_INFINITY = -1;
          const metricOf = (r: any, key: QualitySortKey): number => {
            if (key === 'score') return r.score ?? MINUS_INFINITY;
            const m = r.node?.metrics;
            if (!m) return MINUS_INFINITY;
            switch (key) {
              case 'pagerank':    return m.pagerank ?? MINUS_INFINITY;
              case 'cc':          return m.cyclomatic ?? MINUS_INFINITY;
              case 'cognitive':   return m.cognitive_complexity ?? MINUS_INFINITY;
              case 'nesting':     return m.max_nesting ?? MINUS_INFINITY;
              case 'loc':         return m.loc ?? MINUS_INFINITY;
              case 'params':      return m.param_count ?? MINUS_INFINITY;
              case 'fanIn':       return m.fan_in ?? MINUS_INFINITY;
              case 'fanOut':      return m.fan_out ?? MINUS_INFINITY;
              case 'wmc':         return m.wmc ?? MINUS_INFINITY;
              case 'chainDepth':  return m.chain_depth ?? MINUS_INFINITY;
              case 'methodCount': return m.method_count ?? MINUS_INFINITY;
              case 'fieldCount':  return m.field_count ?? MINUS_INFINITY;
              default:            return MINUS_INFINITY;
            }
          };
          const top = $rows
            .slice()
            .sort((a, b) => metricOf(b, $sortBy) - metricOf(a, $sortBy))
            .slice(0, 100)
            .map((r) => {
              const m = r.node.metrics!;
              return {
                id: r.node.id,
                name: r.node.name,
                kind: r.node.kind,
                file: r.node.file_path,
                line: r.node.line,
                score: r.score,
                tier: tierFromScore(r.score),
                isGhost: r.node.tags?.includes('ghost') ?? false,
                metrics: {
                  loc: m.loc,
                  cc: m.cyclomatic,
                  cognitive: m.cognitive_complexity,
                  nesting: m.max_nesting,
                  params: m.param_count,
                  fanIn: m.fan_in,
                  fanOut: m.fan_out,
                  fieldCount: m.field_count,
                  methodCount: m.method_count,
                  wmc: m.wmc,
                  chainDepth: m.chain_depth,
                  pagerank: m.pagerank,
                  inCycle: m.in_cycle,
                  smells: m.smells,
                },
                tiers: r.tiers,
              };
            });
          // Report which analysis-scope options the Quality view should
          // offer (so it can disable the ones whose prerequisites aren't met).
          const availableScopes: QualityAnalysisScope[] = ['scope'];
          if ($selectedScopes.size > 0) availableScopes.push('visualScope');
          if (($plan?.visibleNodeIds?.size ?? 0) > 0) availableScopes.push('visualSelection');
          if ($curFile) availableScopes.push('currentFile');
          if ($diff) availableScopes.push('changedFiles');
          return {
            summary: $summary as unknown as Record<string, unknown>,
            rows: top,
            analysisScope: $analysisScope,
            availableScopes,
            currentFile: $curFile,
            sortBy: $sortBy,
          };
        },
      );
      unsubQuality = qualityPayload.subscribe((payload: any) => {
        console.log('[mezz] qualityPayload → broadcast: analysisScope=', payload.analysisScope, 'rows=', payload.rows.length);
        reportQuality(payload);
      });

      // Listeners attached — tell the extension to flush queued messages.
      (window as any).__MEZZ_VSCODE__?.postMessage({ type: 'ready' });
    }

    // Which engine, and does it answer (UI-034)? Skipped in the webview,
    // where the extension has already supplied the endpoint and a picker
    // would be a regression. Same-origin is probed like any other: a page
    // the engine served passes in one request, and a page on a static host
    // with no engine behind it is exactly the visitor the screen is for.
    if (!isVscode()) {
      const verdict = await checkCurrentEndpoint();
      if (verdict.kind !== 'ok') {
        booted = true;
        return;
      }

      // Serve mode (UI-007): the backend hosts several repos, so one has to
      // be chosen before any data endpoint is meaningful. Which mode it is
      // came back with the handshake above rather than from a second probe.
      if (verdict.mode === 'serve') {
        await enterServeMode();
        const slug = slugFromHash();
        // A deep link to a slug the server doesn't have falls back to the
        // picker rather than firing a page of 404s against it.
        if (!slug || !selectRepo(slug)) {
          booted = true;
          return;
        }
      }
    }

    // Reveal the shell before loading data — `bootData` awaits the index,
    // and blocking the whole UI on it would regress watch mode, which
    // renders its "Loading…" overlay immediately today.
    booted = true;
    await bootData();
  });

  // Browser back/forward across `#/repo/{slug}`. `backToPicker` already
  // reloads; this catches the history-navigation case, where the hash
  // changes under us and the loaded data no longer matches the URL.
  function onHashChange() {
    if (!get(serveMode)) return;
    if (slugFromHash() !== (get(activeRepo)?.slug ?? null)) window.location.reload();
  }

  /** Load the active repo's data. In serve mode this runs after the user
   *  picks a repo; otherwise it's the ordinary watch/static boot. */
  async function bootData() {
    if ((window as any).__GRAPH_DATA__) {
      publishGraph(loadGraphData((window as any).__GRAPH_DATA__));
    } else {
      await loadIndex();
      // Eagerly populate the full-graph store so Quality's analysis scope
      // (which defaults to the whole repo) works before any visual scope
      // is picked. Silent best-effort; errors surface via graphLoadError.
      void ensureFullData().catch(() => {});
    }
    // Try to connect to the watch server's SSE endpoint for live reload.
    // No-ops in serve mode, which has no `/events`.
    connectLiveReload();
    // Try to load a diff overlay (from `mezz diff`). Silent no-op if
    // diff.json doesn't exist, and skipped entirely in serve mode.
    loadDiff();
    // Saved views (UI-082). After the repo is settled, since serve mode keys
    // its browser-side fallback by the slug. Never awaited: the list is a way
    // back to a picture, not a prerequisite for drawing one.
    void loadViews();
  }

  /**
   * A new endpoint was accepted. Reload rather than resuming in place.
   *
   * Same reason `backToPicker` reloads: every store derived from the old
   * engine — the index, the details cache, the memoized full-graph promise,
   * the filter sets seeded from the previous graph's kinds — would otherwise
   * need individual teardown, and one missed reset shows up as another
   * repo's data bleeding into the view.
   */
  function onEndpointChosen() {
    window.location.reload();
  }

  /** Picker selection: adopt the repo, then run the normal data boot. */
  async function onPickRepo(slug: string) {
    if (!selectRepo(slug)) return;
    await bootData();
  }

  onDestroy(() => {
    unsubFocusFile?.();
    unsubFocusCursor?.();
    unsubSetScopes?.();
    unsubDrillIn?.();
    unsubCommand?.();
    unsubSelection?.();
    unsubDescription?.();
    clearTimeout(descriptionTimer);
    unsubQuality?.();
    unsubFilters?.();
    unsubLevelFilters?.();
    unsubDiff?.();
    unsubBranch?.();
    unsubScopes?.();
    unsubAnalysisScopes?.();
  });

  /**
   * Every column resizes the same way, so there is one handler (UI-093).
   *
   * The two things that differ are which store the drag writes and which way
   * widening runs: the columns left of the canvas grow rightwards, the ones
   * right of it grow leftwards. `sign` is that, and the rest is shared.
   *
   * The ceiling comes from `maxWidthFor`, which runs the same budget the
   * layout runs. That is the point of asking rather than computing it here:
   * the handle can never stop somewhere the layout won't follow, because both
   * answers come out of one function.
   */
  const RESIZE: Record<ColumnId, { store: Writable<number>; min: number; sign: 1 | -1 }> = {
    sidebar: { store: sidebarWidth, min: SIDEBAR_MIN_WIDTH, sign: 1 },
    spec: { store: specWidth, min: SPEC_MIN_WIDTH, sign: 1 },
    details: { store: detailsWidth, min: DETAILS_MIN_WIDTH, sign: -1 },
    description: { store: describeWidth, min: DESCRIPTION_MIN_WIDTH, sign: -1 },
  };

  function startPaneResize(id: ColumnId, e: MouseEvent) {
    e.preventDefault();
    const { store, min, sign } = RESIZE[id];

    // Grabbing an edge is a claim of manual control, and it cannot coexist
    // with a mode that derives the width from where the keyboard is: the
    // focused column is already at its ceiling and every other one is pinned
    // to its floor, so the drag would have nothing to move. Seeding the store
    // from what is on screen first is what stops the pane snapping back to a
    // remembered width the moment the mode goes off (UI-094).
    if ($focusExpand) {
      store.set(Math.max(min, layout.widths[id]));
      focusExpand.set(false);
    }

    const startX = e.clientX;
    const startWidth = get(store);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const onMove = (ev: MouseEvent) => {
      const next = startWidth + sign * (ev.clientX - startX);
      const ceiling = maxWidthFor(id, { ...layoutInput, focus: null });
      store.set(Math.min(Math.max(min, ceiling), Math.max(min, next)));
    };

    const onUp = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }

  /**
   * The one keyboard entry point (UI-075).
   *
   * Bound at the window rather than per panel so a key survives its pane being
   * collapsed, and dispatched through `matchBinding` so the bar at the bottom
   * and the behaviour here can never drift: both read the same table. Nothing
   * is prevented unless a binding claimed it — an unclaimed key stays the
   * browser's.
   */
  function onKeydown(e: KeyboardEvent) {
    if (isVscode()) return;
    const typing = isTypingTarget(e.target as HTMLElement | null);
    const binding = matchBinding(e, $focusedPane, typing);
    if (!binding) return;
    if (runCommand(binding.command, { graphView })) e.preventDefault();
  }

  /**
   * Focus follows the pointer *press*, not hover: the panes are read while the
   * cursor sweeps the canvas, and taking focus on hover would make the live
   * key set flicker under a moving mouse. `data-pane` marks the regions —
   * matched by `closest` so a click anywhere inside a pane counts, including
   * on controls that stop propagation later.
   */
  function onPointerDownCapture(e: PointerEvent) {
    const el = (e.target as HTMLElement | null)?.closest?.('[data-pane]');
    const pane = el?.getAttribute('data-pane') as PaneId | null;
    if (pane) focusedPane.set(pane);
  }
</script>

<svelte:window
  bind:innerWidth={winWidth}
  on:hashchange={onHashChange}
  on:keydown={onKeydown}
  on:pointerdown|capture={onPointerDownCapture}
/>

{#if !booted}
  <!-- Deciding between connect screen, picker and graph. One local request;
       a spinner here would flash for longer than the wait itself. -->
  <div class="boot"></div>
{:else if $connection && $connection.kind !== 'ok'}
  <!-- Nothing answered on the resolved endpoint. A stranger gets a question
       they can act on rather than an empty graph and a console error. -->
  <ConnectScreen
    connection={$connection}
    attemptedBase={endpoint().base}
    onConnected={onEndpointChosen}
  />
{:else if $serveMode && !$activeRepo}
  <RepoPicker onSelect={onPickRepo} />
{:else}
<!-- Standalone, the build stamp rides at the right end of the shortcut bar.
     The webview has no bar, so there it keeps the fixed corner it had. -->
{#if isVscode()}
  <BuildStamp />
{/if}

<div class="app-shell">
<div class="app-root">
  <!-- Left sidebar (standalone mode only — in VS Code the "Scopes" and
       "View Options" native views replace it). -->
  {#if !isVscode()}
    <div class="panel left-panel" class:collapsed={leftCollapsed}
      class:pane-focused={$focusedPane === 'sidebar'}
      data-pane="sidebar"
      data-probe="sidebar-panel"
      style="width: {sidebarShownWidth}px">
      {#if !leftCollapsed}
        <Sidebar />
        <div class="resize-handle right" on:mousedown={(e) => startPaneResize('sidebar', e)}></div>
      {/if}
    </div>
    <!-- Collapsed, the strip carries the pane's digit as well as its arrow:
         folded columns are otherwise identical 20px arrows, and the reader
         has to open one to find out which it was. The digit is also the key
         that opens it, so the strip teaches the shortcut it stands in for. -->
    <button class="panel-toggle left" class:collapsed={leftCollapsed}
      title={leftCollapsed ? `Show the sidebar (${PANE_DIGIT.sidebar})` : 'Hide the sidebar'}
      on:click={() => sidebarPaneOpen.set(leftCollapsed)}>
      {#if leftCollapsed}<span class="toggle-digit">{PANE_DIGIT.sidebar}</span>{/if}
      <span class="toggle-arrow">{leftCollapsed ? '\u25B6' : '\u25C0'}</span>
    </button>
  {/if}

  <!-- Spec: the Elevator layer on its own canvas (ADR 0011). Rendered only
       when the project has one \u2014 the strip and the pane are both absent
       otherwise, rather than present and empty. -->
  {#if specToggleShown}
    <div class="panel spec-panel" class:collapsed={!showSpec}
      class:pane-focused={$focusedPane === 'spec'}
      data-pane="spec"
      data-probe="spec-panel"
      style="width: {specShownWidth}px">
      {#if showSpec}
        <SpecGraphView />
        <div class="resize-handle right" on:mousedown={(e) => startPaneResize('spec', e)}></div>
      {/if}
    </div>
    <button class="panel-toggle spec" class:collapsed={!showSpec}
      class:squashed={specSquashed}
      title={specSquashed
        ? 'The spec pane is hidden \u2014 the window is too narrow for it and the canvas'
        : showSpec ? 'Hide the spec pane' : `Show the spec pane (${PANE_DIGIT.spec})`}
      on:click={() => splitViewOpen.set(!$splitViewOpen)}>
      {#if !showSpec}<span class="toggle-digit">{PANE_DIGIT.spec}</span>{/if}
      <span class="toggle-arrow">{showSpec ? '\u25C0' : '\u25B6'}</span>
    </button>
  {/if}

  <!-- Graph. Foldable since UI-098: a window mirroring another one is a real
       place to put panes and a pointless place to put a graph, and the floor
       the canvas stops reserving is what lets four columns fit a window that
       could never hold five. The strip stays behind when the column goes —
       otherwise a window with no canvas has no mouse route back to one. -->
  {#if canvasToggleShown}
    <button class="panel-toggle canvas" class:collapsed={!showCanvas}
      data-probe="canvas-toggle"
      title={showCanvas ? 'Hide the graph' : `Show the graph (${PANE_DIGIT.graph})`}
      on:click={() => canvasPaneOpen.set(!$canvasPaneOpen)}>
      {#if !showCanvas}<span class="toggle-digit">{PANE_DIGIT.graph}</span>{/if}
      <span class="toggle-arrow">{showCanvas ? '◀' : '▶'}</span>
    </button>
  {/if}

  {#if showCanvas}
  <div class="canvas-column">
  <!-- Toolbar lives above the canvas, not on top of it. As an overlay it
       covered nodes at small window sizes, and fitView measures
       .graph-container — so the area under the toolbar counted as usable
       and the fit put nodes there. In flow, both problems go away. -->
  {#if !isVscode()}
    <CanvasToolbar {graphView} />
  {/if}

  <GraphView bind:this={graphView}>

    <!-- Placeholder / status overlay.
         Shown when:
           - the scope exceeds the render threshold (analysis still works
             in the side panels, the canvas just declines to draw), or
           - no scope is selected yet, or
           - a load error occurred -->
    {#if $indexData && !$graphLoading && ($displayPlan.overflow || $graphData.nodes.length === 0 || blankCanvas)}
      <div class="overlay">
        {#if $displayPlan.overflow}
          <div class="overlay-card warn">
            <h3>Too much to draw at once</h3>
            <p>
              This view wants <strong>{$displayPlan.overflow.drawn.toLocaleString()}</strong>
              nodes on screen; the canvas draws up to
              <strong>{$displayPlan.overflow.ceiling.toLocaleString()}</strong>.
              Both numbers move as you filter.
            </p>
            <p>
              <strong>Analysis continues in the side panels</strong> —
              Quality, Summary, Diff, and Context all work on the full scope.
            </p>
            <p>Draw less:</p>
            <!-- Only remedies the current state can actually apply. The old
                 card recommended "Changes only" unconditionally, which did
                 nothing twice over: the gate ran before the filters, and
                 there is no diff to filter unless one is loaded. -->
            <ul class="remedies">
              {#if coarserLevel}
                <li>
                  <button type="button" class="link-btn" on:click={collapseOneLevel}>
                    Collapse to {coarserLevel}
                  </button>
                  — one node per {coarserLevel === 'file' ? 'file' : 'folder'}
                </li>
              {/if}
              {#if $showGhostNodes}
                <li>
                  <button type="button" class="link-btn" on:click={() => showGhostNodes.set(false)}>
                    Hide external references
                  </button>
                  — library and stdlib nodes outside this repo
                </li>
              {/if}
              {#if $diffActive && !($diffFiltersEnabled && $diffLevel === 'edits')}
                <li>
                  <button type="button" class="link-btn" on:click={showChangesOnly}>
                    Show changed entities only
                  </button>
                </li>
              {/if}
              <li>
                Uncheck entity or relationship types in the sidebar, or narrow
                the <button type="button" class="link-btn" on:click={revealScopeTree}>Analysis Scope</button>.
              </li>
            </ul>
          </div>
        {:else if $graphLoadError}
          <div class="overlay-card warn">
            <h3>Failed to load</h3>
            <p>{$graphLoadError}</p>
          </div>
        {:else if blankCanvas}
          <!-- The scope drew nothing, and a filter is why. Without this the
               canvas is simply white while the side panels stay full, which
               reads as "this scope is empty" — the wrong conclusion, and the
               one that hid UI-064 for as long as it did. -->
          <div class="overlay-card">
            <h3>Everything here is filtered out</h3>
            <p>
              This scope holds <strong>{blankCanvas.built.toLocaleString()}</strong>
              {blankCanvas.built === 1 ? 'node' : 'nodes'}. The current filters
              hide all of them.
            </p>
            <p>Show more:</p>
            <ul class="remedies">
              {#if $diffActive && $diffFiltersEnabled}
                <li>
                  <button type="button" class="link-btn" on:click={clearDiffFilters}>
                    Clear the diff filters
                  </button>
                  — nothing in this scope
                  {$diffSeedFacet === 'new' ? 'is new' : $diffSeedFacet === 'existing' ? 'existing changed' : 'changed'}{$diffSeedFacet === 'all' && $diffLevel === 'edits' ? ' at the source level' : ''}
                </li>
                <!-- The seed split can empty a canvas on its own — a commit
                     that only adds files has no existing half at all — so the
                     way out of it has to be offered here, before the ladder
                     remedy that cannot help while half the change is excluded. -->
                {#if $diffSeedFacet !== 'all'}
                  <li>
                    <button type="button" class="link-btn" on:click={() => diffSeedFacet.set('all')}>
                      Show both new and existing changes
                    </button>
                    — the {$diffSeedFacet} half of this change is empty
                  </li>
                {/if}
                {#if $diffLevel !== 'neighbourhood'}
                  <li>
                    <button type="button" class="link-btn" on:click={() => diffLevel.set('neighbourhood')}>
                      Widen to the neighbourhood
                    </button>
                    — draw what the changes sit next to
                  </li>
                {/if}
                {#if $diffDimOpacity === 0}
                  <li>
                    <button type="button" class="link-btn" on:click={() => diffDimOpacity.set(0.15)}>
                      Fade the unchanged instead of hiding them
                    </button>
                  </li>
                {/if}
              {/if}
              {#if $specSelection.size > 0}
                <!-- The honest case this exists for: a Feature with no `cr:`
                     anywhere in its subtree claims no code, so the filter is
                     working and the answer is "nobody wrote down where this
                     lives". Saying that beats an unexplained white canvas. -->
                <li>
                  <button type="button" class="link-btn" on:click={clearSpecFocus}>
                    Clear the spec filter
                  </button>
                  — this spec entity may declare no <code>cr:</code> code reference
                </li>
              {/if}
              {#if $searchMatchIds.size > 0 && $searchHidesNonMatches}
                <li>
                  <button type="button" class="link-btn" on:click={() => searchHidesNonMatches.set(false)}>
                    Dim non-matches instead of hiding them
                  </button>
                </li>
              {/if}
              {#if !$showGhostNodes}
                <li>
                  <button type="button" class="link-btn" on:click={() => showGhostNodes.set(true)}>
                    Show external references
                  </button>
                  — this scope may be nothing but calls out of it
                </li>
              {/if}
              <li>
                Re-check entity or relationship types in the sidebar, or pick a
                different <button type="button" class="link-btn" on:click={revealScopeTree}>scope</button>.
              </li>
            </ul>
          </div>
        {:else}
          <div class="overlay-card">
            <h3>Pick a scope to visualize</h3>
            <p>
              Indexed: <strong>{$indexData.total_entities.toLocaleString()}</strong> entities and
              <strong>{$indexData.total_relationships.toLocaleString()}</strong> relationships across the whole repo.
            </p>
            {#if suggestedScope}
              <p>
                Start with <strong>{suggestedScope.path}</strong>
                ({suggestedScope.entity_count.toLocaleString()} entities), or check any
                folder in the <button type="button" class="link-btn" on:click={revealScopeTree}>Scope panel</button>.
              </p>
              <button type="button" class="start-here-btn" on:click={startHere}>
                Show {suggestedScope.path}
              </button>
            {:else}
              <p>
                Check the box next to one or more folders/files in the
                <button type="button" class="link-btn" on:click={revealScopeTree}>Scope panel</button>.
              </p>
            {/if}
            <p class="threshold-note">
              Large folders are drawn collapsed — one node per file, or per
              folder — so they render rather than being refused.
            </p>
          </div>
        {/if}
      </div>
    {:else if $graphLoading}
      <div class="overlay">
        <div class="overlay-card">
          <h3>Loading…</h3>
          <p>
            Fetching graph data for {$selectedScopes.size} selected
            scope{$selectedScopes.size === 1 ? '' : 's'}
            ({$selectionStats.entities.toLocaleString()} entities in scope)
          </p>
        </div>
      </div>
    {:else if $indexLoadError}
      <div class="overlay">
        <div class="overlay-card warn">
          <h3>Failed to load index</h3>
          <p>{$indexLoadError}</p>
          <p>Generate one with: <code>mezz analyze &lt;path&gt; -f json -o ui/public/data.json</code></p>
        </div>
      </div>
    {/if}

    <!-- Last in the slot so it paints over the canvas but under the overlay
         cards, which are full-cover and answer a more urgent question than
         "where am I" when they are up. -->
    <OverviewPanel {graphView} bottomInset={overviewBottom} />
  </GraphView>
  </div>
  {/if}

  <!-- Details: the entity under the pointer, or the pinned one. Its own
       column since UI-040 — as the sidebar's bottom half it took a third of
       the height the scope tree and the quality table were short of.
       In VS Code the native "Selection" view replaces it. -->
  {#if !isVscode()}
    <button class="panel-toggle details" class:collapsed={!showDetails}
      class:squashed={detailsSquashed}
      title={detailsSquashed
        ? 'Details is hidden — the window is too narrow for it and the canvas'
        : showDetails ? 'Hide details' : `Show details (${PANE_DIGIT.details})`}
      on:click={() => detailsPaneOpen.set(!$detailsPaneOpen)}>
      {#if !showDetails}<span class="toggle-digit">{PANE_DIGIT.details}</span>{/if}
      <span class="toggle-arrow">{showDetails ? '▶' : '◀'}</span>
    </button>
    <div class="panel details-panel" class:collapsed={!showDetails}
      class:pane-focused={$focusedPane === 'details'}
      data-pane="details"
      data-probe="details-panel"
      style="width: {detailsShownWidth}px">
      {#if showDetails}
        <div class="resize-handle left" on:mousedown={(e) => startPaneResize('details', e)}></div>
        <DetailsPanel />
      {/if}
    </div>
  {/if}

  <!-- Description: the graph as prose. Its own column rather than a tab
       sharing one with Details — reading it is a mode you stay in while the
       pointer sweeps the canvas, so it has to be visible at the same time as
       the metrics, not instead of them.
       In VS Code the native "Description" view replaces it. -->
  {#if !isVscode()}
    <button class="panel-toggle right" class:collapsed={!showDescription}
      class:squashed={descriptionSquashed}
      title={descriptionSquashed
        ? 'Descriptions are hidden — the window is too narrow for a third column'
        : showDescription ? 'Hide descriptions' : `Show descriptions (${PANE_DIGIT.description})`}
      on:click={toggleDescription}>
      {#if !showDescription}<span class="toggle-digit">{PANE_DIGIT.description}</span>{/if}
      <span class="toggle-arrow">{showDescription ? '▶' : '◀'}</span>
    </button>
    <div class="panel right-panel" class:collapsed={!showDescription}
      class:pane-focused={$focusedPane === 'description'}
      data-pane="description"
      data-probe="description-panel"
      style="width: {descriptionShownWidth}px">
      {#if showDescription}
        <div class="resize-handle left" on:mousedown={(e) => startPaneResize('description', e)}></div>
        <DescriptionPanel />
      {/if}
    </div>
  {/if}

</div>

<!-- Which branch this is, what to compare it against, and whether any of it
     is still current. A row of the shell rather than an overlay on the canvas
     since UI-150: as an overlay its width was the canvas's width, so every
     pane the reader opened — and every drag of the graph pane's edge — pushed
     the wrapping diff badge up over the graph it was describing. Standalone
     only, like the shortcut bar below it: the webview has its own chrome. -->
{#if !isVscode()}
  <ChangesBar />
{/if}

<!-- Which pane the keyboard is in, and what it can do from there (UI-075).
     Standalone only: in VS Code the panes are native views with their own
     focus model and their own keybinding surface, and a second one drawn
     inside the webview would describe keys the host never delivers. -->
{#if !isVscode()}
  <ShortcutBar ctx={{ graphView }} />
{/if}
</div>

{#if $shortcutHelpOpen && !isVscode()}
  <ShortcutHelp />
{/if}
{/if}

<style>
  .boot {
    height: 100vh;
    width: 100vw;
    background: var(--bg-deep);
  }

  /* The shell exists so the shortcut bar can be a real row rather than an
     overlay: floating it would have covered the canvas's own bottom-left
     controls, and the bar is read while the pointer is down there. */
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
  }

  .app-root {
    display: flex;
    flex: 1;
    min-height: 0;
    width: 100%;
    overflow: hidden;
  }

  /* Focus is drawn inset, not as an outline: the panes sit edge to edge, and
     an outline would be clipped by the neighbour's overflow on one side. */
  .panel.pane-focused {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  /* A pane the window squashed keeps the focus but not the ring — a 1px
     accent line on a zero-width column is just a stripe. */
  .panel.collapsed.pane-focused { box-shadow: none; }

  .panel {
    background: var(--bg-surface);
    overflow: hidden;
    position: relative;
    transition: width 0.2s;
    flex-shrink: 0;
  }

  .left-panel { border-right: 1px solid var(--border); }
  .spec-panel { border-right: 1px solid var(--border); }
  .details-panel { border-left: 1px solid var(--border); }
  .right-panel { border-left: 1px solid var(--border); }

  .panel.collapsed {
    width: 0 !important;
    border: none;
  }

  .resize-handle {
    position: absolute;
    top: 0;
    width: 5px;
    height: 100%;
    cursor: col-resize;
    z-index: 10;
  }

  .resize-handle:hover { background: var(--accent); }
  .resize-handle.right { right: 0; }
  .resize-handle.left { left: 0; }

  .panel-toggle {
    position: relative;
    z-index: 20;
    width: 20px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    cursor: pointer;
    display: flex;
    /* Column, because the strip is 20px wide and full height: what room it
       has for a second glyph is vertical. */
    flex-direction: column;
    gap: 4px;
    align-items: center;
    justify-content: center;
    font-size: 0.7rem;
    flex-shrink: 0;
  }

  /* The arrow says what the click does; the digit says which pane it does it
     to, and which key does the same thing without the mouse. Accent-coloured
     so it reads as a key rather than as a count. */
  .toggle-digit {
    font-size: 0.65rem;
    font-weight: 600;
    line-height: 1;
    color: var(--accent);
  }

  .panel-toggle.squashed .toggle-digit { color: inherit; }

  .panel-toggle:hover { background: var(--bg-hover); color: var(--text); }
  .panel-toggle.left { border-left: none; border-right: none; }
  .panel-toggle.spec { border-left: none; border-right: none; }
  .panel-toggle.details { border-left: none; border-right: none; }
  .panel-toggle.right { border-left: none; border-right: none; }
  .panel-toggle.canvas { border-left: none; border-right: none; }

  /* Wanted, but the window has no room for it. Dimmed rather than hidden:
     the strip is where the pane comes back from once the window grows. */
  .panel-toggle.squashed {
    color: var(--text-disabled);
    background: var(--bg-deep);
  }

  .canvas-column {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
  }

  .start-here-btn {
    margin-top: 10px;
    padding: 8px 18px;
    border-radius: 6px;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
    font: inherit;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
  }
  .start-here-btn:hover { background: color-mix(in srgb, var(--accent) 28%, transparent); }

  .link-btn {
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    color: var(--accent);
    text-decoration: underline;
    cursor: pointer;
  }

  .threshold-note { color: var(--text-dim); font-size: 0.85em; }
  /* Remedies read as a list of moves, not a paragraph to parse. Each row is
     an action plus what it costs you. */
  .remedies { margin: 0.4em 0 0; padding-left: 1.2em; text-align: left; }
  .remedies li { margin-bottom: 0.35em; }

  .overlay-card {
    pointer-events: auto;
    max-width: 520px;
    background: color-mix(in srgb, var(--bg-surface) 95%, transparent);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 24px 28px;
    color: var(--text-secondary);
    font-size: 0.9rem;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.4);
  }

  .overlay-card.warn {
    border-color: #c04040;
  }

  .overlay-card h3 {
    margin: 0 0 12px;
    color: var(--text);
  }

  .overlay-card p {
    margin: 8px 0;
    line-height: 1.5;
  }

  .overlay-card code {
    background: var(--bg-deep);
    padding: 2px 6px;
    border-radius: 3px;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.8rem;
  }
</style>
